# 录音模式（talk/PTT + hands-free）— 设计规格

> **注记（2026-08-01）**：本 spec 描述的 talk/hands-free **交互设计仍有效**（PTT 按住说话、
> hands-free 常驻 + 静音超时等行为不变）。但「instant_overlay 独立窗口」部分已被
> [2026-08-01-merge-asr-windows-design.md](2026-08-01-merge-asr-windows-design.md) 取代——
> instant_overlay 窗口已删除，instant 指示卡合并进 result_window 单实例（record-mode 切换视图）。
> 下文涉及 `instant_overlay` 独立窗口 / label / 400×80 尺寸的描述以此注记为准（已过时）。

- **日期**：2026-07-31
- **类型**：新功能（新交互模式 + 新窗口 + coordinator 扩展）
- **范围**：新增talk (PTT) + 免提两种录音模式，与现有 toggle 模式并存
- **动机**：toggle 模式适合长思考（说很长一段话），但短句即时交流场景（聊天、命令）体验不佳。talk (PTT)（参考 SayIt/Handy）对短句更友好——按住说话、松开即粘。hands-free 模式适合长会议/口述——按一次开始常驻录音、VAD 自动切段。

## 三模式并存

| 模式 | 说明 | 触发 | 浮窗 | 适用场景 |
|---|---|---|---|---|
| **toggle**（现有，不变） | 按一次开始/再按一次停止 | `asr_shortcut`（Alt+Shift+A） | result_window（CM6 可编辑） | 长思考、要编辑 |
| **talk (PTT)**（新增） | 按住说话、松开识别+粘贴 | PTT 键 keydown/keyup | instant 浮窗（只读指示） | 短句即时交流 |
| **hands-free**（新增） | 按一次开始常驻录音、VAD 自动切段、再按停止 | PTT 键 toggle | instant 浮窗（只读指示） | 长会议、口述 |

### 长按与免提的区分

两种方案（首版选其一）：
- **方案 A（时长区分，推荐）**：同一 PTT 键（AltRight），按下后判断——松开时若按住 < 300ms → 免提 toggle；松开时若按住 ≥ 300ms → PTT 停止识别。参考 SayIt 的 `onPTTDown/Up` + 免提切换逻辑。
- **方案 B（独立配置）**：两个独立快捷键（PTT 键 + 免提键），各自 seed 默认。设置 UI 后续实现。

首版用**方案 A**——一个键两种语义，用户体验更简洁。

### PTT 键

参考 SayIt，支持右侧修饰键（单键操作，不干扰打字）：

| 键 | 说明 |
|---|---|
| 右 Option/Alt（`AltRight`） | **Octopus 默认**——macOS 冲突最少 |
| 右 Cmd（`MetaRight`） | |
| 右 Ctrl（`ControlRight`） | |
| 右 Shift（`ShiftRight`） | SayIt PTT 默认 |

扩展键（后续迭代）：CapsLock / Space / F1-F12 / 鼠标侧键。

## 首版范围

- ✅ talk (PTT)模式：`handy-keys` crate keydown/keyup
- ✅ instant 指示浮窗（只读）
- ✅ seed 默认值写死（`record_mode = "toggle"`，`ptt_key = "AltRight"`）
- ✅ coordinator 新增 instant 路径
- ✅ 复用现有引擎/润色/粘贴
- ✅ hands-free 模式（已实现——单键三模式短按触发，静音 10s 超时，详见 single-key-three-modes-design.md）
- ✅ 设置 UI / 用户自定义键（已实现——asr-key-selector dropdown 5 选 1，详见 asr-key-selector-design.md）
- ✅ PTT/免提时长区分（已实现——单键三模式按键时长 + 双击检测，TAP_TIMEOUT_MS=260）

## 核心决策

| 决策点 | 选择 | 理由 |
|---|---|---|
| PTT 实现 | `handy-keys` crate（跨平台 keydown/keyup） | Handy 同款，macOS 用 CGEventTap 绕过 Tauri 插件 keyup 限制 |
| 默认 PTT 键 | `AltRight`（右 Option） | macOS 冲突最少，跟 SayIt hands-free 模式默认一致 |
| 润色 | 跟 toggle 一样（polish_mode 决定） | 用户确认 |
| 引擎 | 复用当前激活引擎 | 非流式走 VAD 伪流式分段 |
| 浮窗 | 只读指示浮窗（非 CM6 可编辑窗口） | 显示状态 + 实时文字，粘贴后消失 |
| 浮窗位置 | 屏幕底部居中 | 参考 Handy/SayIt |

## 现状（toggle 模式）

### 录音流程
```
用户按 asr_shortcut
  → Command::Toggle（Idle 分支）
    → save_frontmost_pid()
    → 两阶段 prepare：emit("prepare-record") → 前端回推 selection → StartRecording
      → begin_recording → audio.start() + show_result + tray 置 Recording
    → 200ms 看门狗 → FallbackStart
  → 用户再按 asr_shortcut
    → Command::Toggle（活跃态）→ handle_toggle → 停录 → finalize_after_stop
      → show_result(最终文本) → start_final_polish_or_paste → do_paste → PasteDone → Idle
```

### 关键约束
- 两阶段 prepare：给前端机会回推 selection（选中替换）
- 结果窗口（result_window）：CM6 可编辑，支持流式编辑、工具栏、翻译
- 全局快捷键：`tauri_plugin_global_shortcut`，macOS **只触发 Pressed**（不支持 keyup）

## 架构（instant 模式）

### talk (PTT)流程
```
用户按住 PTT 键（AltRight keydown）
  → save_frontmost_pid()
  → show instant 浮窗（"正在聆听…" + 波形）
  → begin_recording（跳过两阶段 prepare，直接开始）
    → audio.start() + 引擎会话（同 toggle）
    → 实时文字 → update instant 浮窗

用户松开 PTT 键（AltRight keyup）
  → 停录（同 handle_toggle）
  → finalize_after_stop
    → 跳过 show_result（不弹 result_window）
    → start_final_polish_or_paste
      → 浮窗显示"润色中…"（如有润色）
    → do_paste（粘贴 + DB 入库）
      → 浮窗显示最终文字 → 500ms 后 hide
    → PasteDone → Idle
```

### instant vs toggle 流程对比

| 步骤 | toggle | talk (PTT) |
|---|---|---|
| 触发开始 | global_shortcut(Pressed) | handy-keys keydown |
| 触发停止 | global_shortcut(Pressed) 再按 | handy-keys keyup |
| 两阶段 prepare | ✅（200ms 看门狗 + selection 回推） | ❌ 跳过 |
| 结果窗口 | result_window（CM6 可编辑） | instant 浮窗（只读指示） |
| 润色 | polish_mode 决定 | 同 toggle |
| 引擎 | 当前激活引擎 | 同 toggle |
| 粘贴 | do_paste | 同 toggle（复用 cached pid） |
| 选中替换 | ✅（selection 回推） | ❌（跳过 prepare） |

## 改动点

### 1. PTT 按键监听（新模块 `platform/ptt.rs`）

引入 `handy-keys` crate，新建 `crates/desktop/src/platform/ptt.rs`：

```rust
pub fn register_ptt(app: &AppHandle, key: &str) -> Result<()>;
pub fn unregister_ptt(app: &AppHandle) -> Result<()>;
```

- `HotkeyManager` 独立线程（同 Handy `handy_keys.rs` manager_thread 模式）
- keydown callback → `save_frontmost_pid()` + coordinator 发 `Command::InstantStart`
- keyup callback → coordinator 发 `Command::InstantStop`
- 线程安全：HotkeyManager 单线程持有，命令通过 mpsc channel 传递（同 Handy）

### 2. 配置（seed 默认值，首版无 UI）

`AppConfig` 加字段：
```rust
/// 录音模式: "toggle"（默认）| "instant"（PTT）
#[serde(default = "default_record_mode")]
pub record_mode: String,

/// instant 模式 PTT 键（默认 "AltRight"）
#[serde(default = "default_ptt_key")]
pub ptt_key: String,
```

db.sql app_config seed：
```sql
('record_mode', 'toggle', '录音模式 toggle/instant'),
('ptt_key', 'AltRight', 'PTT 按键（右侧修饰键）'),
```

### 3. coordinator 新增 Command

```rust
/// PTT keydown → 开始录音（跳过两阶段 prepare）
InstantStart,
/// PTT keyup → 停止录音 + 识别 + 粘贴
InstantStop,
```

- `InstantStart`（Idle 态）：`save_frontmost_pid()` + `begin_recording(instant=true)`（跳过 prepare + show instant 浮窗）
- `InstantStop`（活跃态）：同 `handle_toggle` 停录 + `finalize_after_stop(instant=true)`

### 4. begin_recording / finalize_after_stop / do_paste 加 instant 分支

通过 `RecordType` 扩展或 `instant: bool` flag 区分：
- `begin_recording`：instant 时跳过 `show_result`，改 show instant 浮窗
- `finalize_after_stop`：instant 时跳过 `show_result`（不弹 result_window）
- `do_paste`：instant 时跳过 `show_result`，改 instant 浮窗显示最终文字 + 延迟 hide

### 5. Instant 指示浮窗（新窗口）

**窗口属性**（用 `build_float_window` + `FloatWindowSpec`）：
- label: `"instant_overlay"`
- url: `"instant-overlay.html"`
- 尺寸：compact 280×48px（录音中）/ expanded 400×120px（有文字时）
- 透明、无边框、always_on_top、skip_taskbar
- **不抢焦点**（`focused: Some(false)`）
- 位置：屏幕底部居中

**前端页面**（`pages/InstantOverlay/index.tsx`）：
- 状态：`listening`（波形 + "正在聆听…"）→ `processing`（spinner + "识别中…"）→ `polishing`（spinner + "润色中…"）→ `done`（最终文字）→ hide
- 实时文字：流式引擎 partial 更新（同 toggle 的 update_result 事件，但写入浮窗）
- 波形：简易 CSS 动画

**事件**：
- `emit_to("instant_overlay", "instant-state", { state, text })` — 后端控制浮窗

### 6. 快捷键注册

`setup.rs::register_shortcuts()`：
- `record_mode == "toggle"` → 现有 `register_shortcut(asr_shortcut)`（不变）
- `record_mode == "instant"` → `ptt::register_ptt(app, ptt_key)`

## 权限

`handy-keys` macOS 底层用 CGEventTap，需要**输入监控**（Input Monitoring）权限。
- 项目已有**辅助功能**（AX）权限引导
- instant 模式需新增**输入监控**权限检测（首次启用时引导，或 onboarding 加卡片）
- 权限缺失时：instant 不可用，提示用户授权或使用 toggle

## 不变量

1. **toggle 模式完全不变**
2. **instant 复用引擎/润色/粘贴**
3. **PasteDone 收尾不能省**
4. **frontmost pid 缓存**：PTT keydown 时 `save_frontmost_pid`
5. **busy 保护**：PTT 松开时若 vad 段还在跑（WaitingCompletion），需等 drain 完

## 边界处理

| 场景 | 处理 |
|---|---|
| PTT 按下时已在录音 | 忽略 |
| PTT 松开时在 WaitingCompletion | 正常走 finalize（等 drain 完再 paste） |
| PTT 松开时在 Polishing/Pasting | 忽略 |
| instant 录音中按 toggle 快捷键 | 忽略（混合模式不支持） |
| 录音为空（没说话就松开） | 不粘贴、不报错，hide 浮窗回 Idle |
| handy-keys 权限缺失 | instant 不可用，提示授权或切 toggle |

## 文件清单

| 文件 | 操作 |
|---|---|
| `Cargo.toml`（desktop） | 加 `handy-keys` 依赖 |
| `crates/desktop/src/platform/ptt.rs` | **新增**：PTT 按键监听 |
| `crates/infra/src/config.rs` | 加 `record_mode` + `ptt_key` 字段 |
| `crates/infra/src/db.sql` | app_config seed 加 record_mode + ptt_key |
| `crates/desktop/src/engine/coordinator/mod.rs` | 加 `InstantStart`/`InstantStop` 变体 |
| `crates/desktop/src/engine/coordinator/session.rs` | `begin_recording` instant 分支 |
| `crates/desktop/src/engine/coordinator/lifecycle.rs` | `finalize_after_stop` instant 分支 |
| `crates/desktop/src/engine/coordinator/paste.rs` | `do_paste` instant 分支 |
| `crates/desktop/src/ui/instant_overlay.rs` | **新增**：instant 浮窗 |
| `crates/desktop/frontend/src/entries/instant-overlay-main.tsx` | **新增**：浮窗入口 |
| `crates/desktop/frontend/src/pages/InstantOverlay/index.tsx` | **新增**：浮窗页面 |
| `crates/desktop/frontend/instant-overlay.html` | **新增**：浮窗 HTML |
| `crates/desktop/src/core/setup.rs` | register_shortcuts 按 record_mode 分流 |
| `crates/desktop/src/core/invoke_handler.rs` | 注册新命令（如需） |
| `docs/architecture.md` | instant 模式说明 |
