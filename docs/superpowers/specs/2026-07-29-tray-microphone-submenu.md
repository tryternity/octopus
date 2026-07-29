# 托盘菜单：麦克风快捷选择子菜单

**日期**：2026-07-29
**类型**：新功能
**分支**：`daily_feature_0729`

## 背景与动机

当前切换麦克风需进「设置 → 通用 → 麦克风」下拉。麦克风切换是语音识别高频操作（外接/内置切换、降噪麦克风切换），每次进设置页成本高。托盘菜单已有「引擎信息」只读项，是天然的快捷入口位置。

## 需求（用户决策 2026-07-29）

在托盘菜单的「语音识别」与「引擎信息」之间插入**麦克风子菜单**：
- 父项文案：`麦克风: <当前麦克风名>`（显示当前选中的设备名）
- 点击父项展开子菜单，列出系统所有麦克风，可直接点选切换
- 当前选中项带勾选标记（checkmark）

## 设计

### 菜单结构（macOS）

```
系统设置
───────────
语音识别（⌘⇧A）
麦克风: MacBook Pro 麦克风          ▶  ← 新增 Submenu，hover/点击展开
  ├─ ✓ MacBook Pro 麦克风            ← CheckMenuItem，当前选中
  ├─  外接 USB 麦克风
  └─  默认设备
引擎  sensevoice[本地]                ← 原只读 engine_info
───────────
...
```

### 复用现有逻辑（不重复造轮子）

| 能力 | 复用 | 位置 |
|---|---|---|
| 枚举麦克风 | `list_microphones() -> Vec<String>` | `settings_commands.rs:77`（cpal `input_devices`，已用于 get_config） |
| 当前麦克风 | `cfg.microphone` | `AppConfig.microphone`（空串=系统默认） |
| 切换 + 持久化 | `set_config("microphone", value)` | `settings_commands.rs:92`（写 DB + 更新 runtime config） |

托盘层只做**菜单 UI + 事件分发**，切换逻辑复用 set_config（保证持久化与设置页一致）。

### 实现（`tray.rs`）

#### 1. 子菜单构建函数

```rust
/// 构建麦克风子菜单：父项「麦克风: <current>」+ 设备列表 CheckMenuItem。
/// 当前选中的设备（cfg.microphone）打勾；空串匹配「默认设备」项。
fn build_microphone_submenu(app, cfg) -> Result<Submenu> {
    let mics = list_microphones_from_settings();  // 复用 settings_commands 枚举逻辑
    let current = &cfg.microphone;
    // 子项 id 约定：mic:{device_name}（设备名作 id，点击时反查）
    let items: Vec<CheckMenuItem> = mics.iter().map(|name| {
        CheckMenuItem::with_id(app, format!("mic:{name}"), name, true, name == current, None)
    }).collect();
    // 父项文案：麦克风: <current 或 "默认设备">
    let parent_text = format!("麦克风: {}", if current.is_empty() { "默认设备" } else { current });
    Submenu::with_items(app, parent_text, true, items)  // 注：需确认 Submenu::with_items 签名
}
```

#### 2. 菜单组装插入位置

`create_tray` 中，在 `toggle` 之后、`engine_info` 之前插入 `mic_submenu`：
```rust
let menu = Menu::with_items(app, &[
    &settings, &sep_settings,
    &toggle,
    &mic_submenu,        // ← 新增
    &engine_info, &sep1,
    ...
])
```

#### 3. 事件处理（点击设备项）

`on_menu_event` 增加分枝——id 以 `mic:` 前缀开头的，解析设备名并调 set_config：
```rust
id if id.starts_with("mic:") => {
    let device = &id[4..];
    // 复用 set_config 持久化 + 更新 runtime config
    set_microphone_from_tray(app, device)?;
    // 更新子菜单 checkmark（旧项取消，新项勾选）+ 父项文案
    update_microphone_submenu(app, device);
}
```

#### 4. 状态更新

- **checkmark 切换**：用 `CheckMenuItem::set_checked`——记录旧选中项 handle，取消勾；新选中项勾选。
- **父项文案更新**：用 `Submenu::set_text` 更新为新的「麦克风: <name>」。
- **设备列表变化**（热插拔）：暂不自动重建（低频场景）；用户重启 app 或改配置触发 rebuild_tray_labels 时顺带重建。可在 `rebuild_tray_labels` 里加麦克风子菜单重建。

### TrayItems 结构扩展

`TrayItems` 加 `mic_submenu: Submenu` + `mic_items: Vec<CheckMenuItem>` handle，供动态更新。

### 关键约束

- **macOS 专属**：与 record_start 同级，`#[cfg(target_os = "macos")]`（非 macOS 不编译，与现有录屏组一致）。
- **不阻塞事件循环**：`list_microphones` 是同步 cpal 调用（快），但设备多时可能稍慢——构建时一次性，不在事件处理里调。
- **设备名作 id 的风险**：设备名含特殊字符可能影响 id 匹配。用 `mic:` 前缀 + 设备名后缀，事件处理用 `strip_prefix` 解析。

## 不在本次范围

- 麦克风热插拔实时监听（macOS 可用 `AudioObjectAddPropertyListener`，本次不做，重启/改配置时重建即可）。
- 非 macOS 支持（与 record 组对齐，仅 macOS）。
- 录屏专用麦克风（`record_microphone_device`）的独立切换——复用 ASR 的 `microphone` 配置（与现有 `resolve_mic_device_name` 三级回退一致）。

## 验证

```bash
cargo build -p octopus-desktop   # 编译
# 手动冒烟：托盘菜单点「麦克风」子菜单，切换设备，确认 checkmark + 父项文案更新
```

## 实现注记（2026-07-29）

**已实现**，与设计基本一致，偏差如下：

1. **跨平台而非 macOS 专属**：麦克风枚举用 cpal（跨平台），托盘菜单也跨平台，故麦克风子菜单不限定 `#[cfg(target_os = "macos")]`（与录屏组不同，录屏限 macOS 是 record crate 只 mac provider）。

2. **「默认设备」项**（用户确认）：子菜单首项固定为「默认设备」（id=`mic:default`），对应 `microphone` 配置空串。其余为 cpal 枚举的实际设备。

3. **Checkmark 更新**：用 `Submenu::items()` 取子项 `MenuItemKind`，`as_check_menuitem()` 转回 `CheckMenuItem` 调 `set_checked`。不存 Rust 层 CheckMenuItem handle——muda 层自管 ownership，Rust 层局部变量 drop 后子菜单项仍存活。

4. **持久化路径**：`switch_microphone_from_tray` 复用 `runtime_config::SharedRuntimeConfig` + `save_app_config`（与 `set_config` 等价），不重启 audio stream（下次 build_stream 生效，与设置页一致）。

5. **rebuild_tray_labels 内联**：语言切换时父项文案在 `rebuild_tray_labels` 内联更新（不调独立函数，避免 TRAY_ITEMS 重入死锁）。子项设备名不随语言变，checkmark 状态不变。

6. **死锁规避**：`update_microphone_submenu` 与 `rebuild_tray_labels` 都需 lock `TRAY_ITEMS`，后者已持锁，故 rebuild 内联不调前者。
