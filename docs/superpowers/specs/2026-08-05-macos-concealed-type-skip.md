# 跨平台密码管理器 Concealed Type 跳过记录

- 日期：2026-08-05
- 类型：安全增强（bug fix 类，防敏感数据泄露）
- 优先级：P3
- 依赖：`clipboard-rs 0.3.4+`（三平台后端均含 `ContentFormat::Other` 任意类型检测）
- 演进：初版仅 macOS（2026-08-05），同日扩展 Windows + Linux（本 spec 现况）

## 1. 背景与问题

各平台密码管理器（1Password / Bitwarden / iCloud Keychain / KeePassXC 等）复制密码 / 一次性验证码（TOTP）时，会按平台约定在剪贴板上额外标记一个特殊类型，明确告知消费方「这是敏感数据，不要记录」：

| 平台 | 标记类型 | 来源 |
|---|---|---|
| macOS | `org.nspasteboard.ConcealedType` | [nspasteboard.org](https://nspasteboard.org/) 社区约定 |
| Windows | `ExcludeClipboardContentFromMonitorProcessing` | MS 官方 clipboard format 名，密码管理器写入此 format 通知 clipboard history 跳过 |
| Linux (X11/Wayland) | `x-kde-passwordManagerHint` | KDE / KeePassXC 事实约定（MIME 名）；GNOME 无统一标准但此 MIME 已成跨密码管理器共识 |

**当前问题**：octopus 的剪贴板 watcher（`handle_clipboard_change`，`crates/clipboard/src/watcher.rs`）在判断 files / image / text 类型时**没有检测这些 concealed hint**，会把密码内容当 text 入库：

```
密码管理器复制密码 → 剪贴板标记 concealed hint + 写 password 文本
→ octopus watcher 触发 → handle_clipboard_change → 走 text 分支
→ insert_clipboard_item 写入 SQLite clipboard_history（content=明文密码）
→ FTS5 索引（clipboard_history_fts）收录明文密码（可被搜索）
→ 跨设备 sync（如果被收藏，favorites 同步会传播）
```

**这是敏感数据泄露风险**——密码明文存本地 DB + FTS 索引 + 潜在跨设备传播。

## 2. 方案

### 2.1 核心改动

在 `crates/clipboard/src/watcher.rs::handle_clipboard_change` 函数开头（files / image / text 三分支**之前**）加跨平台 concealed hint 检测，命中则静默 `return`：

```rust
pub fn handle_clipboard_change(handle: &crate::ClipboardHandle) {
    use clipboard_rs::common::{ContentFormat, RustImage};
    // ... 其他 use

    // ── 密码管理器 concealed hint 检测（跨平台）──
    // 各平台密码管理器复制密码时按平台约定标记特殊类型，告知消费方不要记录。
    // 静默跳过避免密码明文入库 + FTS5 索引 + 跨设备 sync 传播。
    //
    // clipboard-rs 0.3.4 三平台后端（macos.rs/win.rs/x11.rs/wayland.rs）的
    // ContentFormat::Other 均支持任意类型字符串检测，故复用同一模式。
    const CONCEALED_HINTS: &[&str] = &[
        #[cfg(target_os = "macos")]
        "org.nspasteboard.ConcealedType",
        #[cfg(target_os = "windows")]
        "ExcludeClipboardContentFromMonitorProcessing",
        #[cfg(any(target_os = "linux", target_os = "dragonfly", target_os = "freebsd", target_os = "netbsd", target_os = "openbsd"))]
        "x-kde-passwordManagerHint",
    ];
    for hint in CONCEALED_HINTS {
        if handle.has(ContentFormat::Other((*hint).to_string())) {
            return;
        }
    }

    // 按优先级判断类型：files > image > text（现有逻辑）
    let result: anyhow::Result<()> = (|| { /* ... */ })();
    // ... 现有后续
}
```

**设计要点**：
- 用 `&[&str]` 数组 + `#[cfg]` 逐元素门控——当前平台只编译对应的常量到数组里，其他平台的字符串不进二进制
- `for hint in CONCEALED_HINTS` 线性探测——密码管理器通常只标 1 个 hint，循环最多 1 次命中即返回，热路径开销可忽略
- `handle.has(ContentFormat::Other(...))` 走各平台原生 API（macOS `availableTypeFromArray` / Windows `register_format` / X11 `get_buffer` 探测 / Wayland MIME 比对）

### 2.2 为什么用 `ContentFormat::Other` 而非平台原生 API

| 方案 | 评估 | 结论 |
|---|---|---|
| **`handle.has(ContentFormat::Other(String))`**（选用） | 复用现有 `ClipboardHandle` 抽象，零新依赖。clipboard-rs 0.3.4+ 四个平台后端（macos.rs / win.rs / x11.rs / wayland.rs）的 `has()` 都实现了 `Other` 分支，支持任意类型字符串检测：macOS 走 `availableTypeFromArray`、Windows 走 `clipboard_win::register_format`、X11/Wayland 走 MIME 名比对。改动：watcher.rs ~20 行（含 `#[cfg]` + 平台常量数组） | ✅ 最小改动 + 跨平台统一 |
| objc2-app-kit / windows-rs / x11rb 直接读 | 完全可控，但 `crates/clipboard` 当前只依赖 clipboard-rs 一个跨平台抽象，加任一平台原生 crate 都要改 Cargo.toml + 平台 cfg 分支 | ❌ 过度工程 |
| `handle.available_formats().iter().any(\|t\| t == HINT)` | 同样零依赖，但 `available_formats()` 返回 `Vec<String>`，热路径上比 `has()` 多一次 Vec 分配 | ❌ 不如 has 轻量 |

### 2.3 常量位置

`CONCEALED_HINTS` 数组在 watcher.rs 本地定义（不跨 crate 引用 `vault/autotype/clipboard.rs` 的同名常量）——两者语义虽同（macOS ConcealedType）但分属不同 crate（clipboard vs desktop/vault），跨 crate 引用会增加耦合。Windows/Linux 的 hint 常量只在 clipboard crate 用到，本地定义最清晰。如果后续多处共用，再考虑提到 `octopus-infra::consts`。

### 2.4 octopus autotype 兼容

octopus 自己的 vault autotype 复制密码时（macOS）也写 ConcealedType（`vault/autotype/clipboard.rs::copy_concealed`），调用方目前手动调 `clipboard.suppress_next()` 防止 watcher 记录（3 处：autotype.rs:134-137 / 335-336 / 369）。

**本改动不改 autotype 调用方**——`suppress_next` 机制保留，原因：

1. **autotype 仅 macOS 实现**：Windows / Linux 的 autotype 尚未实现（`autotype/` 下只有 `macos.rs`），concealed hint 检测虽已跨平台但 autotype 路径不涉及
2. **双重保险**：macOS 上即使 `suppress_next` 因任何原因失效（理论上不会），新增的 ConcealedType 检测也能兜底跳过
3. **最小改动**：不动现有 3 处 `suppress_next` 调用，降低回归风险

watcher 的 `on_clipboard_change` 回调顺序：
```
1. check_and_clear_suppress() → 命中 return（octopus autotype 走这里）
2. is_recording_enabled() gate
3. (on_change)() → enqueue → worker → handle_clipboard_change
   → 新增：ConcealedType 检测 → 命中 return（第三方密码管理器走这里）
   → files / image / text 分支
```

## 3. 影响面

| 场景 | 当前行为 | 改动后 |
|---|---|---|
| macOS 1Password 复制密码 | ❌ 入库（明文 + FTS 索引） | ✅ 静默跳过（`org.nspasteboard.ConcealedType`） |
| macOS Bitwarden / iCloud Keychain | ❌ 入库 | ✅ 静默跳过 |
| macOS Maccy（遵守 nspasteboard 约定）复制 | ❌ 入库 | ✅ 静默跳过 |
| Windows 1Password / KeePass 复制密码 | ❌ 入库 | ✅ 静默跳过（`ExcludeClipboardContentFromMonitorProcessing`） |
| Linux KeePassXC 复制密码 | ❌ 入库 | ✅ 静默跳过（`x-kde-passwordManagerHint`） |
| 普通文本复制（无 concealed hint） | 入库 | 入库（无影响） |
| 图片 / 文件复制 | 入库 | 入库（无影响） |
| octopus autotype 复制密码（macOS） | suppress_next 跳过 | suppress_next 跳过（双保险） |

**平台覆盖说明**：
- **macOS**：1Password / Bitwarden / iCloud Keychain / KeePassXC / Maccy 等普遍遵守 nspasteboard.org 约定
- **Windows**：`ExcludeClipboardContentFromMonitorProcessing` 是 MS 官方 clipboard format，1Password / Bitwarden / KeePass / EnPass 等写入此 format 通知 Windows 剪贴板历史（Win10+ Clipboard History）跳过；octopus 复用同一 format 名检测
- **Linux**：`x-kde-passwordManagerHint` 起源于 KDE/KeePassXC，Wayland 下作 MIME 名传递；GNOME 无官方等价物但 KeePassXC 在 GNOME 也写此 MIME，事实覆盖。其他密码管理器（1Password Linux beta 等）跟进情况未统一，此 MIME 是当前最佳-effort 检测

## 4. 不变量

1. **concealed hint 命中 → 不入库、不 emit `clipboard://changed`、不留任何痕迹**
2. **各平台只编译对应的 hint 常量**（`#[cfg]` 门控，其他平台的字符串不进二进制）
3. **octopus autotype 的 suppress_next 机制不变**（macOS 双重保险；Win/Linux autotype 未实现）
4. **检测点在 files / image / text 分支之前**（避免误入 text 分支入库后再过滤）

## 5. 测试

### 5.1 单测局限

clipboard-rs 的 `has()` 走真 NSPasteboard（`ClipboardContext` 持 `Retained<NSPasteboard>`，私有字段），单测难以 mock `has(ContentFormat::Other(...))` 的返回值。现有 watcher.rs 的单测（如 `test_handle_clipboard_change_*`）用临时 in-memory DB + 真 `ClipboardContext`，但构造 ConcealedType 需要写真 pasteboard。

### 5.2 集成测试方案

加一个 `#[cfg(test)] #[cfg(target_os = "macos")]` 测试：

```rust
#[test]
#[cfg(target_os = "macos")]
fn test_concealed_type_skipped() {
    // 1. 用 ClipboardContext::new() 拿真 pasteboard
    // 2. set_text("fake-password") + 手动 set ConcealedType 标记
    //    （需要 objc2-app-kit，或复用 vault/autotype 的 copy_concealed 逻辑）
    // 3. 调 handle_clipboard_change
    // 4. 查 DB 确认无 "fake-password" 条目
}
```

**实现挑战**：在 clipboard crate（不依赖 objc2-app-kit）里设 ConcealedType 标记需要绕路。两个选项：

- **选项 A**：测试放 `crates/desktop`（依赖 objc2-app-kit + clipboard），复用 `vault/autotype/clipboard.rs::copy_concealed` 设标记，调 `handle_clipboard_change` 验证
- **选项 B**：watcher.rs 内加 `#[cfg(test)]` helper 用 clipboard-rs 的 `set_text` + 手动 `ClipboardContext` 内部 API 设 ConcealedType（clipboard-rs 不暴露任意类型写入 API，此路不通）

**推荐选项 A**——测试放 desktop crate，复用现有 concealed 写入工具。但要注意 `copy_concealed` 走 suppress_next 路径会干扰——测试时需 bypass suppress（直接调 `copy_concealed` 后清 suppress flag，或绕过 autotype 层直接写 pasteboard）。

### 5.3 回归测试

确保现有 `test_handle_clipboard_change_text` / `test_handle_clipboard_change_image` 等测试仍通过（无 ConcealedType 标记时行为不变）。

## 6. YAGNI 边界

明确**不做**：

| 项 | 理由 |
|---|---|
| 占位条目（itemType='concealed'，显示「🔒 密码保护内容」） | YAGNI——静默跳过已满足核心诉求（防泄露）；占位条目需改 schema + FTS5 适配 + 前端渲染分支，过度工程化。如果后续用户反馈「想知道刚复制过密码」，再加 |
| 配置项开关（允许用户关闭 concealed hint 跳过） | 密码安全是默认行为，不应暴露用户可关闭的选项（用户误关会导致密码泄露） |
| 常量提到 infra::consts | 当前只有 watcher.rs（clipboard crate）和 vault/autotype/clipboard.rs（desktop crate）两处用 macOS 同名常量，跨 crate 引用增加耦合；Win/Linux hint 只在 clipboard crate 用；本地定义更清晰 |

## 7. 后续 follow-up（非本 spec 范围）

- **clipboard-rs 升级**：如果 clipboard-rs 未来版本原生支持 concealed 检测（如新增 `ContentFormat::Concealed`），可迁移到原生 API
- **Linux GNOME 官方约定**：当前用 `x-kde-passwordManagerHint` 是 best-effort（KeePassXC 事实标准）；若 GNOME/FreeDesktop 后续推出官方 freedesktop.org hint 规范，跟进

## 8. 参考实现

**平台约定文档**：
- **macOS** [nspasteboard.org](https://nspasteboard.org/)——ConcealedType / TransientType / AutoGeneratedType 等社区标准
- **Windows** `ExcludeClipboardContentFromMonitorProcessing`——MS 官方 clipboard format 名，密码管理器写入此 format 通知 clipboard history 跳过（Win10+ Clipboard History / 第三方剪贴板工具普遍尊重此 format）
- **Linux** `x-kde-passwordManagerHint`——KeePassXC 起源的事实约定，X11 作 atom 名、Wayland 作 MIME 名

**竞品参考**：Maccy（macOS 开源剪贴板管理器）默认跳过 ConcealedType；CopyQ 跨平台默认跳过三平台 hint；Raycast 剪贴板历史同样跳过

**现有 octopus concealed 写入**：`crates/desktop/src/vault/autotype/clipboard.rs:23-24`（`CONCEALED_TYPE` 常量 + `copy_concealed` 实现，macOS autotype 复制密码时写入）

**clipboard-rs Other 分支实现**（均支持任意类型字符串检测）：
- macOS：`~/.cargo/registry/.../clipboard-rs-0.3.4/src/platform/macos.rs`（`availableTypeFromArray(&[NSString])`）
- Windows：`.../platform/win.rs:144`（`clipboard_win::register_format(format).get()`）
- X11：`.../platform/x11.rs:480`（`get_buffer(format_name)` 探测）
- Wayland：`.../platform/wayland.rs:79`（MIME 名比对）
