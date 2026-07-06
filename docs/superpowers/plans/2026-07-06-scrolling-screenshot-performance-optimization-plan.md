# 滚屏截图性能优化实施计划

**日期**：2026-07-06  
**作者**：Antigravity  
**范围**：修改 `crates/capx/src/capture.rs` 与 `crates/desktop/src/screenshot_commands.rs` 以提高 Windows/Linux 截帧性能。

---

## 任务分解

### 任务 1：在 `crates/capx/src/capture.rs` 中新增高性能接口
- **变更点**：
  - 新建 `capture_single_monitor` 接口，仅遍历和捕获匹配给定的 `(mon_x, mon_y)` 的单个显示器。如果找不到则降级捕获主屏幕。
  - 新建 `crop_region_rgba_direct` 接口，入参 `rgba_bytes: &[u8]` 传入只读 Slice，做行优先越界安全裁剪，返回包含裁剪数据所有权的 `RgbaImage`。
  - 将 `capture_all_monitors` 和新增函数的 `log::info!` 降低为 `log::debug!`，移除热路径高频输出。

---

### 任务 2：重构 `crates/desktop/src/screenshot_commands.rs` 截帧热循环
- **变更点**：
  - 在 `screenshot_commands.rs` 的滚动截帧热路径中（包括首帧获取以及 `SCROLL_RECORDING` 循环截帧两处）：
    - 在非 macOS 分支中，用 `capture_single_monitor(mon_phys_x, mon_phys_y)` 替换 `capture_all_monitors()`。
    - 用 `crop_region_rgba_direct(full.width, full.height, &full.rgba_bytes, ...)` 替换 `crop_region_rgba(...)`，传入只读 Slice，避免克隆。

---

## 验证与测试命令

- 编译桌面应用（本地 macOS）：
  `cargo check -p octopus-desktop --features embedded`
- 语法与静态检查 Windows 目标：
  `cargo check --target x86_64-pc-windows-msvc -p octopus-desktop`
- 语法与静态检查 Linux 目标：
  `cargo check --target x86_64-unknown-linux-gnu -p octopus-desktop`
- 运行桌面端测试（手动触发滚动截屏）：
  `cargo run -p octopus-desktop --features embedded`
