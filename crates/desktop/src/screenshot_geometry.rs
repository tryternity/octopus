//! start_scroll_recording 提取出的纯逻辑：坐标换算、显示器命中、preview 裁剪参数。
//! 所有函数不依赖 Tauri/Quartz 类型，输入输出均为纯数据。

/// 显示器矩形（从 Tauri Monitor 提取的纯数据，用于命中检测）。
#[derive(Debug, Clone, Copy)]
pub(crate) struct MonitorRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub scale: f64,
}

/// 选区的全局逻辑坐标。
#[derive(Debug, Clone, Copy)]
pub(crate) struct SelectionGlobal {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// 选区在目标显示器内的物理像素裁剪参数。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PhysicalCrop {
    pub px: u32,
    pub py: u32,
    pub pw: u32,
    pub ph: u32,
}

/// 选区全局坐标 = 窗口原点 + CSS 偏移。
pub(crate) fn compute_selection_global(
    win_origin_x: f64,
    win_origin_y: f64,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
) -> SelectionGlobal {
    SelectionGlobal {
        x: win_origin_x + x,
        y: win_origin_y + y,
        w,
        h,
    }
}

/// 找到包含点 (cx, cy) 的显示器索引，无命中返回 None。
pub(crate) fn find_monitor_for_point(
    monitors: &[MonitorRect],
    cx: f64,
    cy: f64,
) -> Option<usize> {
    monitors.iter().position(|m| {
        cx >= m.x && cx < m.x + m.w && cy >= m.y && cy < m.y + m.h
    })
}

/// 计算选区在显示器内的物理像素裁剪参数。
pub(crate) fn compute_physical_crop(
    sel: &SelectionGlobal,
    mon: &MonitorRect,
) -> PhysicalCrop {
    PhysicalCrop {
        px: ((sel.x - mon.x).max(0.0) * mon.scale) as u32,
        py: ((sel.y - mon.y).max(0.0) * mon.scale) as u32,
        pw: (sel.w * mon.scale) as u32,
        ph: (sel.h * mon.scale) as u32,
    }
}

/// 计算预览裁剪参数：从 canvas 底部取最近 N 行用于生成预览缩略图。
/// 返回 (crop_src_h, crop_y)。
pub(crate) fn compute_preview_crop(
    canvas_h: u32,
    canvas_w: u32,
    preview_w: u32,
    max_preview_h: u32,
) -> (u32, u32) {
    let src_h = ((canvas_h as u64 * canvas_w as u64 / preview_w as u64)
        .min(canvas_h as u64)) as u32;
    let crop_src_h = src_h
        .min(max_preview_h * canvas_w / preview_w)
        .min(canvas_h);
    let crop_y = canvas_h - crop_src_h;
    (crop_src_h, crop_y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_global_adds_origin() {
        let s = compute_selection_global(100.0, 200.0, 10.0, 20.0, 300.0, 400.0);
        assert_eq!(s.x, 110.0);
        assert_eq!(s.y, 220.0);
        assert_eq!(s.w, 300.0);
        assert_eq!(s.h, 400.0);
    }

    #[test]
    fn find_monitor_center_hit() {
        let monitors = vec![
            MonitorRect { x: 0.0, y: 0.0, w: 1920.0, h: 1080.0, scale: 1.0 },
            MonitorRect { x: 1920.0, y: 0.0, w: 2560.0, h: 1440.0, scale: 2.0 },
        ];
        assert_eq!(find_monitor_for_point(&monitors, 2000.0, 500.0), Some(1));
        assert_eq!(find_monitor_for_point(&monitors, 960.0, 540.0), Some(0));
    }

    #[test]
    fn find_monitor_no_hit_returns_none() {
        let monitors = vec![
            MonitorRect { x: 0.0, y: 0.0, w: 1920.0, h: 1080.0, scale: 1.0 },
        ];
        assert_eq!(find_monitor_for_point(&monitors, 3000.0, 500.0), None);
    }

    #[test]
    fn physical_crop_basic() {
        let sel = SelectionGlobal { x: 100.0, y: 50.0, w: 200.0, h: 300.0 };
        let mon = MonitorRect { x: 0.0, y: 0.0, w: 1920.0, h: 1080.0, scale: 2.0 };
        let crop = compute_physical_crop(&sel, &mon);
        assert_eq!(crop.px, 200);
        assert_eq!(crop.py, 100);
        assert_eq!(crop.pw, 400);
        assert_eq!(crop.ph, 600);
    }

    #[test]
    fn physical_crop_with_monitor_offset() {
        let sel = SelectionGlobal { x: 2000.0, y: 100.0, w: 100.0, h: 100.0 };
        let mon = MonitorRect { x: 1920.0, y: 0.0, w: 2560.0, h: 1440.0, scale: 2.0 };
        let crop = compute_physical_crop(&sel, &mon);
        assert_eq!(crop.px, 160);
        assert_eq!(crop.py, 200);
        assert_eq!(crop.pw, 200);
        assert_eq!(crop.ph, 200);
    }

    #[test]
    fn physical_crop_selection_before_monitor_clamps_to_zero() {
        // 跨显示器边界：选区左边缘在目标显示器左侧之外 → px/py 应 clamp 到 0 而非 wrap
        let sel = SelectionGlobal { x: 1900.0, y: -50.0, w: 100.0, h: 100.0 };
        let mon = MonitorRect { x: 1920.0, y: 0.0, w: 2560.0, h: 1440.0, scale: 2.0 };
        let crop = compute_physical_crop(&sel, &mon);
        assert_eq!(crop.px, 0);  // (1900-1920).max(0.0) * 2 = 0
        assert_eq!(crop.py, 0);  // (-50-0).max(0.0) * 2 = 0
    }

    #[test]
    fn preview_crop_small_canvas() {
        let (src_h, y) = compute_preview_crop(500, 800, 400, 1200);
        assert_eq!(src_h, 500);
        assert_eq!(y, 0);
    }

    #[test]
    fn preview_crop_large_canvas() {
        let (src_h, y) = compute_preview_crop(5000, 800, 400, 1200);
        assert_eq!(src_h, 2400);
        assert_eq!(y, 5000 - 2400);
    }
}
