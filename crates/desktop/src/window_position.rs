//! 窗口位置持久化：保存/恢复窗口的 x, y 坐标到 app_config。
//! 使用 category='system' 与业务配置隔离。
//! 恢复时检测坐标是否在可见显示器范围内（防外接显示器拔出后窗口消失）。

use log::debug;

/// 保存窗口位置到 app_config。
/// key 格式：`window_pos.{label}`，value 格式：`"x,y"`（逻辑坐标）。
pub fn save_window_position(label: &str, x: f64, y: f64) {
    let key = format!("window_pos.{}", label);
    let value = format!("{:.0},{:.0}", x, y);
    if let Err(e) = octopus_infra::db::save_config_key(&key, &value) {
        log::warn!("Failed to save window position for {}: {}", label, e);
    } else {
        debug!("Saved window position {}: {},{}", label, x, y);
    }
}

/// 从 app_config 恢复窗口位置。
/// 坐标不在任何显示器范围内时返回 None（调用方 fallback 到默认位置）。
pub fn load_window_position(label: &str) -> Option<(f64, f64)> {
    let key = format!("window_pos.{}", label);
    let value = octopus_infra::db::load_config_key(&key).ok().flatten()?;
    let (x, y) = parse_position(&value)?;
    debug!("Loaded window position {}: {},{}", label, x, y);
    Some((x, y))
}

/// 检查坐标是否在任一显示器的可视范围内（含 50px 容差）。
pub fn is_position_visible(
    x: f64,
    y: f64,
    monitors: &[tauri::Monitor],
) -> bool {
    const TOLERANCE: f64 = 50.0;
    monitors.iter().any(|m| {
        let ms = m.scale_factor();
        let mx = m.position().x as f64 / ms;
        let my = m.position().y as f64 / ms;
        let mw = m.size().width as f64 / ms;
        let mh = m.size().height as f64 / ms;
        x >= mx - TOLERANCE
            && x <= mx + mw + TOLERANCE
            && y >= my - TOLERANCE
            && y <= my + mh + TOLERANCE
    })
}

/// 恢复窗口位置：从 DB 读取 → 检查可见性 → 应用或 fallback。
/// 返回 true 表示成功恢复，false 表示 fallback 到默认。
pub fn restore_window_position(
    window: &tauri::WebviewWindow,
    label: &str,
    fallback: impl FnOnce(&tauri::WebviewWindow),
) {
    if let Some((x, y)) = load_window_position(label) {
        let monitors = window.available_monitors().unwrap_or_default();
        if is_position_visible(x, y, &monitors) {
            let _ = window.set_position(tauri::Position::Logical(
                tauri::LogicalPosition::new(x, y),
            ));
            debug!("Restored {} position to {},{}", label, x, y);
            return;
        } else {
            debug!("Saved position {},{} not visible, fallback", x, y);
        }
    }
    fallback(window);
}

/// 保存窗口当前位置（用于 window event handler）。
pub fn save_current_position(window: &tauri::WebviewWindow, label: &str) {
    if let Ok(pos) = window.outer_position() {
        let scale = window.scale_factor().unwrap_or(1.0);
        let x = pos.x as f64 / scale;
        let y = pos.y as f64 / scale;
        save_window_position(label, x, y);
    }
}

fn parse_position(value: &str) -> Option<(f64, f64)> {
    let parts: Vec<&str> = value.split(',').collect();
    if parts.len() != 2 {
        return None;
    }
    let x = parts[0].trim().parse::<f64>().ok()?;
    let y = parts[1].trim().parse::<f64>().ok()?;
    Some((x, y))
}
