use anyhow::{Context, Result};
use serde::Serialize;
use xcap::Window;

#[derive(Debug, Clone, Serialize)]
pub struct WindowRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub pid: u32,
    pub id: u32,
    pub app_name: String,
}

/// 找到包含逻辑坐标 (x, y) 的最顶层应用窗口。
pub fn window_at_point(x: f64, y: f64, scale_factor: f64) -> Result<Option<WindowRect>> {
    let windows = Window::all().context("Failed to list windows")?;
    let phys_x = x * scale_factor;
    let phys_y = y * scale_factor;
    log::info!("window_at_point: logical({}, {}) → phys({}, {}), {} windows total", x, y, phys_x, phys_y, windows.len());

    let mut filtered: Vec<_> = windows.into_iter()
        .filter(|w| {
            let title = w.title().unwrap_or_default();
            let name = w.app_name().unwrap_or_default();
            let w_val = w.width().unwrap_or(0);
            let h_val = w.height().unwrap_or(0);
            if w_val < 50 || h_val < 50 { return false; }
            if w.is_minimized().unwrap_or(false) { return false; }
            let name_lower = name.to_lowercase();
            let title_lower = title.to_lowercase();
            if title.is_empty() && name.is_empty() { return false; }
            if name_lower.contains("dock") || title_lower.contains("dock") { return false; }
            if name_lower == "finder" && (title_lower == "desktop" || title.is_empty()) { return false; }
            if name_lower.contains("octopus") || name_lower.contains("screenshot") { return false; }
            true
        })
        .collect();

    filtered.sort_by(|a, b| {
        b.z().unwrap_or(0).cmp(&a.z().unwrap_or(0))
    });
    log::info!("window_at_point: {} windows after filter", filtered.len());
    for w in &filtered {
        log::info!("  window: name={} title={} x={} y={} w={} h={} z={}",
            w.app_name().unwrap_or_default(),
            w.title().unwrap_or_default(),
            w.x().unwrap_or(0), w.y().unwrap_or(0),
            w.width().unwrap_or(0), w.height().unwrap_or(0),
            w.z().unwrap_or(0));
    }

    for w in filtered {
        let wx = w.x().unwrap_or(0) as f64;
        let wy = w.y().unwrap_or(0) as f64;
        let ww = w.width().unwrap_or(0) as f64;
        let wh = w.height().unwrap_or(0) as f64;

        if phys_x >= wx && phys_x <= wx + ww && phys_y >= wy && phys_y <= wy + wh {
            return Ok(Some(WindowRect {
                x: wx / scale_factor,
                y: wy / scale_factor,
                w: ww / scale_factor,
                h: wh / scale_factor,
                pid: w.pid().unwrap_or(0),
                id: w.id().unwrap_or(0),
                app_name: w.app_name().unwrap_or_default(),
            }));
        }
    }

    Ok(None)
}

/// 激活窗口到最前面（跨平台）。
pub fn activate_window(pid: u32, _window_id: u32) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        use objc2_app_kit::{NSRunningApplication, NSApplicationActivationOptions};
        let app = NSRunningApplication::runningApplicationWithProcessIdentifier(pid as i32);
        if let Some(app) = &app {
            #[allow(deprecated)]
            app.activateWithOptions(NSApplicationActivationOptions::ActivateIgnoringOtherApps);
            log::info!("Activated app pid={}", pid);
        }
    }
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::SetForegroundWindow;
        unsafe {
            let hwnd = HWND(_window_id as isize);
            let _ = SetForegroundWindow(hwnd);
        }
    }
    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("xdotool")
            .args(["windowactivate", &_window_id.to_string()])
            .spawn();
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        log::warn!("Window activation not supported on this platform");
    }
    Ok(())
}
