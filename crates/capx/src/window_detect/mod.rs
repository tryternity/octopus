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
#[derive(Debug, Clone, PartialEq, Serialize)]
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

/// 显示器矩形（跨屏判定用；全局显示逻辑坐标，points）。
///
/// 注意：本类型**不含 `scale`**——capx 只做几何命中判定，scale 是前端渲染关注点。
/// desktop crate 的 `screenshot_geometry::MonitorRect` 带 `scale: f64`（物理↔逻辑换算），
/// 二者分属不同 crate 层（desktop 依赖 capx，反向不可），故各自独立、无需互转——
/// desktop 的 `hit_test_window` 命令只传 `(gx, gy)`，capx 内部用 CGDisplay 构造本类型。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MonitorRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// 窗口识别器 trait（跨平台抽象，仿 PinWindow trait）。
///
/// v1 仅实现 `Granularity::Window` 分支；`Element`（AX 元素级）v2 才补，v1 返回 None。
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
/// 过滤：① pid == self_pid（排除 octopus 自身拥有的所有窗口：截图覆盖窗 + 主窗 + 任何子窗——截图时绝不吸附自己的窗口）② layer < 0（桌面/壁纸）
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
    let mut best: Option<(i32, SnapRect)> = None;
    for win in windows {
        // 过滤
        if win.pid == self_pid {
            continue;
        }
        if win.layer < 0 {
            continue;
        }
        if win.w <= 0.0 || win.h <= 0.0 {
            continue;
        }
        // 跨屏：bounds 必须完全在 monitor 内
        let fully_inside = win.x >= monitor.x
            && win.y >= monitor.y
            && win.x + win.w <= monitor.x + monitor.w
            && win.y + win.h <= monitor.y + monitor.h;
        if !fully_inside {
            continue;
        }
        // 命中点
        if !(gx >= win.x && gx < win.x + win.w && gy >= win.y && gy < win.y + win.h) {
            continue;
        }
        // layer 降序取最上层；layer 相同保留先出现者（数组顺序 = z-order，前 = 上层）
        let take = match &best {
            None => true,
            Some((bl, _)) => win.layer > *bl, // 严格大于：同 layer 不替换 → 保留先出现
        };
        if take {
            best = Some((
                win.layer,
                SnapRect {
                    x: win.x,
                    y: win.y,
                    w: win.w,
                    h: win.h,
                    title: None,
                },
            ));
        }
    }
    best.map(|(_, r)| r)
}

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "macos")]
pub use macos::{hit_test_window_global, MacOsDetector};

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

    #[test]
    fn right_bottom_edge_is_half_open_miss() {
        // 命中区间左闭右开 [x, x+w)：右边界 x+w 恰好等于应 miss
        let ws = [win(100, 0, 0.0, 0.0, 100.0, 100.0)];
        assert!(pick_top_window(&ws, 100.0, 50.0, MON, 999).is_none()); // 右边界
        assert!(pick_top_window(&ws, 50.0, 100.0, MON, 999).is_none()); // 下边界
    }

    #[test]
    fn left_top_edge_is_inclusive_hit() {
        // 左/上边界 x / y 恰好等于应命中（闭）
        let ws = [win(100, 0, 10.0, 10.0, 100.0, 100.0)];
        let r = pick_top_window(&ws, 10.0, 10.0, MON, 999).unwrap();
        assert_eq!((r.x, r.y), (10.0, 10.0));
    }
}
