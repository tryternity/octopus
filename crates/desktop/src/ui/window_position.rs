//! 窗口位置持久化：保存/恢复窗口的 x, y 坐标到 app_config。
//! 使用 category='system' 与业务配置隔离。
//! 恢复时检测坐标是否在可见显示器范围内（防外接显示器拔出后窗口消失）。

use log::debug;

/// dock 状态内存缓存——P1-8 修复（2026-07-17）。
///
/// 原先 `load_dock_state` 每次都走 DB SELECT，clipboard_window 的 Moved 事件
/// 拖拽期间 ~60Hz 触发 → 拖一次产生数百次 DB round-trip（SQLite 单行 SELECT
/// 本身不慢，但 acquire ReentrantMutex + prepare statement 是纯浪费）。
///
/// 改为内存镜像：`save_dock_state` 同步更新缓存 + DB；`load_dock_state` 优先
/// 读缓存，首次或缓存未命中时回退 DB 并填缓存。窗口 label 数量极少（当前
/// 仅 clipboard_window），缓存规模可控。
static DOCK_CACHE: once_cell::sync::Lazy<std::sync::Mutex<std::collections::HashMap<String, String>>> =
    once_cell::sync::Lazy::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

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

// ── 按显示器存位置（result_window 多屏跟随，spec 2026-07-31）──
// key 格式：`window_pos.{label}@{display_id}`，value 同单值 `"x,y"`（逻辑坐标）。
// 每屏独立存；display_id 变（热插拔）→ key 对不上 → fallback 默认（符合预期）。

/// 保存窗口位置到指定显示器（按 display_id 分键）。
pub(crate) fn save_window_position_for_display(
    label: &str,
    display_id: u32,
    x: f64,
    y: f64,
) {
    let key = format!("window_pos.{}@{}", label, display_id);
    let value = format!("{:.0},{:.0}", x, y);
    if let Err(e) = octopus_infra::db::save_config_key(&key, &value) {
        log::warn!("Failed to save window position for {}@{}: {}", label, display_id, e);
    } else {
        debug!("Saved window position {}@{}: {},{}", label, display_id, x, y);
    }
}

/// 读取指定显示器的窗口位置（按 display_id 分键）。无则 None。
pub(crate) fn load_window_position_for_display(
    label: &str,
    display_id: u32,
) -> Option<(f64, f64)> {
    let key = format!("window_pos.{}@{}", label, display_id);
    let value = octopus_infra::db::load_config_key(&key).ok().flatten()?;
    let (x, y) = parse_position(&value)?;
    debug!("Loaded window position {}@{}: {},{}", label, display_id, x, y);
    Some((x, y))
}

/// 保存窗口当前位置（按窗口当前所在 display_id 分键）。
/// 找不到窗口所在屏的 display_id 时静默跳过（兼容非 macOS / 无屏）。
pub(crate) fn save_current_position_per_display(window: &tauri::WebviewWindow, label: &str) {
    let Ok(pos) = window.outer_position() else { return };
    let scale = window.scale_factor().unwrap_or(1.0);
    let x = pos.x as f64 / scale;
    let y = pos.y as f64 / scale;
    match find_window_display_id(window) {
        Some(display_id) => {
            save_window_position_for_display(label, display_id, x, y);
        }
        None => {
            // 找不到 display_id（非 macOS / 屏匹配失败）→ fallback 到单值，保持兼容
            debug!("save_current_position_per_display: no display_id for {}, fallback to single-key", label);
            save_window_position(label, x, y);
        }
    }
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

/// 保存窗口 dock 状态到 app_config + 同步更新内存缓存。
/// key 格式：`window_dock.{label}`，value: "right" | "left" | "none"。
pub fn save_dock_state(label: &str, edge: &str) {
    let key = format!("window_dock.{}", label);
    if let Err(e) = octopus_infra::db::save_config_key(&key, edge) {
        log::warn!("Failed to save dock state for {}: {}", label, e);
    } else {
        debug!("Saved dock state {}: {}", label, edge);
    }
    // 同步内存缓存——load_dock_state 后续走缓存不触 DB
    DOCK_CACHE.lock().unwrap().insert(label.to_string(), edge.to_string());
}

/// 读取窗口 dock 状态——优先内存缓存，首次/未命中回退 DB 并填缓存。
///
/// P1-8 修复（2026-07-17）：clipboard_window 的 Moved 事件 ~60Hz 触发，
/// 原先每次走 DB SELECT，拖拽产生数百次 DB round-trip。改内存优先避免。
pub fn load_dock_state(label: &str) -> Option<String> {
    // 1. 内存命中（高频路径，零 DB）
    if let Some(edge) = DOCK_CACHE.lock().unwrap().get(label).cloned() {
        if edge.is_empty() { return None; }
        return Some(edge);
    }
    // 2. 未命中：查 DB 并填缓存
    let key = format!("window_dock.{}", label);
    let value = octopus_infra::db::load_config_key(&key).ok().flatten()?;
    let edge = value.trim().to_string();
    if edge.is_empty() {
        // 空值也缓存（避免反复查 DB），但 load 返回 None
        DOCK_CACHE.lock().unwrap().insert(label.to_string(), String::new());
        return None;
    }
    debug!("Loaded dock state from DB (cache miss): {} {}", label, edge);
    DOCK_CACHE.lock().unwrap().insert(label.to_string(), edge.clone());
    Some(edge)
}

// ── 多屏 helper（macOS 原生 CoreGraphics，spec 2026-07-31 单键三模式 result_window 跟随鼠标屏）──

/// 获取鼠标全局位置（macOS Quartz 逻辑坐标，y 轴向下；非 macOS 返回 None）。
///
/// CGEvent::location() 返回逻辑坐标（points），**不除 scale**（AGENTS.md 坐标 gotcha）。
#[cfg(target_os = "macos")]
pub(crate) fn get_mouse_location() -> Option<(f64, f64)> {
    use core_graphics::event::CGEvent;
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState).ok()?;
    let event = CGEvent::new(source).ok()?;
    let point = event.location();
    Some((point.x, point.y))
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn get_mouse_location() -> Option<(f64, f64)> {
    None
}

/// 鼠标所在显示器的信息（display_id + 逻辑 bounds origin_x/origin_y/width/height）。
///
/// 用 CGDisplay::active_displays() + bounds() 找鼠标所在的屏（CoreGraphics 原生
/// 逻辑坐标，不除 scale——AGENTS.md 坐标 gotcha）。返回 display_id 供按屏存取 key 用。
#[cfg(target_os = "macos")]
pub(crate) fn find_monitor_at_mouse(
    mouse: Option<(f64, f64)>,
) -> Option<(u32, f64, f64, f64, f64)> {
    use core_graphics::display::CGDisplay;
    let (mx, my) = mouse?;
    let displays = CGDisplay::active_displays().ok()?;
    for display_id in displays {
        let bounds = CGDisplay::new(display_id).bounds();
        if bounds.size.width <= 0.0 || bounds.size.height <= 0.0 {
            continue;
        }
        // CGDisplay::bounds() 返回 Quartz 逻辑坐标（points），与 CGEvent::location() 同坐标系。
        if mx >= bounds.origin.x
            && mx < bounds.origin.x + bounds.size.width
            && my >= bounds.origin.y
            && my < bounds.origin.y + bounds.size.height
        {
            return Some((
                display_id,
                bounds.origin.x,
                bounds.origin.y,
                bounds.size.width,
                bounds.size.height,
            ));
        }
    }
    None
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn find_monitor_at_mouse(
    _mouse: Option<(f64, f64)>,
) -> Option<(u32, f64, f64, f64, f64)> {
    None
}

/// 找窗口当前所在屏的 display_id（save 时用——按屏存 key）。
///
/// ⚠️ 难点：Tauri `Monitor` 不暴露 CGDirectDisplayID。解法：用窗口所在屏的**逻辑 origin**
/// （Monitor::position ÷ scale）去匹配 `CGDisplay::active_displays()` 里 bounds.origin
/// 相同的 display_id。两端（save 经 origin 匹配、load 经 find_monitor_at_mouse）都拿到
/// 同一个 display_id，key 一致。
///
/// 窗口位置用 outer_position（物理像素）÷ scale 转逻辑后判落在哪个 Tauri Monitor，
/// 再用该 Monitor 的 origin 去 CGDisplay 找 display_id。
#[cfg(target_os = "macos")]
pub(crate) fn find_window_display_id(window: &tauri::WebviewWindow) -> Option<u32> {
    use core_graphics::display::CGDisplay;
    let monitors = window.available_monitors().ok()?;
    let scale = window.scale_factor().unwrap_or(1.0);
    let win_pos = window.outer_position().ok()?;
    let wx = win_pos.x as f64 / scale;
    let wy = win_pos.y as f64 / scale;
    // 找窗口所在 Tauri Monitor（逻辑坐标比较）
    let win_monitor = monitors.into_iter().find(|m| {
        let mx = m.position().x as f64 / scale;
        let my = m.position().y as f64 / scale;
        let mw = m.size().width as f64 / scale;
        let mh = m.size().height as f64 / scale;
        wx >= mx && wx < mx + mw && wy >= my && wy < my + mh
    })?;
    let mon_ox = win_monitor.position().x as f64 / scale;
    let mon_oy = win_monitor.position().y as f64 / scale;
    // 用 origin 匹配 CGDisplay 找 display_id（容差 1pt 防 float 误差）
    let displays = CGDisplay::active_displays().ok()?;
    for display_id in displays {
        let bounds = CGDisplay::new(display_id).bounds();
        if (bounds.origin.x - mon_ox).abs() < 1.0 && (bounds.origin.y - mon_oy).abs() < 1.0 {
            return Some(display_id);
        }
    }
    None
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn find_window_display_id(_window: &tauri::WebviewWindow) -> Option<u32> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dock_state_round_trip() {
        let label = "test_window_dock_roundtrip";
        crate::ui::window_position::save_dock_state(label, "right");
        let loaded = crate::ui::window_position::load_dock_state(label);
        assert_eq!(loaded.as_deref(), Some("right"));

        crate::ui::window_position::save_dock_state(label, "left");
        let loaded = crate::ui::window_position::load_dock_state(label);
        assert_eq!(loaded.as_deref(), Some("left"));

        crate::ui::window_position::save_dock_state(label, "none");
        let loaded = crate::ui::window_position::load_dock_state(label);
        assert_eq!(loaded.as_deref(), Some("none"));
    }

    // ── 按屏存位置（spec 2026-07-31 result_window 多屏跟随）──

    #[test]
    fn per_display_save_load_round_trip() {
        let label = "test_window_per_display_rt";
        // 屏 1 存 (120, 80)
        save_window_position_for_display(label, 1, 120.0, 80.0);
        assert_eq!(
            load_window_position_for_display(label, 1),
            Some((120.0, 80.0)),
            "同 display_id 应读到存入的坐标"
        );
        // 屏 2 没存 → None
        assert_eq!(
            load_window_position_for_display(label, 2),
            None,
            "不同 display_id 应返回 None"
        );
        // 屏 2 存不同坐标，不影响屏 1
        save_window_position_for_display(label, 2, 1560.0, 80.0);
        assert_eq!(load_window_position_for_display(label, 2), Some((1560.0, 80.0)));
        assert_eq!(load_window_position_for_display(label, 1), Some((120.0, 80.0)));
    }

    #[test]
    fn per_display_key_isolates_from_single_key() {
        // 按 display 存的 key（window_pos.{label}@{id}）不应污染单值 key（window_pos.{label}）
        let label = "test_window_per_display_isolate";
        save_window_position_for_display(label, 7, 200.0, 90.0);
        assert_eq!(
            load_window_position(label),
            None,
            "per-display 存不应影响单值 key"
        );
        // 反向：单值存不影响 per-display
        save_window_position(label, 300.0, 100.0);
        assert_eq!(
            load_window_position_for_display(label, 7),
            Some((200.0, 90.0)),
            "单值存不应影响 per-display key"
        );
    }

    #[test]
    fn get_mouse_location_does_not_panic_on_non_macos() {
        // 非 macOS 返回 None，不 panic（macOS 返回值不固定，只校验不 panic）
        let _ = get_mouse_location();
    }

    #[test]
    fn find_monitor_at_mouse_none_when_no_mouse() {
        // mouse=None 应返回 None（不 panic）
        assert_eq!(find_monitor_at_mouse(None), None);
    }
}
