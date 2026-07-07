//! 统一内容查看器窗口：独立 Tauri 窗口，原生标题栏，880×620 可调大小，居中。
//! 支持 tab 切换文本/图片/语音条目。窗口尺寸可调 + 记忆。
//!
//! 单例 + 关窗即销毁：open 时已存在则 show+focus（由 commands 层额外 emit load 推送新文本），
//! 否则创建。macOS：开窗切 Regular（Dock 显图标），关窗切回 Accessory，与 settings 对称。

use tauri::{WebviewUrl, WebviewWindowBuilder, Manager};

use crate::compact_editor_commands::PendingTabFull;

const WIDTH: f64 = 880.0;
const HEIGHT: f64 = 620.0;
const MIN_WIDTH: f64 = 480.0;
const MIN_HEIGHT: f64 = 360.0;
pub const WINDOW_LABEL: &str = "compact_editor_window";
const STATE_KEY: &str = "compact_editor_window_state";

#[derive(serde::Serialize, serde::Deserialize, Default, Clone, Debug)]
#[serde(rename_all = "camelCase")]
struct WindowState {
    width: f64,
    height: f64,
    x: f64,
    y: f64,
    maximized: bool,
}

/// 读窗口状态记忆（DB app_config）。无记忆用默认 880×620 居中。
fn load_window_state() -> WindowState {
    octopus_infra::db::load_config_key(STATE_KEY)
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// 写窗口状态记忆到 DB。
fn save_window_state(state: &WindowState) {
    if let Ok(json) = serde_json::to_string(state) {
        let _ = octopus_infra::db::save_config_key(STATE_KEY, &json);
    }
}

/// 关窗时调用：保存当前窗口位置/大小到 DB。
pub fn on_compact_editor_save_state(app_handle: &tauri::AppHandle) {
    if let Some(win) = app_handle.get_webview_window(WINDOW_LABEL) {
        let maximized = win.is_maximized().unwrap_or(false);
        let scale = win.scale_factor().unwrap_or(1.0);
        let state = if maximized {
            WindowState { width: WIDTH, height: HEIGHT, x: 0.0, y: 0.0, maximized: true }
        } else if let (Ok(pos), Ok(size)) = (win.inner_position(), win.inner_size()) {
            // inner_position + inner_size 对称保存恢复（都用内容区坐标，
            // 不含标题栏），消除 outer/inner 混用导致的坐标偏差。
            WindowState {
                width: size.width as f64 / scale,
                height: size.height as f64 / scale,
                x: pos.x as f64 / scale,
                y: pos.y as f64 / scale,
                maximized: false,
            }
        } else {
            return;
        };
        log::info!("[compact-editor] save window state {:?} (scale={})", state, scale);
        save_window_state(&state);
    }
}

/// 创建统一查看器窗口（调用方已确保当前不存在同名窗口）。
///
/// ⚠️ 必须在主线程调用：内含 macOS AppKit 主线程操作（set_activation_policy +
/// set_dock_icon，后者用 `MainThreadMarker::new_unchecked` 强制假定主线程）。
/// 从 async worker 线程同步调用会导致整个应用僵死。若需从 worker 触发建窗，
/// 用 `app_handle.run_on_main_thread(...)` 投递（见 screenshot_commands::ocr_screenshot）。
pub fn create_compact_editor_window(app_handle: &tauri::AppHandle, pending: Option<&PendingTabFull>) {
    log::info!("[compact-editor] create start");
    // macOS：编辑窗口切 Regular 让 Dock 显示图标（与 settings 一致）。
    #[cfg(target_os = "macos")]
    {
        log::info!("[compact-editor] before set_activation_policy(Regular)");
        let _ = app_handle.set_activation_policy(tauri::ActivationPolicy::Regular);
        log::info!("[compact-editor] after set_activation_policy(Regular)");
        log::info!("[compact-editor] before set_dock_icon");
        crate::settings_window::set_dock_icon();
        log::info!("[compact-editor] after set_dock_icon");
    }

    let state = load_window_state();
    log::info!("[compact-editor] window state {:?}", state);

    // URL 参数注入：首个 tab 的数据 + 背景色 hex 拼入 URL query string，
    // 前端首次渲染时同步读取（零 IPC 打开）+ 首帧即有正确背景色（零 CSS 依赖）。
    let mut url = if let Some(p) = pending {
        let encoded_text = urlencode(&p.text);
        let mut u = format!(
            "index.html?itemId={}&source={}&itemType={}&text={}",
            p.item_id, p.source, p.item_type, encoded_text
        );
        // 图片类型注入原始尺寸——前端 ImagePreview 首帧即有正确宽高，消除布局突变
        if p.item_type == "image" && p.img_width > 0 && p.img_height > 0 {
            u.push_str(&format!("&imgWidth={}&imgHeight={}", p.img_width, p.img_height));
        }
        u
    } else {
        "index.html".to_string()
    };
    // 背景色 hex 注入——index.html <head> 脚本同步设为 #hex，零 CSS 依赖消除白屏
    if let Some(bg) = crate::theme::window_bg_hex(WINDOW_LABEL) {
        let sep = if url.contains('?') { "&" } else { "?" };
        url.push_str(&format!("{}bg={}", sep, bg));
    }

    let mut builder = WebviewWindowBuilder::new(
        app_handle,
        WINDOW_LABEL,
        WebviewUrl::App(url.into()),
    )
    .title("查看")
    .min_inner_size(MIN_WIDTH, MIN_HEIGHT)
    .decorations(true)
    .resizable(true)
    .visible(false);  // 先隐藏——所有配置（最大化/尺寸/位置）就绪后再 show

    // 非最大化时才设具体尺寸+位置——最大化状态由 builder.maximized(true) 直接生效，
    // 避免设了尺寸再被 maximize 覆盖的冗余布局计算。
    if !state.maximized {
        if state.width > 0.0 && state.height > 0.0 {
            // 检测保存的位置是否在可见显示器范围内——多显示器拔接后坐标可能失效。
            let monitors = app_handle.available_monitors().unwrap_or_default();
            let visible = monitors.iter().any(|m| {
                let mx = m.position().x as f64;
                let my = m.position().y as f64;
                let mw = m.size().width as f64 / m.scale_factor();
                let mh = m.size().height as f64 / m.scale_factor();
                state.x >= mx - 50.0 && state.x <= mx + mw + 50.0
                    && state.y >= my - 50.0 && state.y <= my + mh + 50.0
            });
            if visible {
                builder = builder.inner_size(state.width, state.height).position(state.x, state.y);
            } else {
                // 坐标不在任何显示器内（如副屏拔了）—— fallback 到居中
                log::info!("[compact-editor] saved position {},{} not visible, center", state.x, state.y);
                builder = builder.inner_size(state.width, state.height).center();
            }
        } else {
            builder = builder.inner_size(WIDTH, HEIGHT).center();
        }
        builder = builder.maximized(false);
    } else {
        builder = builder.maximized(true);
    }

    let win = builder.build();
    log::info!("[compact-editor] after build");

    // 所有配置（最大化/尺寸/位置）就绪后才 show——消除"先中间再放大"的过渡感
    if let Ok(ref win) = win {
        let _ = win.show();
    }
}

/// macOS: 统一查看器窗口关闭后，仅当无其他常规窗口存活时才切回 Accessory（仅托盘）。
/// 窗口状态保存已在 CloseRequested 里完成（Destroyed 时窗口已销毁）。
#[cfg(target_os = "macos")]
pub fn on_compact_editor_closed(app_handle: &tauri::AppHandle) {
    crate::activation::restore_accessory_if_no_regular_window(app_handle);
}

/// URL 百分号编码（用于把 text 内容安全拼入 URL query string）。
fn urlencode(s: &str) -> String {
    let mut result = String::with_capacity(s.len() * 3);
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(byte as char);
            }
            _ => {
                result.push_str(&format!("%{:02X}", byte));
            }
        }
    }
    result
}
