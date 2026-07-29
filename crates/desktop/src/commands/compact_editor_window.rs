//! 统一内容查看器窗口：独立 Tauri 窗口，原生标题栏，1100×680 可调大小，居中。
//! 支持 tab 切换文本/图片/语音条目。窗口尺寸可调 + 记忆。
//!
//! 单例 + 关窗即销毁：open 时已存在则 show+focus（由 commands 层额外 emit load 推送新文本），
//! 否则创建。macOS：开窗切 Regular（Dock 显图标），关窗切回 Accessory，与 settings 对称。

use tauri::{WebviewUrl, WebviewWindowBuilder, Manager};

use crate::commands::compact_editor_commands::PendingTabFull;

const WIDTH: f64 = 1100.0;
const HEIGHT: f64 = 680.0;
const MIN_WIDTH: f64 = 600.0;
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

/// 读窗口状态记忆（DB app_config）。无记忆用默认 1100×680 居中。
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
        // inner_position() / inner_size() 返回物理像素，÷ scale 统一到逻辑像素。
        // 最大化时不再 unmaximize→read→remaximize（OS 窗口状态变更异步，
        // 同步读拿到的仍是最大化尺寸）——改存 maximize 标记，
        // 恢复时用上次非最大化记忆的 size/pos（如有），否则用默认值。
        let state = if maximized {
            // 读已保存的非最大化状态作为 fallback（不依赖 unmaximize 读尺寸）
            let prev = load_window_state();
            WindowState {
                width: if prev.width > 0.0 { prev.width } else { WIDTH },
                height: if prev.height > 0.0 { prev.height } else { HEIGHT },
                x: if prev.x != 0.0 || prev.y != 0.0 { prev.x } else { 100.0 },
                y: if prev.x != 0.0 || prev.y != 0.0 { prev.y } else { 100.0 },
                maximized: true,
            }
        } else if let (Ok(pos), Ok(size)) = (win.inner_position(), win.inner_size()) {
            let lw = size.width as f64 / scale;
            let lh = size.height as f64 / scale;
            let lx = pos.x as f64 / scale;
            let ly = pos.y as f64 / scale;
            WindowState {
                width: lw, height: lh, x: lx, y: ly,
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
    let mut should_maximize = false;
    log::info!("[compact-editor] window state {:?}", state);

    // URL 参数注入：首个 tab 的元数据 + 背景色 hex 拼入 URL query string。
    // 不注入 text——长文本（ASR 录音/大段粘贴）会导致 URL 超长使 WebView 白屏。
    // 前端 mount 后经 get_pending_compact_tabs 批量拉取含 text 的完整数据。
    let mut url = if let Some(p) = pending {
        let mut u = format!(
            "compact-editor.html?itemId={}&source={}&itemType={}",
            p.item_id, p.source, p.item_type
        );
        // 图片类型注入原始尺寸——前端 ImagePreview 首帧即有正确宽高，消除布局突变
        if p.item_type == "image" && p.img_width > 0 && p.img_height > 0 {
            u.push_str(&format!("&imgWidth={}&imgHeight={}", p.img_width, p.img_height));
        }
        u
    } else {
        "compact-editor.html".to_string()
    };
    // 背景色 hex 注入——compact-editor.html <head> 脚本同步设为 #hex，零 CSS 依赖消除白屏
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

    // 非最大化时设具体尺寸+位置；最大化时不设 position——
    // 让窗口在默认屏幕创建后 maximize（macOS 在窗口所在屏幕最大化）。
    // 之前最大化保存 x=0,y=0 → 恢复时 position(0,0) 可能在副屏 → maximize 停在副屏。
    if !state.maximized {
        if state.width > 0.0 && state.height > 0.0 {
            // 检测保存的位置是否在可见显示器范围内——多显示器拔接后坐标可能失效。
            let monitors = app_handle.available_monitors().unwrap_or_default();
            let visible = monitors.iter().any(|m| {
                let ms = m.scale_factor();
                let mx = m.position().x as f64 / ms;
                let my = m.position().y as f64 / ms;
                let mw = m.size().width as f64 / ms;
                let mh = m.size().height as f64 / ms;
                state.x >= mx - 50.0 && state.x <= mx + mw + 50.0
                    && state.y >= my - 50.0 && state.y <= my + mh + 50.0
            });
            if visible {
                builder = builder.inner_size(state.width, state.height).position(state.x, state.y);
            } else {
                log::info!("[compact-editor] saved position {},{} not visible, center", state.x, state.y);
                builder = builder.inner_size(state.width, state.height).center();
            }
        } else {
            builder = builder.inner_size(WIDTH, HEIGHT).center();
        }
        builder = builder.maximized(false);
    } else {
        // 最大化：用保存坐标所在的显示器创建接近全屏的大窗体，再 maximize。
        let monitors = app_handle.available_monitors().unwrap_or_default();
        for m in &monitors {
            let ms = m.scale_factor();
            log::info!("[compact-editor] monitor: pos={},{} size={}x{} scale={}",
                m.position().x, m.position().y, m.size().width, m.size().height, ms);
        }
        let monitor = monitors.iter().find(|m| {
            // state.x/y 是逻辑像素，显示器 position/size 是物理像素——统一到逻辑
            let ms = m.scale_factor();
            let mx = m.position().x as f64 / ms;
            let my = m.position().y as f64 / ms;
            let mw = m.size().width as f64 / ms;
            let mh = m.size().height as f64 / ms;
            state.x >= mx && state.x < mx + mw && state.y >= my && state.y < my + mh
        });

        if let Some(monitor) = monitor {
            // 坐标匹配到显示器——在该显示器创建大窗体 + maximize
            let scale = monitor.scale_factor();
            let mw = monitor.size().width as f64 / scale;
            let mh = monitor.size().height as f64 / scale;
            let mx = monitor.position().x as f64 / scale;
            let my = monitor.position().y as f64 / scale;
            let margin = 80.0;
            builder = builder
                .inner_size(mw - margin * 2.0, mh - margin * 1.5)
                .position(mx + margin, my + margin * 0.5);
            log::info!("[compact-editor] maximized: monitor {}x{} at {},{} → window {:.0}x{:.0} at {:.0},{:.0}",
                mw, mh, mx, my, mw - margin * 2.0, mh - margin * 1.5, mx + margin, my + margin * 0.5);
            should_maximize = true;
        } else if let Some(monitor) = app_handle.primary_monitor().ok().flatten() {
            // 保存的屏幕未连接——回退当前主屏最大化
            log::info!("[compact-editor] saved monitor not connected, fallback to primary");
            let scale = monitor.scale_factor();
            let mw = monitor.size().width as f64 / scale;
            let mh = monitor.size().height as f64 / scale;
            let mx = monitor.position().x as f64 / scale;
            let my = monitor.position().y as f64 / scale;
            let margin = 80.0;
            builder = builder
                .inner_size(mw - margin * 2.0, mh - margin * 1.5)
                .position(mx + margin, my + margin * 0.5);
            should_maximize = true;
        } else {
            // 极端情况（连主屏都拿不到）——默认大小居中
            builder = builder.inner_size(WIDTH, HEIGHT).center();
        }
    }

    let win = builder.build();
    log::info!("[compact-editor] after build, maximized={}", state.maximized);

    if let Ok(ref win) = win {
        let _ = win.show();
        let _ = win.set_focus();
        // show 后再 maximize——窗口已经是接近全屏的大尺寸，
        // maximize 的视觉变化极小，用户几乎感知不到
        if should_maximize {
            let _ = win.maximize();
        }
        log::info!("[compact-editor] after show");
    }
}

/// macOS: 统一查看器窗口关闭后，仅当无其他常规窗口存活时才切回 Accessory（仅托盘）。
/// 窗口状态保存已在 CloseRequested 里完成（Destroyed 时窗口已销毁）。
#[cfg(target_os = "macos")]
pub fn on_compact_editor_closed(app_handle: &tauri::AppHandle) {
    crate::platform::activation::restore_accessory_if_no_regular_window(app_handle);
}
