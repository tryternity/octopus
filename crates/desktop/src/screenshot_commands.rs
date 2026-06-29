use std::sync::Mutex;
use tauri::{Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};
use base64::{Engine, engine::general_purpose};
use octopus_clipboard::ClipboardHandle;

/// 截图数据副本（不含 monitor 坐标，仅像素数据用于裁剪）
#[derive(Clone)]
struct ScreenCaptureClone {
    rgba_bytes: Vec<u8>,
    width: u32,
    height: u32,
}

/// 所有显示器的截图数据，按 window label 索引。
static ALL_CAPTURES: Mutex<Vec<(String, ScreenCaptureClone)>> = Mutex::new(Vec::new());
/// 待处理的图片 base64，按 window label 索引（前端 mount 后拉取）。
static PENDING_IMAGES: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());
/// 窗口 ready 计数（前端报告 ready 后累加，达到总数后统一 show）。
static READY_COUNT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
static TOTAL_WINDOWS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// 注册截图全局快捷键。main 启动注册 + set_config 热重载共用，
/// 与 shortcut::register_shortcut / result_window::register_edit_global_shortcut 范式一致。
pub fn register_screenshot_shortcut(
    app: &tauri::AppHandle,
    shortcut_str: &str,
) -> Result<(), String> {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};
    let shortcut: Shortcut = shortcut_str
        .parse()
        .map_err(|e| format!("Failed to parse shortcut '{}': {}", shortcut_str, e))?;
    let app_handle = app.clone();
    app.global_shortcut()
        .on_shortcut(shortcut, move |_app, _scut, event| {
            if event.state() == ShortcutState::Pressed {
                let ah = app_handle.clone();
                tauri::async_runtime::spawn(async move {
                    let _ = start_screenshot(ah).await;
                });
            }
        })
        .map_err(|e| format!("Failed to register screenshot shortcut '{}': {}", shortcut_str, e))?;
    Ok(())
}

/// 启动截图：截所有显示器 → 每个显示器一个窗口
#[tauri::command]
pub async fn start_screenshot(app_handle: tauri::AppHandle) -> Result<(), String> {
    // 1. 截所有显示器
    let captures = octopus_capx::capture::capture_all_monitors()
        .map_err(|e| format!("截图失败: {}", e))?;

    // 3. 获取 Tauri 的显示器列表（逻辑坐标）
    let tauri_monitors = app_handle.available_monitors()
        .map_err(|e| format!("获取显示器失败: {}", e))?;

    // 清理旧数据 + 旧窗口
    ALL_CAPTURES.lock().unwrap().clear();
    PENDING_IMAGES.lock().unwrap().clear();
    READY_COUNT.store(0, std::sync::atomic::Ordering::SeqCst);
    TOTAL_WINDOWS.store(0, std::sync::atomic::Ordering::SeqCst);
    let old_labels: Vec<String> = app_handle
        .webview_windows()
        .keys()
        .filter(|k| k.starts_with("screenshot_"))
        .cloned()
        .collect();
    for label in &old_labels {
        if let Some(win) = app_handle.get_webview_window(label) {
            let _ = win.destroy();
        }
    }

    // session ID 确保窗口 label 唯一（无需 sleep 等待 destroy）
    let session_id = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);

    // 4. 按 Tauri monitor 匹配 xcap capture（用物理坐标精确匹配）
    for (i, tauri_mon) in tauri_monitors.iter().enumerate() {
        let phys_w = tauri_mon.size().width as f64;
        let phys_h = tauri_mon.size().height as f64;
        let scale = tauri_mon.scale_factor() as f64;
        let pos_x = tauri_mon.position().x as f64 / scale;  // 物理 → 逻辑
        let pos_y = tauri_mon.position().y as f64 / scale;
        let log_w = phys_w / scale;
        let log_h = phys_h / scale;

        // 用物理坐标匹配 xcap capture（避免双相同分辨率显示器匹配到同一个）
        let target_x = tauri_mon.position().x;
        let target_y = tauri_mon.position().y;
        let capture = captures.iter()
            .find(|c| c.monitor_x == target_x && c.monitor_y == target_y)
            .or_else(|| captures.iter().find(|c| c.width as f64 == phys_w && c.height as f64 == phys_h))
            .or_else(|| captures.get(i));

        let capture = match capture {
            Some(c) => c,
            None => continue,
        };

        let label = format!("screenshot_{}_{}", session_id, i);

        // RGBA → JPEG base64（截图背景只需视觉展示，JPEG 编码比 PNG 快 10×+）
        let img = ::image::RgbaImage::from_raw(capture.width, capture.height, capture.rgba_bytes.clone())
            .ok_or("图像处理失败")?;
        let mut jpg_bytes = Vec::new();
        let rgb_img = ::image::DynamicImage::ImageRgba8(img).into_rgb8();
        let mut jpg_encoder = ::image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpg_bytes, 85);
        jpg_encoder.encode(&rgb_img, rgb_img.width(), rgb_img.height(), ::image::ExtendedColorType::Rgb8)
            .map_err(|e| format!("JPEG 编码失败: {:?}", e))?;
        let b64 = general_purpose::STANDARD.encode(&jpg_bytes);

        // 暂存
        PENDING_IMAGES.lock().unwrap().push((label.clone(), b64));
        ALL_CAPTURES.lock().unwrap().push((label.clone(), ScreenCaptureClone {
            rgba_bytes: capture.rgba_bytes.clone(),
            width: capture.width,
            height: capture.height,
        }));

        // 串行创建窗口（同时创建多个全屏 WebView 会导致 macOS segfault）
        if i > 0 {
            std::thread::sleep(std::time::Duration::from_millis(150));
        }

        let window_result = WebviewWindowBuilder::new(
            &app_handle,
            &label,
            WebviewUrl::default(),
        )
        .title("")
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .visible(false)
        .position(pos_x, pos_y)
        .inner_size(log_w, log_h)
        .build();

        if let Err(e) = &window_result {
            log::error!("Failed to create screenshot window '{}': {}", label, e);
            continue;
        }

        TOTAL_WINDOWS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        log::info!(
            "Screenshot window '{}' at ({},{}) {}x{} (monitor phys {}x{}, scale {})",
            label, pos_x, pos_y, log_w, log_h, phys_w, phys_h, tauri_mon.scale_factor(),
        );
    }

    // 超时 fallback：3s 后如果仍有窗口未显示，强制全部显示（防死锁）
    {
        let ah = app_handle.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(3));
            let count = READY_COUNT.load(std::sync::atomic::Ordering::SeqCst);
            let total = TOTAL_WINDOWS.load(std::sync::atomic::Ordering::SeqCst);
            if count < total {
                log::warn!("Screenshot show timeout: {}/{} ready, force showing", count, total);
                show_all_screenshot_windows(&ah);
            }
        });
    }

    Ok(())
}

/// 截图 OCR：合成选区 → 入库 → OCR 识别 → 写 search_text + 剪贴板 + 新建文档
#[tauri::command]
pub async fn ocr_screenshot(
    png_base64: String,
    app_handle: tauri::AppHandle,
    handle: State<'_, std::sync::Arc<ClipboardHandle>>,
) -> Result<(), String> {
    let png_bytes = general_purpose::STANDARD.decode(&png_base64)
        .map_err(|e| format!("base64 解码失败: {}", e))?;

    ALL_CAPTURES.lock().unwrap().clear();
    PENDING_IMAGES.lock().unwrap().clear();

    // SHA-256 去重 → WebP → 入库
    let hash = octopus_clipboard::image::sha256_hex(&png_bytes);

    let existing = octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::find_by_content_hash(conn, &hash)
    }).map_err(|e| e.to_string())?;

    let item_id = if let Some(id) = existing {
        octopus_infra::db::with_db(|conn| {
            octopus_clipboard::store::touch_created_at(conn, id)
        }).map_err(|e| e.to_string())?;
        id
    } else {
        let img = ::image::load_from_memory(&png_bytes)
            .map_err(|e| format!("解码失败: {:?}", e))?;
        let crop_w = img.width();
        let crop_h = img.height();
        let encoded = octopus_clipboard::image::encode_to_webp(&img)
            .map_err(|e| format!("WebP 编码失败: {}", e))?;

        octopus_infra::db::with_db(|conn| {
            octopus_clipboard::store::insert_image_data(
                conn, &hash, &encoded.webp_blob, &encoded.thumb_blob,
                crop_w as i64, crop_h as i64,
            )
        }).map_err(|e| e.to_string())?;

        let id = octopus_clipboard::store::chrono_millis();
        octopus_infra::db::with_db(|conn| {
            octopus_clipboard::store::insert_clipboard_item(conn, &octopus_clipboard::store::NewClipboardItem {
                id,
                item_type: octopus_clipboard::ItemType::Image,
                content: hash.clone(),
                search_text: String::new(),
                created_at: octopus_clipboard::store::iso_now(),
                blob_hash: Some(hash),
                width: Some(crop_w as i64),
                height: Some(crop_h as i64),
                has_thumbnail: Some(1),
                file_count: None,
                is_rich: false,
            })
        }).map_err(|e| e.to_string())?;
        id
    };

    // OCR 识别
    let engine = octopus_ocr::engine::OcrEngine::instance()
        .map_err(|e| e.to_string())?;
    let text = engine.recognize(&png_bytes).map_err(|e| e.to_string())?;

    if !text.trim().is_empty() {
        // 写 search_text
        octopus_infra::db::with_db(|conn| {
            octopus_clipboard::store::update_search_text(conn, item_id, &text)
        }).map_err(|e| e.to_string())?;

        // 写剪贴板
        handle.write_text(&text).map_err(|e| e.to_string())?;

        // 新建文档
        open_text_editor_with_content(&text);
    }

    let _ = app_handle.emit("clipboard://changed", ());
    close_all_screenshot_windows(&app_handle);

    Ok(())
}

fn open_text_editor_with_content(text: &str) {
    #[cfg(target_os = "macos")]
    {
        let escaped = text.replace('\\', "\\\\").replace('"', "\\\"");
        let script = format!(
            r#"tell application "TextEdit"
    activate
    make new document with properties {{text:"{}"}}
end tell"#,
            escaped
        );
        let _ = std::process::Command::new("osascript")
            .args(["-e", &script])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
}

/// 前端渲染完成后调用。所有窗口都 ready 后统一 show（同步显示，避免逐个弹出）。
#[tauri::command]
pub fn show_screenshot_window(app_handle: tauri::AppHandle) {
    let count = READY_COUNT.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    let total = TOTAL_WINDOWS.load(std::sync::atomic::Ordering::SeqCst);
    if count >= total && total > 0 {
        show_all_screenshot_windows(&app_handle);
    }
}

fn show_all_screenshot_windows(app_handle: &tauri::AppHandle) {
    let labels: Vec<String> = app_handle
        .webview_windows()
        .keys()
        .filter(|k| k.starts_with("screenshot_"))
        .cloned()
        .collect();
    for label in &labels {
        if let Some(window) = app_handle.get_webview_window(label) {
            let _ = window.show();
        }
    }
    // 聚焦主显示器窗口（label 以 _0 结尾）
    let main_label = labels.iter().find(|l| l.ends_with("_0"));
    if let Some(ml) = main_label {
        if let Some(window) = app_handle.get_webview_window(ml) {
            let _ = window.set_focus();
        }
    }
}

/// 弹系统保存对话框，保存截图到用户指定路径
#[tauri::command]
pub async fn save_screenshot_dialog(
    png_base64: String,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    let png_bytes = general_purpose::STANDARD.decode(&png_base64)
        .map_err(|e| format!("base64 解码失败: {}", e))?;

    ALL_CAPTURES.lock().unwrap().clear();
    PENDING_IMAGES.lock().unwrap().clear();

    // 先关闭截图窗口，恢复正常屏幕，再弹保存对话框
    close_all_screenshot_windows(&app_handle);

    use tauri_plugin_dialog::DialogExt;
    let save_path = app_handle.dialog()
        .file()
        .add_filter("PNG 图片", &["png"])
        .set_file_name("screenshot.png")
        .blocking_save_file();

    if let Some(path) = save_path {
        let path = path.as_path().ok_or("无效路径")?;
        std::fs::write(path, &png_bytes).map_err(|e| e.to_string())?;
        log::info!("Screenshot saved to {}", path.display());
    }

    Ok(())
}

/// 前端合成标注+裁剪后，直接发送最终 PNG base64（含标注）
#[tauri::command]
pub async fn confirm_screenshot_with_data(
    _label: String,
    png_base64: String,
    _width: u32,
    _height: u32,
    app_handle: tauri::AppHandle,
    handle: State<'_, std::sync::Arc<ClipboardHandle>>,
) -> Result<(), String> {
    // 解码 base64 → PNG bytes
    let png_bytes = general_purpose::STANDARD.decode(&png_base64)
        .map_err(|e| format!("base64 解码失败: {}", e))?;

    // 清空所有暂存
    ALL_CAPTURES.lock().unwrap().clear();
    PENDING_IMAGES.lock().unwrap().clear();

    // SHA-256 去重
    let hash = octopus_clipboard::image::sha256_hex(&png_bytes);

    let existing = octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::find_by_content_hash(conn, &hash)
    }).map_err(|e| e.to_string())?;

    if let Some(id) = existing {
        octopus_infra::db::with_db(|conn| {
            octopus_clipboard::store::touch_created_at(conn, id)
        }).map_err(|e| e.to_string())?;
    } else {
        let img = ::image::load_from_memory(&png_bytes)
            .map_err(|e| format!("解码失败: {:?}", e))?;
        let crop_w = img.width();
        let crop_h = img.height();
        let encoded = octopus_clipboard::image::encode_to_webp(&img)
            .map_err(|e| format!("WebP 编码失败: {}", e))?;

        octopus_infra::db::with_db(|conn| {
            octopus_clipboard::store::insert_image_data(
                conn, &hash, &encoded.webp_blob, &encoded.thumb_blob,
                crop_w as i64, crop_h as i64,
            )
        }).map_err(|e| e.to_string())?;

        octopus_infra::db::with_db(|conn| {
            octopus_clipboard::store::insert_clipboard_item(conn, &octopus_clipboard::store::NewClipboardItem {
                id: octopus_clipboard::store::chrono_millis(),
                item_type: octopus_clipboard::ItemType::Image,
                content: hash.clone(),
                search_text: String::new(),
                created_at: octopus_clipboard::store::iso_now(),
                blob_hash: Some(hash),
                width: Some(crop_w as i64),
                height: Some(crop_h as i64),
                has_thumbnail: Some(1),
                file_count: None,
                is_rich: false,
            })
        }).map_err(|e| e.to_string())?;
    }

    handle.write_image(&png_bytes).map_err(|e| e.to_string())?;
    let _ = app_handle.emit("clipboard://changed", ());
    close_all_screenshot_windows(&app_handle);

    Ok(())
}
#[tauri::command]
pub fn get_screenshot_image(label: String) -> Result<serde_json::Value, String> {
    // 取出对应的 base64（克隆而非 remove，兼容 StrictMode 双 mount）
    let b64 = {
        let pending = PENDING_IMAGES.lock().unwrap();
        pending
            .iter()
            .find(|(l, _)| *l == label)
            .map(|(_, b64)| b64.clone())
    }
    .ok_or("无待处理截图数据")?;

    // 找到对应的截图尺寸
    let (w, h) = ALL_CAPTURES.lock().unwrap()
        .iter()
        .find(|(l, _)| *l == label)
        .map(|(_, c)| (c.width, c.height))
        .unwrap_or((0, 0));

    Ok(serde_json::json!({
        "image": b64,
        "width": w,
        "height": h,
    }))
}

/// 确认截图：从指定窗口的截图裁剪选区 → 写剪贴板历史 → 关所有窗口
#[tauri::command]
pub async fn confirm_screenshot(
    label: String,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    app_handle: tauri::AppHandle,
    handle: State<'_, std::sync::Arc<ClipboardHandle>>,
) -> Result<(), String> {
    // 1. 取对应窗口的全屏数据
    let full = {
        let mut all = ALL_CAPTURES.lock().unwrap();
        all.iter()
            .position(|(l, _)| *l == label)
            .map(|i| all.remove(i).1)
    }
    .ok_or("无截图数据")?;

    // 清空所有暂存
    ALL_CAPTURES.lock().unwrap().clear();
    PENDING_IMAGES.lock().unwrap().clear();

    // 2. 裁剪选区
    let fake_full = octopus_capx::capture::ScreenCapture {
        rgba_bytes: full.rgba_bytes.clone(),
        width: full.width,
        height: full.height,
        monitor_x: 0,
        monitor_y: 0,
    };
    let png_bytes = octopus_capx::capture::crop_region(&fake_full, x, y, w, h)
        .map_err(|e| format!("裁剪失败: {}", e))?;

    // 3. SHA-256 去重
    let hash = octopus_clipboard::image::sha256_hex(&png_bytes);

    // 4. 检查 DB 中是否已有此 hash
    let existing = octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::find_by_content_hash(conn, &hash)
    }).map_err(|e| e.to_string())?;

    if let Some(id) = existing {
        octopus_infra::db::with_db(|conn| {
            octopus_clipboard::store::touch_created_at(conn, id)
        }).map_err(|e| e.to_string())?;
    } else {
        // 5. 编码 WebP 无损 + 缩略图
        let img = ::image::load_from_memory(&png_bytes)
            .map_err(|e| format!("解码裁剪图失败: {:?}", e))?;
        let crop_w = img.width();
        let crop_h = img.height();
        let encoded = octopus_clipboard::image::encode_to_webp(&img)
            .map_err(|e| format!("WebP 编码失败: {}", e))?;

        // 6. 存 image_data BLOB
        octopus_infra::db::with_db(|conn| {
            octopus_clipboard::store::insert_image_data(
                conn, &hash, &encoded.webp_blob, &encoded.thumb_blob,
                crop_w as i64, crop_h as i64,
            )
        }).map_err(|e| e.to_string())?;

        // 7. 存 clipboard_history 条目
        octopus_infra::db::with_db(|conn| {
            octopus_clipboard::store::insert_clipboard_item(conn, &octopus_clipboard::store::NewClipboardItem {
                id: octopus_clipboard::store::chrono_millis(),
                item_type: octopus_clipboard::ItemType::Image,
                content: hash.clone(),
                search_text: String::new(),
                created_at: octopus_clipboard::store::iso_now(),
                blob_hash: Some(hash),
                width: Some(crop_w as i64),
                height: Some(crop_h as i64),
                has_thumbnail: Some(1),
                file_count: None,
                is_rich: false,
            })
        }).map_err(|e| e.to_string())?;
    }

    // 8. 写系统剪贴板（suppress flag）
    handle.write_image(&png_bytes).map_err(|e| e.to_string())?;

    // 9. 通知前端刷新
    let _ = app_handle.emit("clipboard://changed", ());

    // 10. 关闭所有截图窗口
    close_all_screenshot_windows(&app_handle);

    Ok(())
}

/// 取消截图：关所有窗口
#[tauri::command]
pub async fn cancel_screenshot(app_handle: tauri::AppHandle) -> Result<(), String> {
    ALL_CAPTURES.lock().unwrap().clear();
    PENDING_IMAGES.lock().unwrap().clear();
    close_all_screenshot_windows(&app_handle);
    Ok(())
}

fn close_all_screenshot_windows(app_handle: &tauri::AppHandle) {
    let labels: Vec<String> = app_handle
        .webview_windows()
        .keys()
        .filter(|k| k.starts_with("screenshot_"))
        .cloned()
        .collect();
    for label in &labels {
        if let Some(win) = app_handle.get_webview_window(label) {
            let _ = win.destroy();
        }
    }
}

// ── 滚动截图 ──

static SCROLL_RECORDING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[tauri::command]
pub async fn start_scroll_recording(
    x: f64, y: f64, w: f64, h: f64,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    SCROLL_RECORDING.store(true, std::sync::atomic::Ordering::SeqCst);

    let ah = app_handle.clone();
    tauri::async_runtime::spawn(async move {
        let scale = ah.primary_monitor()
            .ok()
            .flatten()
            .map(|m| m.scale_factor())
            .unwrap_or(1.0);

        let px = (x * scale) as u32;
        let py = (y * scale) as u32;
        let pw = (w * scale) as u32;
        let ph = (h * scale) as u32;

        // 隐藏截图窗口（让用户可以正常操作底层应用滚动）
        let scroll_labels: Vec<String> = ah
            .webview_windows()
            .keys()
            .filter(|k| k.starts_with("screenshot_"))
            .cloned()
            .collect();
        for label in &scroll_labels {
            if let Some(win) = ah.get_webview_window(label) {
                let _ = win.hide();
            }
        }

        // 首帧（spawn_blocking 避免阻塞 async runtime）
        let first_result = tokio::task::spawn_blocking(move || {
            let captures = octopus_capx::capture::capture_all_monitors()?;
            let full = captures.into_iter().next().ok_or_else(|| anyhow::anyhow!("no monitor"))?;
            let png = octopus_capx::capture::crop_region(&full, px, py, pw, ph)?;
            let img = image::load_from_memory(&png)?.to_rgba8();
            anyhow::Ok(img)
        }).await;

        let first_img = match first_result { Ok(Ok(img)) => img, _ => return };
        let mut stitcher = octopus_capx::stitch::Stitcher::new(first_img, Default::default());

        // 通知前端：开始录制
        let _ = ah.emit("scroll://started", ());

        let frame_duration = std::time::Duration::from_millis(100); // 10fps（降低 CPU 压力）
        let mut interval = tokio::time::interval(frame_duration);
        interval.tick().await;

        let ah2 = ah.clone();
        while SCROLL_RECORDING.load(std::sync::atomic::Ordering::SeqCst) {
            interval.tick().await;

            // spawn_blocking 截屏（避免阻塞 event loop 导致 ESC 无响应）
            let capture_result = tokio::task::spawn_blocking({
                let px = px; let py = py; let pw = pw; let ph = ph;
                move || {
                    let captures = octopus_capx::capture::capture_all_monitors()?;
                    let full = captures.into_iter().next().ok_or_else(|| anyhow::anyhow!("no monitor"))?;
                    let png = octopus_capx::capture::crop_region(&full, px, py, pw, ph)?;
                    let img = image::load_from_memory(&png)?.to_rgba8();
                    anyhow::Ok(img)
                }
            }).await;

            let frame_rgba = match capture_result { Ok(Ok(img)) => img, _ => continue };

            let added = stitcher.process_frame(&frame_rgba).unwrap_or(false);

            if added {
                let canvas = stitcher.canvas();
                let preview_w = 200u32;
                let preview_h = (preview_w * canvas.height() / canvas.width()).min(600);
                let preview = image::imageops::resize(canvas, preview_w, preview_h, image::imageops::FilterType::Nearest);
                let mut png_bytes = Vec::new();
                let _ = preview.write_to(&mut std::io::Cursor::new(&mut png_bytes), image::ImageFormat::Png);
                let b64 = general_purpose::STANDARD.encode(&png_bytes);
                let _ = ah2.emit("scroll://frame", serde_json::json!({
                    "image": b64,
                    "height": stitcher.height(),
                    "phys_height": (stitcher.height() as f64 / scale) as u32,
                }));
            }
        }

        // 录制结束：关闭截图窗口（不需要恢复，直接关闭）
        close_all_screenshot_windows(&ah);

        // 入库
        let canvas = stitcher.canvas().clone();
        let mut png_bytes = Vec::new();
        let _ = canvas.write_to(&mut std::io::Cursor::new(&mut png_bytes), image::ImageFormat::Png);

        let hash = octopus_clipboard::image::sha256_hex(&png_bytes);
        let img = match image::load_from_memory(&png_bytes) { Ok(i) => i, Err(_) => return };
        let encoded = match octopus_clipboard::image::encode_to_webp(&img) { Ok(e) => e, Err(_) => return };

        let item_id = octopus_clipboard::store::chrono_millis();
        let _ = octopus_infra::db::with_db(|conn| {
            octopus_clipboard::store::insert_image_data(conn, &hash, &encoded.webp_blob, &encoded.thumb_blob, img.width() as i64, img.height() as i64)
        });
        let _ = octopus_infra::db::with_db(|conn| {
            octopus_clipboard::store::insert_clipboard_item(conn, &octopus_clipboard::store::NewClipboardItem {
                id: item_id, item_type: octopus_clipboard::ItemType::Image,
                content: hash.clone(), search_text: String::new(),
                created_at: octopus_clipboard::store::iso_now(),
                blob_hash: Some(hash), width: Some(img.width() as i64),
                height: Some(img.height() as i64), has_thumbnail: Some(1),
                file_count: None, is_rich: false,
            })
        });

        let _ = ah.emit("scroll://done", serde_json::json!({ "id": item_id }));
        let _ = ah.emit("clipboard://changed", ());
    });

    Ok(())
}

#[tauri::command]
pub fn stop_scroll_recording() {
    SCROLL_RECORDING.store(false, std::sync::atomic::Ordering::SeqCst);
}
