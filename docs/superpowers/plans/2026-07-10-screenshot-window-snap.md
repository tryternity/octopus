# 截图智能窗口识别（区域截图自动吸附）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 区域截图模式加入「鼠标悬停高亮候选窗口 + 单击即选整窗」，减少手动框选；macOS 先行，零额外权限（复用已有屏幕录制权限）。

**Architecture:** 新增 `crates/capx/src/window_detect/`（`WindowDetector` trait + `Granularity`/`SnapRect` 类型 + 命中纯函数 `pick_top_window` + macOS `CGWindowList` impl）。后端一条同步命令 `hit_test_window(gx, gy)`。前端 `Screenshot/index.tsx` 在现有 mouse 状态机上加：idle 悬停节流查询 + Canvas 高亮绘制 + onMouseUp 单击分支选中吸附候选 + Cmd 禁用。吸附只改"选区怎么来"，下游标注/OCR/贴图/复制零改动。

**Tech Stack:** Rust（core-graphics 0.24 + core-foundation 0.10，capx 已依赖）、Tauri 2 `#[tauri::command]`、React 19 + Canvas 2D、`#[cfg(test)]` 纯函数单测。

**对应 spec:** `docs/superpowers/specs/2026-07-10-screenshot-window-snap-design.md`

**v1 范围（本计划）**：仅 `Granularity::Window`（`CGWindowList`，零额外权限）。`Element` 变体预留，macOS impl 返回 `None`（v2 AX 仅浏览器，后续计划）。

---

## File Structure

| 文件 | 责任 | 动作 |
|---|---|---|
| `crates/capx/src/window_detect/mod.rs` | `WindowDetector` trait + `Granularity`/`SnapRect`/`WinInfo`/`MonitorRect` 类型 + 命中纯函数 `pick_top_window` + 单测 | 新建 |
| `crates/capx/src/window_detect/macos.rs` | `MacOsDetector`：`CGWindowListCopyWindowInfo` FFI 解析 + `CGDisplay` 找含点显示器 + 调 `pick_top_window` | 新建 |
| `crates/capx/src/lib.rs` | 导出 `pub mod window_detect` | 修改 |
| `crates/desktop/src/screenshot_commands.rs` | `hit_test_window` 命令（薄包装，调 capx） | 修改 |
| `crates/desktop/src/main.rs` | `generate_handler!` 注册 `hit_test_window` | 修改 |
| `crates/desktop/frontend/src/pages/Screenshot/index.tsx` | winOrigin 缓存 + idle 悬停节流查询 + Canvas 高亮 + onMouseUp 单击选中 + Cmd 禁用 | 修改 |
| `docs/architecture.md`、`docs/features/screenshot.md` | 窗口识别模块说明 | 修改 |

**复用现有**：`CGWindowListCopyWindowInfo` extern + `CFDictionary` 解析模式（`crates/capx/src/capture.rs:288-364` `find_window_id_by_pid`）、geometry.rs 的纯函数 + `#[cfg(test)]` 测试范式、main.rs:255-267 的 `generate_handler!` 列表、前端 `getCurrentWindow()`/`invoke`（Screenshot/index.tsx:2-3）。

**worktree 提醒**：cargo 必须用 `--manifest-path /Users/wudarui/workspace/agent/octopus/.claude/worktrees/asr-wordbook/Cargo.toml`（或在该 cwd 下跑）；前端 tsc 用 `crates/desktop/frontend/node_modules/.bin/tsc -p crates/desktop/frontend/tsconfig.json --noEmit`。

---

### Task 1: 命中算法纯函数 `pick_top_window` + 类型（TDD）

**Files:**
- Create: `crates/capx/src/window_detect/mod.rs`

- [ ] **Step 1: 写失败的单测（先于实现）**

创建 `crates/capx/src/window_detect/mod.rs`，先只放测试与签名（实现体留 `todo!()` 或空，确保编译过、测试失败）：

```rust
//! 截图智能窗口识别——区域截图吸附窗口边界。
//! v1：仅 Granularity::Window（CGWindowList，零额外权限）；Element(v2 AX 仅浏览器) 预留。
//!
//! 命中算法 pick_top_window 是纯函数（不调 FFI），便于单测；FFI 解析在 macos.rs。

use serde::Serialize;

/// 吸附粒度。Window=整窗（CGWindowList）；Element=UI 元素（v2 AX，仅浏览器 bundle id）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Granularity {
    Window,
    Element,
}

/// 吸附矩形（全局显示逻辑坐标，points；前端用 winOrigin 减回本窗 CSS）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct SnapRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    /// 窗口标题（CGWindowList 的 kCGWindowName，可能为空）。v1 命中算法暂不填，留 None。
    pub title: Option<String>,
}

/// 单个 on-screen 窗口纯数据（从 CGWindowList 字典解析，喂 pick_top_window）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WinInfo {
    pub pid: i32,
    pub layer: i32,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// 显示器矩形（跨屏判定用；全局显示逻辑坐标）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MonitorRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// 窗口识别器 trait（跨平台抽象，仿 PinWindow trait）。
pub trait WindowDetector {
    /// 找鼠标 (gx,gy) 下最上层合格窗口的吸附矩形；无候选返回 None。
    fn hit_test(
        &self,
        gx: f64,
        gy: f64,
        granularity: Granularity,
        monitor: MonitorRect,
    ) -> Option<SnapRect>;
}

/// 命中算法纯函数：从窗口列表找 (gx,gy) 下最上层合格窗口。
///
/// 过滤：① pid == self_pid（自身截图覆盖窗）② layer < 0（桌面/壁纸）
///      ③ 退化 bounds（w 或 h ≤ 0）④ 跨屏（bounds 不完全包含在 monitor 内）。
/// 候选 = bounds 含点；按 layer 降序取最上层，layer 相同则保留数组靠前者
/// （CGWindowList 数组顺序即 z-order，index 越小越上层）。
pub fn pick_top_window(
    windows: &[WinInfo],
    gx: f64,
    gy: f64,
    monitor: MonitorRect,
    self_pid: i32,
) -> Option<SnapRect> {
    todo!("Task 1 Step 3 实现")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn win(pid: i32, layer: i32, x: f64, y: f64, w: f64, h: f64) -> WinInfo {
        WinInfo { pid, layer, x, y, w, h }
    }
    const MON: MonitorRect = MonitorRect { x: 0.0, y: 0.0, w: 1920.0, h: 1080.0 };

    #[test]
    fn picks_higher_layer_at_same_point() {
        // 两窗都含 (300,300)；layer 3（菜单级）应盖过 layer 0
        let ws = [
            win(100, 0, 0.0, 0.0, 1000.0, 800.0),
            win(101, 3, 100.0, 100.0, 500.0, 400.0),
        ];
        let r = pick_top_window(&ws, 300.0, 300.0, MON, 999).unwrap();
        assert_eq!((r.x, r.y, r.w, r.h), (100.0, 100.0, 500.0, 400.0));
    }

    #[test]
    fn same_layer_keeps_array_first_zorder() {
        // 同 layer 0：数组靠前的更上层（CGWindowList z-order）
        let ws = [
            win(101, 0, 100.0, 100.0, 500.0, 400.0), // 前 = 上层
            win(100, 0, 0.0, 0.0, 1000.0, 800.0),
        ];
        let r = pick_top_window(&ws, 300.0, 300.0, MON, 999).unwrap();
        assert_eq!(r.x, 100.0); // 命中数组第一个
    }

    #[test]
    fn skip_self_pid() {
        let ws = [win(999, 0, 0.0, 0.0, 1000.0, 800.0)]; // pid == self_pid
        assert!(pick_top_window(&ws, 500.0, 500.0, MON, 999).is_none());
    }

    #[test]
    fn skip_negative_layer() {
        let ws = [win(100, -1, 0.0, 0.0, 1920.0, 1080.0)]; // 桌面层
        assert!(pick_top_window(&ws, 500.0, 500.0, MON, 999).is_none());
    }

    #[test]
    fn skip_degenerate_bounds() {
        let ws = [
            win(100, 0, 0.0, 0.0, 0.0, 800.0),   // w=0
            win(101, 0, 0.0, 0.0, 800.0, 0.0),   // h=0
        ];
        assert!(pick_top_window(&ws, 1.0, 1.0, MON, 999).is_none());
    }

    #[test]
    fn skip_cross_screen_window() {
        // 窗口横跨出 monitor 右边界（x+w > 1920）→ 跳过
        let ws = [win(100, 0, 1800.0, 0.0, 400.0, 800.0)]; // 右边到 2200 > 1920
        assert!(pick_top_window(&ws, 1850.0, 100.0, MON, 999).is_none());
    }

    #[test]
    fn no_candidate_when_point_off_all_windows() {
        let ws = [win(100, 0, 0.0, 0.0, 500.0, 400.0)];
        assert!(pick_top_window(&ws, 1000.0, 1000.0, MON, 999).is_none());
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --manifest-path crates/capx/Cargo.toml window_detect`
Expected: 7 个测试因 `todo!()` panic 而 FAIL（`not yet implemented`）。

- [ ] **Step 3: 实现 `pick_top_window`**

替换 `todo!()` 为：

```rust
pub fn pick_top_window(
    windows: &[WinInfo],
    gx: f64,
    gy: f64,
    monitor: MonitorRect,
    self_pid: i32,
) -> Option<SnapRect> {
    let mut best: Option<(i32, SnapRect)> = None;
    for win in windows {
        // 过滤
        if win.pid == self_pid { continue; }
        if win.layer < 0 { continue; }
        if win.w <= 0.0 || win.h <= 0.0 { continue; }
        // 跨屏：bounds 必须完全在 monitor 内
        let fully_inside = win.x >= monitor.x
            && win.y >= monitor.y
            && win.x + win.w <= monitor.x + monitor.w
            && win.y + win.h <= monitor.y + monitor.h;
        if !fully_inside { continue; }
        // 命中点
        if !(gx >= win.x && gx < win.x + win.w && gy >= win.y && gy < win.y + win.h) { continue; }
        // layer 降序取最上层；layer 相同保留先出现者（数组顺序 = z-order，前 = 上层）
        let take = match &best {
            None => true,
            Some((bl, _)) => win.layer > *bl, // 严格大于：同 layer 不替换 → 保留先出现
        };
        if take {
            best = Some((win.layer, SnapRect {
                x: win.x, y: win.y, w: win.w, h: win.h, title: None,
            }));
        }
    }
    best.map(|(_, r)| r)
}
```

- [ ] **Step 4: 跑测试确认全绿**

Run: `cargo test --manifest-path crates/capx/Cargo.toml window_detect`
Expected: 7 passed, 0 failed。

- [ ] **Step 5: 提交**

```bash
git add crates/capx/src/window_detect/mod.rs
git commit -m "feat(capx): 窗口命中算法纯函数 pick_top_window + 类型（吸附 v1 Task1）"
```

---

### Task 2: macOS `MacOsDetector`（CGWindowList FFI）

**Files:**
- Create: `crates/capx/src/window_detect/macos.rs`

复用 `capture.rs:288-364` 的 `CGWindowListCopyWindowInfo` extern + `CFDictionary` 解析模式。

- [ ] **Step 1: 写 macos.rs 实现**

```rust
//! macOS WindowDetector：CGWindowListCopyWindowInfo → pick_top_window。
//! v1 仅 Granularity::Window；Element 返回 None（v2 AX 仅浏览器，后续）。

use super::{pick_top_window, Granularity, MonitorRect, SnapRect, WinInfo, WindowDetector};
use core_foundation::array::CFArray;
use core_foundation::base::TCFType;
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGWindowListCopyWindowInfo(
        option: u32,
        relativeToWindow: u32,
    ) -> core_foundation::array::CFArrayRef;
}

pub struct MacOsDetector {
    self_pid: i32,
}

impl Default for MacOsDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl MacOsDetector {
    pub fn new() -> Self {
        Self { self_pid: std::process::id() as i32 }
    }
}

impl WindowDetector for MacOsDetector {
    fn hit_test(
        &self,
        gx: f64,
        gy: f64,
        granularity: Granularity,
        monitor: MonitorRect,
    ) -> Option<SnapRect> {
        // v1：Element 暂不实现（v2 AX 仅浏览器）。统一走 Window。
        let _ = granularity;
        let windows = collect_on_screen_windows()?;
        pick_top_window(&windows, gx, gy, monitor, self.self_pid)
    }
}

/// 调 CGWindowListCopyWindowInfo 解析为 WinInfo 列表（保持 CGWindowList 数组顺序=z-order）。
/// 失败/空返回 None。
fn collect_on_screen_windows() -> Option<Vec<WinInfo>> {
    // kCGWindowListOptionOnScreenOnly = 1 << 0
    let option: u32 = 1 << 0;
    unsafe {
        let array_ref = CGWindowListCopyWindowInfo(option, 0); // kCGNullWindowID = 0
        if array_ref.is_null() { return None; }
        let array = CFArray::<CFDictionary>::wrap_under_create_rule(array_ref);

        let pid_key = CFString::from_static_string("kCGWindowOwnerPID");
        let layer_key = CFString::from_static_string("kCGWindowLayer");
        let bounds_key = CFString::from_static_string("kCGWindowBounds");
        let x_key = CFString::from_static_string("X");
        let y_key = CFString::from_static_string("Y");
        let w_key = CFString::from_static_string("Width");
        let h_key = CFString::from_static_string("Height");

        let mut out: Vec<WinInfo> = Vec::with_capacity(array.len());
        for i in 0..array.len() {
            let dict = match array.get(i) {
                Some(d) => d,
                None => continue,
            };
            let pid = get_i32(dict, &pid_key);
            let layer = get_i32(dict, &layer_key);
            let (x, y, w, h) = match dict.find(bounds_key.as_CFTypeRef()) {
                Some(bv) => {
                    let bd = CFDictionary::<*const std::ffi::c_void, *const std::ffi::c_void>
                        ::wrap_under_get_rule(*bv as *const _);
                    (get_f64(&bd, &x_key), get_f64(&bd, &y_key), get_f64(&bd, &w_key), get_f64(&bd, &h_key))
                }
                None => continue,
            };
            let (pid, layer, x, y, w, h) = match (pid, layer, x, y, w, h) {
                (Some(p), Some(l), Some(x), Some(y), Some(w), Some(h)) => (p, l, x, y, w, h),
                _ => continue,
            };
            out.push(WinInfo { pid, layer, x, y, w, h });
        }
        Some(out)
    }
}

/// 找包含 (gx,gy) 的显示器；找不到（鼠标在屏外，极少）返回兜底超大 rect（不滤跨屏）。
fn monitor_containing(gx: f64, gy: f64) -> MonitorRect {
    use core_graphics::display::CGDisplay;
    let fallback = MonitorRect { x: 0.0, y: 0.0, w: f64::MAX, h: f64::MAX };
    let ids = match CGDisplay::active_displays() {
        Ok(v) => v,
        Err(_) => return fallback,
    };
    for id in ids {
        let b = CGDisplay::new(id).bounds();
        if gx >= b.origin.x
            && gx < b.origin.x + b.size.width
            && gy >= b.origin.y
            && gy < b.origin.y + b.size.height
        {
            return MonitorRect { x: b.origin.x, y: b.origin.y, w: b.size.width, h: b.size.height };
        }
    }
    fallback
}

/// 对外暴露的便利函数：给定全局坐标，自动定 monitor 并命中（供 desktop 命令直接调）。
pub fn hit_test_window_global(gx: f64, gy: f64) -> Option<SnapRect> {
    let monitor = monitor_containing(gx, gy);
    MacOsDetector::new().hit_test(gx, gy, Granularity::Window, monitor)
}

fn get_i32(dict: &CFDictionary, key: &CFString) -> Option<i32> {
    let v = dict.find(key.as_CFTypeRef())?;
    CFNumber::wrap_under_get_rule(*v as *const _).to_i32()
}

fn get_f64(dict: &CFDictionary, key: &CFString) -> Option<f64> {
    let v = dict.find(key.as_CFTypeRef())?;
    let n = CFNumber::wrap_under_get_rule(*v as *const _);
    n.to_i64().map(|i| i as f64).or_else(|| n.to_f64())
}
```

> **API 校验提醒**：`core_graphics::display::CGDisplay::active_displays()` / `CGDisplay::new(id).bounds()` 为 0.24 API。若签名不符，查 `crates/capx/Cargo.toml` 锁定的 core-graphics 版本文档调整；`bounds()` 返回 `CGRect { origin: CGPoint{x,y}, size: CGSize{width,height} }`，坐标系为 Quartz 全局逻辑 points（Y 向下，原点主屏左上），与 `CGWindowList` bounds 同空间。

- [ ] **Step 2: 在 mod.rs 挂载 macos 子模块（条件编译）**

在 `crates/capx/src/window_detect/mod.rs` 末尾（`#[cfg(test)]` 之前）加：

```rust
#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "macos")]
pub use macos::{hit_test_window_global, MacOsDetector};
```

- [ ] **Step 3: 编译确认（macOS）**

Run: `cargo build --manifest-path crates/capx/Cargo.toml`
Expected: 编译通过（FFI unsafe 块无警告错误）。FFI 部分不进单测（隔离）。

- [ ] **Step 4: 提交**

```bash
git add crates/capx/src/window_detect/macos.rs crates/capx/src/window_detect/mod.rs
git commit -m "feat(capx): MacOsDetector CGWindowList FFI 命中（吸附 v1 Task2）"
```

---

### Task 3: capx 导出 window_detect 模块

**Files:**
- Modify: `crates/capx/src/lib.rs`

- [ ] **Step 1: 加模块导出**

读 `crates/capx/src/lib.rs`，在现有 `pub mod` 行（`pub mod capture;` / `pub mod stitch;` 旁）加一行：

```rust
pub mod window_detect;
```

- [ ] **Step 2: 编译确认**

Run: `cargo build --manifest-path crates/capx/Cargo.toml`
Expected: 通过。

- [ ] **Step 3: 提交**

```bash
git add crates/capx/src/lib.rs
git commit -m "feat(capx): 导出 window_detect 模块（吸附 v1 Task3）"
```

---

### Task 4: desktop `hit_test_window` 命令 + 注册

**Files:**
- Modify: `crates/desktop/src/screenshot_commands.rs`
- Modify: `crates/desktop/src/main.rs`（`generate_handler!` 列表，约 255-267 行）

- [ ] **Step 1: 在 screenshot_commands.rs 加命令**

在文件末尾（其他 `#[tauri::command]` 之后）加：

```rust
/// 截图覆盖窗前端 mousemove 调：给全局逻辑坐标，返回最上层合格窗口的吸附矩形（全局逻辑）。
/// 非 macOS 平台返回 None（v1 仅 macOS）。granularity 固定 Window（v2 再加 Element）。
#[tauri::command]
pub fn hit_test_window(gx: f64, gy: f64) -> Option<octopus_capx::window_detect::SnapRect> {
    #[cfg(target_os = "macos")]
    {
        octopus_capx::window_detect::hit_test_window_global(gx, gy)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (gx, gy);
        None
    }
}
```

> 命令是同步（非 async）——CGWindowList 元数据查询微秒级，无需 async；返回 `Option<SnapRect>` 直接序列化（`SnapRect` 已 `#[derive(Serialize)]`）。

- [ ] **Step 2: 在 main.rs 注册命令**

在 `crates/desktop/src/main.rs` 的 `generate_handler!` 列表中（`screenshot_commands::pin_screenshot,` 那一行，约 267 行）后加一行：

```rust
            screenshot_commands::hit_test_window,
```

- [ ] **Step 3: 编译 + 确认命令已注册**

Run: `cargo build --manifest-path crates/desktop/Cargo.toml`
Expected: 通过（`generate_handler!` 宏引用 `hit_test_window` 返回类型，`pub` 可见性正确）。

- [ ] **Step 4: 提交**

```bash
git add crates/desktop/src/screenshot_commands.rs crates/desktop/src/main.rs
git commit -m "feat(desktop): hit_test_window 命令 + 注册（吸附 v1 Task4）"
```

---

### Task 5: 前端吸附（悬停高亮 + 单击即选 + Cmd 禁用）

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Screenshot/index.tsx`

前端 GUI 交互，无单测（手动验收）。改动点：①onMount 缓存 winOrigin（逻辑坐标）②onMouseMove idle 悬停节流查 hit_test → snapRef + draw ③draw() 加吸附高亮 ④onMouseUp selecting 单击分支选中吸附候选 ⑤Cmd 禁用。

**坐标约定**：选区/鼠标是本窗 CSS（`clientX/clientY`，= 逻辑 points）。winOrigin = `getCurrentWindow().outerPosition()`（物理）/ `scaleFactor()`（→ 逻辑）。全局逻辑 `gx = winOrigin.x + clientX`，与后端 `CGWindowList` / `CGDisplay` 同空间（Quartz 全局逻辑 points，Y 向下）。吸附矩形返回全局，前端 `localX = snap.x - winOrigin.x` 得本窗 CSS 绘制。

- [ ] **Step 1: 加 winOrigin / snap / 节流 refs + onMount 缓存**

在 `Screenshot` 组件内现有 refs 区（`startPtRef` 等 useRef 附近，约 60-90 行之间）加：

```tsx
  // 窗口识别吸附（v1）：winOrigin=本窗全局逻辑原点；snapRef=当前悬停吸附候选（本窗 CSS）；lastHitRef=节流时间戳。
  const winOriginRef = useRef<{ x: number; y: number } | null>(null);
  const snapRef = useRef<{ x: number; y: number; w: number; h: number } | null>(null);
  const lastHitRef = useRef(0);
```

在现有 `useEffect`（加载截图的那个）旁加一个 onMount effect 取 winOrigin：

```tsx
  // onMount：缓存本窗全局逻辑原点（outerPosition 物理 / scaleFactor → 逻辑 points）。
  // 截图覆盖窗定位后 show，onMount 时位置已稳定。
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const win = getCurrentWindow();
        const factor = await win.scaleFactor();
        const pos = await win.outerPosition(); // PhysicalPosition
        if (!cancelled && factor > 0) {
          winOriginRef.current = { x: pos.x / factor, y: pos.y / factor };
        }
      } catch { /* winOrigin 取不到 → 吸附自动失效（snapRef 永远 null），回退纯手动 */ }
    })();
    return () => { cancelled = true; };
  }, []);
```

- [ ] **Step 2: onMouseMove 加 idle 悬停吸附查询（节流 50Hz）**

在 `onMouseMove`（约 381 行）函数体最前面（`const mx = e.clientX; const my = e.clientY;` 之后、`if (mode === "scrolling") return;` 之前）插入：

```tsx
    // 窗口识别吸附：仅 idle（无选区、无标注工具）时悬停查询；Cmd 临时禁用。
    // 注：selecting/selected 不在此清 snapRef——必须保留供 onMouseUp 单击判定；
    //     draw() 的 mode==="idle" 守卫保证这些模式下不画吸附高亮。
    if (mode === "idle" && !sel && tool === "none") {
      if (e.metaKey) {
        if (snapRef.current) { snapRef.current = null; draw(); }
      } else if (winOriginRef.current) {
        const now = performance.now();
        if (now - lastHitRef.current >= 20) { // 50Hz 节流
          lastHitRef.current = now;
          const o = winOriginRef.current;
          const gx = o.x + mx;
          const gy = o.y + my;
          invoke<{ x: number; y: number; w: number; h: number } | null>("hit_test_window", { gx, gy })
            .then((snap) => {
              if (!snap) {
                if (snapRef.current) { snapRef.current = null; draw(); }
                return;
              }
              snapRef.current = { x: snap.x - o.x, y: snap.y - o.y, w: snap.w, h: snap.h };
              draw();
            })
            .catch(() => { /* 查询失败 → 不高亮，回退手动 */ });
        }
      }
    }
```

- [ ] **Step 3: draw() 加吸附高亮绘制**

在 `draw()` 函数内，绘制选区框/手柄/标注之后、函数返回之前，加：

```tsx
    // 窗口识别吸附高亮：idle 悬停时画候选窗口描边 + 5% 填充
    if (mode === "idle" && !sel && snapRef.current) {
      const s = snapRef.current;
      if (s.w > 0 && s.h > 0) {
        ctx.save();
        ctx.fillStyle = "rgba(59, 130, 246, 0.08)";   // 蓝 5% 填充
        ctx.fillRect(s.x, s.y, s.w, s.h);
        ctx.strokeStyle = "rgba(59, 130, 246, 0.9)";   // 蓝描边
        ctx.lineWidth = 2;
        ctx.setLineDash([6, 4]);
        ctx.strokeRect(s.x + 1, s.y + 1, s.w - 2, s.h - 2);
        ctx.restore();
      }
    }
```

> `ctx` 是 `draw()` 内已取得的 `canvasRef.current.getContext("2d")`（实现时按 `draw()` 现有变量名对齐；若 draw 内 ctx 变量名不同，替换为实际变量名）。注入位置：选区相关绘制全部完成后、`return` 前——读 `draw()` 末尾定位。

- [ ] **Step 4: onMouseUp selecting 单击分支选中吸附候选**

定位 `onMouseUp`（约 492 行）的 selecting 分支：

```tsx
    if (mode === "selecting" && sel) {
      if (sel.w < MIN_SIZE || sel.h < MIN_SIZE) { setSel(null); setModeSafe("idle"); }
      else { setModeSafe("selected"); }
    } else if ...
```

改为（单击 = sel 太小时，优先吃吸附候选）：

```tsx
    if (mode === "selecting" && sel) {
      if (sel.w < MIN_SIZE || sel.h < MIN_SIZE) {
        // 单击：若有吸附候选 → 选中整窗；否则清空回 idle（现状）
        if (snapRef.current && snapRef.current.w >= MIN_SIZE && snapRef.current.h >= MIN_SIZE) {
          const snapped = { ...snapRef.current };
          snapRef.current = null; // 选中后清，防残留高亮
          setSel(snapped);
          setModeSafe("selected");
        } else {
          setSel(null);
          setModeSafe("idle");
        }
      } else {
        setModeSafe("selected");
      }
    } else if (mode === "move" || mode === "resize") {
      setModeSafe("selected");
      setResizeHandle(null);
    }
```

并在选中吸附后清 snapRef（避免残留高亮）——在 `setSel({ ...snapRef.current })` 后加 `snapRef.current = null;`。

- [ ] **Step 5: （无需 mousedown 清 snapRef——保留供单击判定）**

selecting/selected 不画吸附高亮**由 draw() 的 `mode === "idle"` 守卫保证**（Step 3），故 onMouseDown **不**清 snapRef。snapRef 必须保留到 onMouseUp（Step 4）的单击分支吃掉它；若 mousedown 清空，单击即选会失效。拖拽完成（sel ≥ MIN_SIZE）后 mode=selected，draw 守卫不画，残留 snapRef 在回 idle 时由 onMouseMove（Step 2）刷新。

- [ ] **Step 6: tsc 类型检查**

Run: `cd crates/desktop/frontend && ./node_modules/.bin/tsc -p tsconfig.json --noEmit`
Expected: EXIT 0，无类型错误。

- [ ] **Step 7: 提交**

```bash
git add crates/desktop/frontend/src/pages/Screenshot/index.tsx
git commit -m "feat(screenshot): 区域截图悬停高亮+单击即选整窗吸附（v1 Task5）"
```

---

### Task 6: 文档同步

**Files:**
- Modify: `docs/architecture.md`
- Modify: `docs/features/screenshot.md`

- [ ] **Step 1: screenshot.md 加「窗口识别」节**

在 `docs/features/screenshot.md` 现有节之间（如第 2 节「区域截图流程」之后）插入新节：

```markdown
---

## N. 智能窗口识别（区域截图自动吸附）

区域截图模式下，鼠标悬停自动高亮候选窗口，单击即选中整窗（Snow Shot 式），减少手动框选。拖拽手画仍可用（mousedown 后 move > 阈值进拖拽，吸附灭）；按住 Cmd 临时禁用吸附做像素级精框。

- **粒度**：v1 仅窗口级（`CGWindowListCopyWindowInfo`，零额外权限，复用屏幕录制权限）；v2 预留元素级 AX（仅浏览器 bundle id，需辅助功能权限）。
- **命中算法**：`crates/capx/src/window_detect/mod.rs::pick_top_window` 纯函数——过滤自身 PID / layer<0 / 退化 bounds / 跨屏，按 layer 降序取最上层。
- **FFI**：`macos.rs::MacOsDetector` 复用 capture.rs 的 `CGWindowListCopyWindowInfo` 解析模式；`monitor_containing` 用 `CGDisplay` 找含点显示器做跨屏判定。
- **命令**：`hit_test_window(gx, gy) -> Option<SnapRect>`（全局逻辑坐标进/出）。
- **前端**：`Screenshot/index.tsx` onMount 缓存 winOrigin（outerPosition/scaleFactor → 逻辑），onMouseMove idle 悬停节流 50Hz 查询 → Canvas 蓝色描边+5% 填充高亮；onMouseUp 单击（sel < MIN_SIZE）吃吸附候选选中整窗。
- **坐标**：选区/鼠标=本窗 CSS；winOrigin+clientX=全局逻辑（与 CGWindowList/CGDisplay 同 Quartz 空间）；吸附 rect 返回全局，前端减 winOrigin 得本窗 CSS 绘制。
- **下游零改动**：吸附只改选区来源，标注/OCR/贴图/复制链路不变。
```

- [ ] **Step 2: architecture.md capx 模块说明加 window_detect**

在 `docs/architecture.md` 描述 `octopus-capx` 的位置（capture/stitch 旁）补一行模块说明：

```markdown
- `window_detect` — 区域截图窗口边界吸附：`WindowDetector` trait（跨平台）+ `pick_top_window` 命中纯函数 + macOS `CGWindowList` impl。v1 窗口级零权限，v2 预留 AX 元素级（仅浏览器）。
```

- [ ] **Step 3: 提交**

```bash
git add docs/architecture.md docs/features/screenshot.md
git commit -m "docs(screenshot): 窗口识别吸附模块说明（v1 Task6）"
```

---

### Task 8: v1.1 增强——前台 app 过滤（避免吸附被遮挡后台窗口）

> e2e 验收中发现：吸附被遮挡的后台 app 窗口会截到遮挡内容（截图是 t0 全屏快照）。决策（spec 关键决策 7）：吸附候选限定为**前台 app 窗口**，后台被遮挡的不吸附（用户先 CMD+Tab 聚焦再截）。放弃「激活+重截」（覆盖窗污染新快照+激活 API 权限 trade-off）与「单窗口截图 patch」（绘制管线/竞态复杂）两条路径——前台过滤最简最可靠。

**Files:**
- Modify: `crates/capx/src/window_detect/mod.rs`（`pick_top_window` 加 frontmost 过滤 + 测试）
- Modify: spec 关键决策 7 / 命中算法 / 错误处理 / 测试策略（已完成）

- [x] **Step 1: 写红测试** `background_app_visible_part_not_snapped`（旧逻辑吸附后台 B → fail）+ `frontmost_filter_excludes_background_app` / `foreground_app_multiple_windows_all_adhereable` / `no_layer0_window_disables_frontmost_filter`；改 `picks_higher_layer` 两窗同 owner（保留 layer 降序语义）
- [x] **Step 2: 跑确认红** `cargo test --manifest-path .../crates/capx/Cargo.toml window_detect` → 1 failed（background 红测试）+ 12 passed
- [x] **Step 3: 实现** `frontmost_pid` 计算（数组首个 layer0 非 self owner pid）+ 循环内 `owner≠frontmost` 过滤
- [x] **Step 4: 跑全 capx 测试绿** 45 passed
- [x] **Step 5: 同步文档** spec 决策 7 + screenshot.md §13 + 本 task

**实现要点**：frontmost 计算与过滤均在 `pick_top_window` 内部（无需调用方传参）；全无 layer0 非 self 窗口 → `frontmost=None` → 不过滤（退化为 v1.0，安全 fallback）。`macos.rs` / `hit_test_window` 命令 / 前端零改动。

### Task 7: 手动验收（macOS GUI）

截图吸附靠 GUI 交互，无法 headless 自动化，需手动验收。打包/`cargo run` 后逐项验证。

- [ ] **Step 1: 全量编译 + 单测绿**

Run: `cargo test --manifest-path crates/capx/Cargo.toml --no-fail-fast` + `cd crates/desktop/frontend && ./node_modules/.bin/tsc -p tsconfig.json --noEmit`
Expected: capx 单测全绿（含 7 个 window_detect 测试）、tsc EXIT 0。

- [ ] **Step 2: 手动验收清单（macOS，多窗口环境）**

触发截图快捷键进入区域截图模式，逐项验证：

1. **悬停高亮**：鼠标移到某应用窗口上 → 该窗口出现蓝色虚线描边 + 淡蓝填充
2. **单击即选**：在某窗口上单击 → 选区 = 整窗（含 8 手柄 + 尺寸标注），进入 selected
3. **拖拽手画覆盖**：在窗口上 mousedown 后拖动 > 阈值 → 走原手画框选（吸附高亮灭），松开后选区=手画区域
4. **Cmd 禁用**：按住 Cmd 移动鼠标 → 无高亮；拖拽 = 纯手动精框
5. **落空不动**：单击纯桌面（无窗口候选）→ 选区不动（不报错、不清空已有选区）
6. **多显示器**：鼠标在副屏窗口上悬停 → 高亮该副屏窗口；单击选中（坐标换算正确，无偏移）
7. **跨屏窗口跳过**：横跨两屏的窗口悬停时不高亮（被跳过）
8. **OCR fallback**：吸附选中某窗口 → 点工具栏 OCR → OCR 该整窗内容（下游链路未被吸附破坏）
9. **自身窗口不命中**：吸附高亮永远不会命中 octopus 自己的截图覆盖窗（按 PID 滤除）

- [ ] **Step 3: 坐标对齐抽查（关键，防 Quartz 坐标陷阱）**

验收 6（多显示器）时重点确认：副屏窗口的高亮描边**精准贴齐窗口真实边界**，无整体偏移。若发现系统性偏移（如 Y 轴翻转、scale 倍数错），检查 `winOriginRef`（outerPosition 物理/scaleFactor）与 `CGDisplay::bounds` 是否同空间——这是 macOS 坐标系最易踩的点。

- [ ] **Step 4: 验收通过后无额外提交（验收清单本身不进 commit）**

---

## Verification 总览

1. **单测**：`cargo test -p octopus-capx window_detect`（7 测试绿）
2. **编译**：`cargo build --workspace`；前端 `tsc --noEmit` EXIT 0
3. **手动验收**：Task 7 清单 9 项 + 坐标对齐抽查
4. **e2e 不适用**：本功能非 ASR/pipeline，GUI 交互手动验收为主，不套真实录音断言那套铁律
