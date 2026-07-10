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
        if array_ref.is_null() {
            return None;
        }
        let array = CFArray::<CFDictionary>::wrap_under_create_rule(array_ref);

        let pid_key = CFString::from_static_string("kCGWindowOwnerPID");
        let layer_key = CFString::from_static_string("kCGWindowLayer");
        let bounds_key = CFString::from_static_string("kCGWindowBounds");
        let x_key = CFString::from_static_string("X");
        let y_key = CFString::from_static_string("Y");
        let w_key = CFString::from_static_string("Width");
        let h_key = CFString::from_static_string("Height");

        // CFArray::len 返回 isize；Vec::with_capacity 需 usize。
        let mut out: Vec<WinInfo> = Vec::with_capacity(array.len() as usize);
        for i in 0..array.len() {
            // array.get 返回 Option<ItemRef<CFDictionary>>；ItemRef 实现 Deref<Target=CFDictionary>。
            // 必须让 ItemRef 存活整个循环体（不能在 match 臂里 &*d 借出再丢弃），故绑定后用 &*。
            let dict = match array.get(i) {
                Some(d) => d,
                None => continue,
            };
            let dict: &CFDictionary = &*dict;
            let pid = get_i32(dict, &pid_key);
            let layer = get_i32(dict, &layer_key);
            let (x, y, w, h) = match dict.find(bounds_key.as_CFTypeRef()) {
                Some(bv) => {
                    let bd = CFDictionary::<
                        *const std::ffi::c_void,
                        *const std::ffi::c_void,
                    >::wrap_under_get_rule(*bv as *const _);
                    (
                        get_f64(&bd, &x_key),
                        get_f64(&bd, &y_key),
                        get_f64(&bd, &w_key),
                        get_f64(&bd, &h_key),
                    )
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
            return MonitorRect {
                x: b.origin.x,
                y: b.origin.y,
                w: b.size.width,
                h: b.size.height,
            };
        }
    }
    fallback
}

/// 对外暴露的便利函数：给定全局坐标，自动定 monitor 并命中（供 desktop 命令直接调）。
pub fn hit_test_window_global(gx: f64, gy: f64) -> Option<SnapRect> {
    let monitor = monitor_containing(gx, gy);
    MacOsDetector::new().hit_test(gx, gy, Granularity::Window, monitor)
}

/// 读 CGWindowList 字典里的 i32 字段（kCGWindowOwnerPID / kCGWindowLayer）。
/// wrap_under_get_rule 是 unsafe（不获取所有权，仅借用 C 端引用计数），需 unsafe 块。
fn get_i32(dict: &CFDictionary, key: &CFString) -> Option<i32> {
    let v = dict.find(key.as_CFTypeRef())?;
    unsafe { CFNumber::wrap_under_get_rule(*v as *const _).to_i32() }
}

/// 读 bounds 子字典里的 f64 字段（X/Y/Width/Height）。优先按 i64 解释（CGWindowList bounds
/// 实际是整数 points），失败再 fallback 到 f64。
fn get_f64(dict: &CFDictionary, key: &CFString) -> Option<f64> {
    let v = dict.find(key.as_CFTypeRef())?;
    let n = unsafe { CFNumber::wrap_under_get_rule(*v as *const _) };
    n.to_i64().map(|i| i as f64).or_else(|| n.to_f64())
}
