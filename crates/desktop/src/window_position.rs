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

#[cfg(test)]
mod tests {
    #[test]
    fn dock_state_round_trip() {
        let label = "test_window_dock_roundtrip";
        crate::window_position::save_dock_state(label, "right");
        let loaded = crate::window_position::load_dock_state(label);
        assert_eq!(loaded.as_deref(), Some("right"));

        crate::window_position::save_dock_state(label, "left");
        let loaded = crate::window_position::load_dock_state(label);
        assert_eq!(loaded.as_deref(), Some("left"));

        crate::window_position::save_dock_state(label, "none");
        let loaded = crate::window_position::load_dock_state(label);
        assert_eq!(loaded.as_deref(), Some("none"));
    }
}
