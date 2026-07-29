# 首次启动权限引导页设计

**日期**：2026-07-29
**分支**：`feat/package-macos-dmg`
**状态**：🔜 实施中

## 背景

macOS 三类核心权限（麦克风 / 辅助功能 / 屏幕录制）原来在启动时各自触发系统弹窗，导致**两个系统对话框同时弹出**（cpal 触发麦克风 + 代码主动触发辅助功能），用户体验混乱。

辅助功能权限此前**从未主动申请**——3 处 `AXIsProcessTrustedWithOptions(null)` 只静默检查，导致截图 / 返回焦点 / autotype / keystroke 等 AX 依赖功能未授权时静默失败（`d4aea0ab` 已修 FFI 但触发时机需优化）。

## 目标

首次启动（`onboarding_completed == false`）弹独立引导窗口，展示 3 个权限卡片，用户逐一授权后点「完成」关闭，写 DB flag 不再弹。非首次启动不重复弹。

## 功能范围

| 项 | 说明 |
|---|---|
| 首次启动检测 | `AppConfig.onboarding_completed: bool`（默认 false），setup 时检查 |
| 引导窗口 | 独立 `onboarding_window`（仿 `settings_window`），Regular activation policy（Dock 显示） |
| 3 权限卡片 | 麦克风 / 辅助功能 / 屏幕录制，各显示状态 + 申请/打开系统设置按钮 |
| 完成按钮 | 写 `onboarding_completed = true` + 关窗；允许跳过（不强制全 granted） |
| 二次启动 | flag 为 true → 不弹引导页，正常启动流程 |

## 架构

### 启动流程变更（`main.rs` setup hook）

```
setup {
    load config
    if !config.onboarding_completed {
        open_onboarding(app)       // 弹引导页（Regular）
        // 延迟 recorder.open 到引导页完成（避免麦克风弹窗叠加）
    } else {
        recorder.open()             // 正常启动（cpal 触发麦克风 TCC）
    }
    // ... 其他初始化
}
```

引导页 `complete_onboarding` 命令：
1. `set_config(onboarding_completed = true)` save DB
2. 如果首次启动（recorder 未 open）→ 现在执行 `recorder.open()`
3. 关窗 + restore Accessory

### 权限命令（`record_commands.rs` 新增 4 个）

| 命令 | 实现 | 返回 |
|---|---|---|
| `check_microphone_permission` | cpal probe：try `default_host().default_input_device()` + `build_input_stream`（立即 drop） | Granted/Denied |
| `request_microphone_permission` | 同 probe（open stream 即触发 TCC 弹窗） | Granted/Denied |
| `check_accessibility_permission` | `app_context::ffi::is_accessibility_trusted()` | Granted/Denied |
| `request_accessibility_permission` | `app_context::ffi::prompt_accessibility_permission()` | Granted/Denied |

**注意**：macOS 麦克风无「not-determined」API 区分——cpal build 成功 = granted，失败 = denied。AX 同理（bool → Granted/Denied，无 not-determined）。仅屏幕录制有 not-determined（helper 的 `--check-permission` 返回三态）。

### `PrivacySection` 扩展（`protocol.rs`）

```rust
pub enum PrivacySection { ScreenCapture, Microphone, Accessibility }
```
- `Accessibility` → `x-apple.systempreferences:com.apple.preference.security?Privacy_Assistive`

### 前端引导页（`onboarding.html` + entry）

独立窗口，3 个 `PermissionCard` 子组件（parametrize by permission type，复用 PermissionGate 的 refresh/request/openSettings 模式）。

每卡片：
- 图标（Mic / Accessibility / Monitor）
- 标题 + 描述
- 状态徽章（granted=绿 / denied=红 / not-determined=琥珀）
- 按钮：not-determined → 申请权限；denied → 打开系统设置

底部「完成」按钮（3 项全 granted 高亮，但允许跳过）。

## 不变量

- `onboarding_completed` 一旦 true 永不回退（重置需清 DB）
- 首次启动 + 引导页未完成期间不 open recorder（避免麦克风弹窗）
- 引导页窗口单例（已开则 set_focus）

## 降级路径

- 引导页加载失败 → fallback 走原启动流程（recorder.open + AX prompt 直接调）
- cpal probe 失败 → 麦克风卡片显示 denied，引导用户到系统设置
- 引导页 skip → `onboarding_completed = true`，权限留给用户后续在系统设置手动开
