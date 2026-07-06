# 滚屏截图性能优化设计规格

**日期**：2026-07-06  
**作者**：Antigravity  
**范围**：优化 Windows 和 Linux 下 30ms 滚屏截图热路径的系统开销（CPU、内存和 I/O），对齐 macOS 的局部与轻量化捕获能力。

---

## 1. 痛点分析与优化方案

### 1.1 避免全屏大内存双重拷贝
- **痛点**：在截取当前帧并裁剪时，原 `crop_region_rgba` 调用了 `.clone()` 复制整张全屏 RGBA 字节（约 33MB），造成极大的内存吞吐（每秒 ~1.1GB/s 拷贝）。
- **设计**：新增 `crop_region_rgba_direct` 函数。直接接收只读的 `&[u8]` Slice 引用。只分配裁剪后的目标选区（例如 400x300 选区仅需 480KB），使用 `copy_from_slice` 逐行拷贝，绕过任何全屏大缓存克隆。内存占用与分配次数减少 98% 以上。

### 1.2 避免多屏幕冗余截图与开销
- **痛点**：多屏环境下，每 30ms 都会对所有显示器做一次完整截图（`capture_all_monitors()`），之后丢弃无关的显示器图像。这大大拉高了图形 API 时间以及内存负担。
- **设计**：新增 `capture_single_monitor(mon_x, mon_y)` 接口，接收指定的物理显示器坐标。截屏时使用 `xcap` 定位匹配的显示器，**仅捕获这一个显示器的画面**，避免其他无用显示器的图像捕获和数据分配。

### 1.3 抑制热路径日志泛滥
- **痛点**：在高频 30ms 热路径中，`capture_all_monitors` 每秒输出过百条 `log::info!` 日志，抢占互斥锁并导致 I/O 频繁写入。
- **设计**：在截帧循环和底层 `capture_single_monitor` 中，将不必要的 `log::info!` 调整为 `log::debug!` / `log::trace!`。使得 Release 编译版本自动过滤并静音热路径日志。

---

## 2. 接口设计

### 2.1 `octopus-capx` 暴露的新接口

在 `crates/capx/src/capture.rs` 中：
```rust
/// 仅截取指定坐标位置的单个显示器，避免多屏冗余捕获与内存分配。
pub fn capture_single_monitor(mon_x: i32, mon_y: i32) -> Result<ScreenCapture>;

/// 从只读的 RGBA 像素 Slice 中直接裁剪矩形区域，返回 `RgbaImage`（零全屏克隆）。
pub fn crop_region_rgba_direct(
    full_width: u32,
    full_height: u32,
    rgba_bytes: &[u8],
    x: u32,
    y: u32,
    w: u32,
    h: u32,
) -> Result<::image::RgbaImage>;
```

### 2.2 `screenshot_commands.rs` 调用变更

在 `screenshot_commands.rs` 的非 macOS 滚动截帧热路径中：
1. 替换 `capture_all_monitors()` 为 `capture_single_monitor(mon_phys_x, mon_phys_y)`。
2. 替换 `crop_region_rgba(&full, ...)` 为 `crop_region_rgba_direct(full.width, full.height, &full.rgba_bytes, ...)`。
