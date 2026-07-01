use objc2_app_kit::NSRunningApplication;
use objc2_foundation::MainThreadMarker;

/// 获取指定坐标下最上层非自身应用的 window owner PID。
pub fn get_window_pid_at_point(x: f64, y: f64) -> Option<i32> {
    use core_graphics::display::CGDisplay;
    let windows = CGDisplay::window_list_info(
        core_graphics::display::kCGWindowListOptionOnScreenOnly,
        None,
    )?;

    use core_foundation::base::{CFTypeRef, TCFType};
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::string::CFString;
    use core_foundation::number::CFNumber;

    let curr_pid = std::process::id() as i32;

    for item in windows.iter() {
        let dict_ref = *item as CFTypeRef;
        if dict_ref.is_null() { continue; }
        let dict: CFDictionary<CFString, CFTypeRef> = unsafe { TCFType::wrap_under_get_rule(dict_ref as *const _) };

        let key_pid = CFString::new("kCGWindowOwnerPID");
        let pid_item = dict.get(&key_pid);
        let pid_ptr: CFTypeRef = *pid_item;
        if pid_ptr.is_null() { continue; }
        let pid_num: CFNumber = unsafe { TCFType::wrap_under_get_rule(pid_ptr as *const _) };
        let pid = pid_num.to_i32()?;
        if pid == curr_pid { continue; }

        let key_layer = CFString::new("kCGWindowLayer");
        let layer_item = dict.get(&key_layer);
        let layer_ptr: CFTypeRef = *layer_item;
        if !layer_ptr.is_null() {
            let layer_num: CFNumber = unsafe { TCFType::wrap_under_get_rule(layer_ptr as *const _) };
            if let Some(layer) = layer_num.to_i32() {
                if layer != 0 { continue; }
            }
        }

        let key_bounds = CFString::new("kCGWindowBounds");
        let bounds_item = dict.get(&key_bounds);
        let bounds_ptr: CFTypeRef = *bounds_item;
        if !bounds_ptr.is_null() {
            let bdict: CFDictionary<CFString, CFTypeRef> = unsafe { TCFType::wrap_under_get_rule(bounds_ptr as *const _) };
            let get_f64 = |key: &str| -> f64 {
                let k = CFString::new(key);
                let item = bdict.get(&k);
                let ptr: CFTypeRef = *item;
                if ptr.is_null() { return 0.0; }
                let n: CFNumber = unsafe { TCFType::wrap_under_get_rule(ptr as *const _) };
                n.to_f64().unwrap_or(0.0)
            };
            let (bx, by, bw, bh) = (get_f64("X"), get_f64("Y"), get_f64("Width"), get_f64("Height"));
            if x >= bx && x < bx + bw && y >= by && y < by + bh {
                return Some(pid);
            }
        }
    }
    None
}

/// 激活指定 PID 的应用（必须在主线程调用）。
pub fn activate_app_by_pid(pid: i32) {
    if let Some(mtm) = MainThreadMarker::new() {
        if let Some(app) = NSRunningApplication::runningApplicationWithProcessIdentifier(pid) {
            let success = app.activateWithOptions(objc2_app_kit::NSApplicationActivationOptions(1 << 1));
            if success {
                log::info!("[scroll-capture] activated app pid={}", pid);
            }
        }
    }
}

/// 获取主屏高度（用于 Y 轴翻转）。
pub fn get_primary_screen_height() -> f64 {
    use objc2::{class, msg_send, runtime::AnyObject};
    unsafe {
        let screens: *mut AnyObject = msg_send![class!(NSScreen), screens];
        let primary: *mut AnyObject = msg_send![screens, objectAtIndex: 0];
        let frame: objc2_foundation::NSRect = msg_send![primary, frame];
        frame.size.height as f64
    }
}
