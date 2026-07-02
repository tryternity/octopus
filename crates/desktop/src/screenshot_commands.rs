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
/// 截图是否处于活动状态（有截图窗口存在）
#[allow(dead_code)]
pub fn is_screenshot_active() -> bool {
    TOTAL_WINDOWS.load(std::sync::atomic::Ordering::SeqCst) > 0
        || SCROLL_RECORDING.load(std::sync::atomic::Ordering::SeqCst)
}

#[tauri::command]
pub async fn start_screenshot(app_handle: tauri::AppHandle) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    save_frontmost_app();

    crate::tray::update_tray_screenshot_label(true);


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
        .transparent(true)
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

        // 新建笔记并打开记事本
        open_notepad_with_content(&app_handle, &text);
    }

    let _ = app_handle.emit("clipboard://changed", ());
    close_all_screenshot_windows(&app_handle);

    Ok(())
}

/// 创建 OCR 文本笔记并打开记事本窗口。
fn open_notepad_with_content(app_handle: &tauri::AppHandle, text: &str) {
    // OCR 是纯文本，type=text 直存原文（不再 <p> 包裹成 html）
    let ah = app_handle.clone();
    let text_owned = text.to_string();
    tauri::async_runtime::spawn(async move {
        let _ = crate::note_commands::create_note(
            "Ocr".to_string(),
            None,
            text_owned,
            "text".to_string(),
            ah.clone(),
        ).await;
        crate::notepad_window::open_notepad(ah);
    });
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

/// 贴图到桌面：裁剪选区 → 创建原生浮动窗口显示截图
#[tauri::command]
pub async fn pin_screenshot(
    label: String,
    x: f64, y: f64, w: f64, h: f64,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    let full = {
        let mut all = ALL_CAPTURES.lock().unwrap();
        all.iter()
            .position(|(l, _)| *l == label)
            .map(|i| all.remove(i).1)
    }
    .ok_or("无截图数据")?;

    ALL_CAPTURES.lock().unwrap().clear();
    PENDING_IMAGES.lock().unwrap().clear();

    let sel_win = app_handle
        .get_webview_window(&label)
        .ok_or("截图窗口不存在")?;

    #[cfg(target_os = "macos")]
    {
        let scale = sel_win.scale_factor().unwrap_or(1.0) as f64;

        let fake_full = octopus_capx::capture::ScreenCapture {
            rgba_bytes: full.rgba_bytes.clone(),
            width: full.width,
            height: full.height,
            monitor_x: 0,
            monitor_y: 0,
        };
        let png_bytes = octopus_capx::capture::crop_region(
            &fake_full,
            (x * scale) as u32,
            (y * scale) as u32,
            (w * scale) as u32,
            (h * scale) as u32,
        )
        .map_err(|e| format!("裁剪失败: {}", e))?;

        let (pin_x, pin_y) = if let Some((cx, cy, _cw, ch)) = get_window_cocoa_frame(&sel_win) {
            (cx + x, cy + ch - y - h)
        } else {
            (x, y)
        };

        let (tx, rx) = std::sync::mpsc::channel();
        let _ = sel_win.run_on_main_thread(move || {
            <crate::pin_window::MacPinWindow as crate::pin_window::PinWindow>::create(
                &png_bytes, pin_x, pin_y, w, h,
            );
            let _ = tx.send(());
        });
        let _ = rx.recv_timeout(std::time::Duration::from_secs(2));
    }

    #[cfg(not(target_os = "macos"))]
    {
        log::warn!("贴图功能仅支持 macOS");
        let _ = (x, y, w, h, full);
    }

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
    TOTAL_WINDOWS.store(0, std::sync::atomic::Ordering::SeqCst);
    crate::tray::update_tray_screenshot_label(false);
}

// ── 滚动截图 ──

/// 用户停止时的操作模式：保存文件 / 复制入库 / 取消
#[repr(u8)]
#[derive(Clone, Copy, PartialEq)]
enum ScrollStopMode {
    Copy = 0,
    Save = 1,
    Cancel = 2,
}

static SCROLL_STOP_MODE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

static SCROLL_RECORDING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(target_os = "macos")]
struct SendApp(objc2::rc::Retained<objc2_app_kit::NSRunningApplication>);
unsafe impl Send for SendApp {}
unsafe impl Sync for SendApp {}

#[cfg(target_os = "macos")]
static PREV_ACTIVE_APP: Mutex<Option<SendApp>> = Mutex::new(None);

#[cfg(target_os = "macos")]
fn save_frontmost_app() {
    use objc2_app_kit::{NSWorkspace, NSRunningApplication};
    let workspace = NSWorkspace::sharedWorkspace();
    if let Some(app) = workspace.frontmostApplication() {
        let curr = NSRunningApplication::currentApplication();
        let is_current = app.processIdentifier() == curr.processIdentifier();
        if !is_current {
            if let Some(name) = app.localizedName() {
                log::info!("Scroll screenshot: saved frontmost app '{}'", name.to_string());
            }
            let mut guard = PREV_ACTIVE_APP.lock().unwrap();
            *guard = Some(SendApp(app));
        } else {
            log::info!("Scroll screenshot: ignored saving current app");
        }
    }
}

#[cfg(target_os = "macos")]
fn activate_prev_app(win: &tauri::WebviewWindow) {
    let app_opt = {
        let guard = PREV_ACTIVE_APP.lock().unwrap();
        guard.as_ref().map(|p| p.0.clone())
    };
    let _ = win.run_on_main_thread(move || {
        if let Some(app) = app_opt {
            let success = app.activateWithOptions(objc2_app_kit::NSApplicationActivationOptions(1 << 1));
            log::info!("Scroll screenshot: activated previous app on main thread, success={}", success);
        } else {
            log::info!("Scroll screenshot: no previous app to activate, deactivating ourselves");
            if let Some(mtm) = objc2::MainThreadMarker::new() {
                let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
                app.deactivate();
            }
        }
    });
}

/// macOS：获取指定坐标下最上层非截图应用的 window owner PID。
#[cfg(target_os = "macos")]
fn get_window_pid_at_point(x: f64, y: f64) -> Option<i32> {
    use core_graphics::display::CGDisplay;
    let windows = CGDisplay::window_list_info(
        core_graphics::display::kCGWindowListOptionOnScreenOnly,
        None,
    )?;
    let curr_pid = std::process::id() as i32;

    use core_foundation::base::{CFTypeRef, TCFType};
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::string::CFString;
    use core_foundation::number::CFNumber;

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

        // 检查窗口 bounds 是否包含该点
        let key_bounds = CFString::new("kCGWindowBounds");
        let bounds_item = dict.get(&key_bounds);
        let bounds_ptr: CFTypeRef = *bounds_item;
        if bounds_ptr.is_null() { continue; }
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
    None
}

/// macOS：通过 PID 激活应用（主线程执行）。
#[cfg(target_os = "macos")]
fn activate_app_by_pid(ah: &tauri::AppHandle, pid: i32) {
    use objc2_app_kit::NSRunningApplication;
    // 通过任意窗口的 run_on_main_thread 在主线程执行激活
    if let Some(win) = ah.webview_windows().values().next() {
        let _ = win.run_on_main_thread(move || {
            if let Some(app) = NSRunningApplication::runningApplicationWithProcessIdentifier(pid) {
                let success = app.activateWithOptions(objc2_app_kit::NSApplicationActivationOptions(1 << 1));
                if success {
                    eprintln!("[scroll] activated app pid={} for scroll focus", pid);
                }
            }
        });
    }
}

/// macOS：获取 NSWindow 的 windowNumber（用于 CGWindowListCreateImage 排除 overlay 窗口）
#[cfg(target_os = "macos")]
fn get_window_number(win: &tauri::WebviewWindow) -> Option<u32> {
    let ptr = win.ns_window().ok()?;
    if ptr.is_null() { return None; }
    let ns_win = unsafe { &*(ptr as *const objc2_app_kit::NSWindow) };
    Some(ns_win.windowNumber() as u32)
}

#[cfg(not(target_os = "macos"))]
fn get_window_number(_win: &tauri::WebviewWindow) -> Option<u32> { None }

#[cfg(target_os = "macos")]
fn get_primary_screen_height() -> f64 {
    use objc2::{class, msg_send, runtime::AnyObject};
    unsafe {
        let screens: *mut AnyObject = msg_send![class!(NSScreen), screens];
        let primary: *mut AnyObject = msg_send![screens, objectAtIndex: 0];
        let frame: objc2_foundation::NSRect = msg_send![primary, frame];
        frame.size.height as f64
    }
}

#[cfg(target_os = "macos")]
fn get_window_cocoa_frame(win: &tauri::WebviewWindow) -> Option<(f64, f64, f64, f64)> {
    use objc2::{msg_send, runtime::AnyObject};
    let ptr = win.ns_window().ok()?;
    if ptr.is_null() { return None; }
    
    let rect: objc2_foundation::NSRect = unsafe { msg_send![ptr as *mut AnyObject, frame] };
    Some((rect.origin.x as f64, rect.origin.y as f64, rect.size.width as f64, rect.size.height as f64))
}

#[cfg(target_os = "macos")]
fn set_window_ignores_mouse_events(win: &tauri::WebviewWindow, ignore: bool) {
    let win_clone = win.clone();
    let label = win.label().to_string();
    let _ = win.run_on_main_thread(move || {
        if let Ok(ptr) = win_clone.ns_window() {
            if !ptr.is_null() {
                let ns_win = unsafe { &*(ptr as *const objc2_app_kit::NSWindow) };
                ns_win.setIgnoresMouseEvents(ignore);
                log::info!("[scroll-diag] NSWindow '{}' setIgnoresMouseEvents({}) completed on main thread", label, ignore);
            }
        }
    });
}

#[cfg(not(target_os = "macos"))]
fn set_window_ignores_mouse_events(win: &tauri::WebviewWindow, ignore: bool) {
    let _ = win.set_ignore_cursor_events(ignore);
}





#[cfg(target_os = "macos")]
fn set_app_active_on_main(win: &tauri::WebviewWindow, active: bool) {
    use objc2_app_kit::NSApplication;
    use objc2::MainThreadMarker;
    use std::sync::mpsc;
    let (tx, rx) = mpsc::channel();
    let _ = win.run_on_main_thread(move || {
        if let Some(mtm) = MainThreadMarker::new() {
            let app = NSApplication::sharedApplication(mtm);
            if active {
                #[allow(deprecated)]
                app.activateIgnoringOtherApps(true);
            } else {
                app.deactivate();
            }
        }
        let _ = tx.send(());
    });
    let _ = rx.recv_timeout(std::time::Duration::from_secs(1));
}

/// macOS: 模拟一次垂直滚轮事件（像素级）
#[cfg(target_os = "macos")]
#[allow(dead_code)]
fn send_scroll(delta: i32) {
    use core_graphics::event::{CGEvent, ScrollEventUnit, CGEventTapLocation};
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
    if let Ok(source) = CGEventSource::new(CGEventSourceStateID::HIDSystemState) {
        if let Ok(event) = CGEvent::new_scroll_event(
            source, ScrollEventUnit::PIXEL, 1, delta, 0, 0,
        ) {
            event.post(CGEventTapLocation::Session);
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn send_scroll(_delta: i32) {}

/// 前端传递的交互区域（工具栏、预览窗等），窗口局部逻辑坐标。
#[derive(Debug, Clone, serde::Deserialize)]
pub struct InteractiveRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

#[tauri::command]
pub async fn start_scroll_recording(
    x: f64, y: f64, w: f64, h: f64,
    win_label: String,
    interactive_rects: Vec<InteractiveRect>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    SCROLL_RECORDING.store(true, std::sync::atomic::Ordering::SeqCst);

    let ah = app_handle.clone();
    tauri::async_runtime::spawn(async move {
        // ── 通过 win_label 定位选区所在的截图窗口（spec §6.4）──
        let sel_win = match ah.get_webview_window(&win_label) {
            Some(w) => w,
            None => {
                log::error!("start_scroll_recording: window '{}' not found", win_label);
                return;
            }
        };

        // 窗口原点：用 CGDisplay::bounds() 获取 Quartz 逻辑原点（最可靠）。
        // 截图窗口全屏覆盖显示器，所以窗口原点 = 显示器逻辑原点。
        // outer_position()/sf 在混合 DPI 下可能不准（Tauri 物理 vs Quartz 逻辑断层）。
        #[cfg(target_os = "macos")]
        let (win_origin_x, win_origin_y) = {
            let primary_h = get_primary_screen_height();
            if let Some((cx, cy, _, ch)) = get_window_cocoa_frame(&sel_win) {
                (cx, primary_h - (cy + ch))
            } else {
                (0.0, 0.0)
            }
        };
        #[cfg(not(target_os = "macos"))]
        let (win_origin_x, win_origin_y) = {
            let sf = sel_win.scale_factor().unwrap_or(1.0);
            match sel_win.outer_position() {
                Ok(p) => (p.x as f64 / sf, p.y as f64 / sf),
                Err(_) => (0.0, 0.0),
            }
        };
        eprintln!("[scroll] win_origin=({},{}) sel_local=({},{},{},{})", win_origin_x, win_origin_y, x, y, w, h);
        // 选区的全局逻辑坐标 = 窗口原点 + CSS 偏移
        let sel_global_x = win_origin_x + x;
        let sel_global_y = win_origin_y + y;

        // ── 找到选区所在的显示器 + scale ──
        let monitors = ah.available_monitors().unwrap_or_default();
        let (scale, mon_logical_x, mon_logical_y, _mon_phys_x, _mon_phys_y): (f64, f64, f64, i32, i32) = {
            let hit = monitors.iter().find(|m| {
                let mx = m.position().x as f64 / m.scale_factor();
                let my = m.position().y as f64 / m.scale_factor();
                let mw = m.size().width as f64 / m.scale_factor();
                let mh = m.size().height as f64 / m.scale_factor();
                let cx = sel_global_x + w / 2.0;
                let cy = sel_global_y + h / 2.0;
                cx >= mx && cx < mx + mw && cy >= my && cy < my + mh
            }).or_else(|| monitors.first());
            match hit {
                Some(m) => {
                    let sf = m.scale_factor();
                    (sf, m.position().x as f64 / sf, m.position().y as f64 / sf, m.position().x, m.position().y)
                }
                None => (1.0, 0.0, 0.0, 0, 0),
            }
        };

        // 选区在该显示器内的物理像素偏移
        let px = ((sel_global_x - mon_logical_x) * scale) as u32;
        let py = ((sel_global_y - mon_logical_y) * scale) as u32;
        let pw = (w * scale) as u32;
        let ph = (h * scale) as u32;

        log::info!(
            "Scroll recording: win_label={}, sel=({},{},{},{}), global=({},{},{}), scale={}, crop phys=({},{},{},{})",
            win_label, x, y, w, h, sel_global_x, sel_global_y, scale, scale,
            px, py, pw, ph,
        );

        // ── macOS：获取 display_id + overlay windowNumber（spec §6.4 CGWindowList 排除）──
        #[cfg(target_os = "macos")]
        let (_display_id, exclude_wid, target_wid) = {
            use core_graphics::display::CGDisplay;
            let displays = match CGDisplay::active_displays() {
                Ok(d) => d,
                Err(_) => { log::error!("CGGetActiveDisplayList failed"); return; }
            };
            let hit = displays.iter().find(|&&id| {
                let bounds = CGDisplay::new(id).bounds();
                let cx = sel_global_x + w / 2.0;
                let cy = sel_global_y + h / 2.0;
                cx >= bounds.origin.x && cx < bounds.origin.x + bounds.size.width
                    && cy >= bounds.origin.y && cy < bounds.origin.y + bounds.size.height
            }).copied().unwrap_or(0);
            let wid = get_window_number(&sel_win).unwrap_or(0);

            // Find target window ID from the app under the selection area (not PREV_ACTIVE_APP).
            // 用选区中心点检测下方的应用窗口，确保截到的是选区下方的真实内容。
            let target_wid = {
                let cx = sel_global_x + w / 2.0;
                let cy = sel_global_y + h / 2.0;
                if let Some(pid) = get_window_pid_at_point(cx, cy) {
                    let found = octopus_capx::capture::find_window_id_by_pid(pid);
                    log::info!("Scroll capture: app under selection (pid={}) yielded window ID {:?}", pid, found);
                    found
                } else {
                    None
                }
            };

            eprintln!("[scroll-diag] display_id={}, exclude_wid={} (windowNumber), target_wid={:?}, displays={:?}",
                hit, wid, target_wid, displays);
            (hit, wid, target_wid)
        };

        // 获取所有截图窗口 label（用于 set_ignore_cursor_events）
        let scroll_labels: Vec<String> = ah
            .webview_windows()
            .keys()
            .filter(|k| k.starts_with("screenshot_"))
            .cloned()
            .collect();

        // 录制开始：保持 always_on_top(true) + set_ignore_cursor_events(true) + deactivate
        #[cfg(target_os = "macos")]
        {
            for label in &scroll_labels {
                if let Some(win) = ah.get_webview_window(label) {
                    let _ = win.set_always_on_top(true);
                }
            }
        }
        for label in &scroll_labels {
            if let Some(win) = ah.get_webview_window(label) {
                set_window_ignores_mouse_events(&win, true);
            }
        }
        #[cfg(target_os = "macos")]
        {
            activate_prev_app(&sel_win);
            eprintln!("[scroll] manual mode: activated previous app for scroll passthrough");
            // Wait 120ms for window activation transition to complete and repaint in active state
            tokio::time::sleep(std::time::Duration::from_millis(120)).await;
        }

        // 独立鼠标监听线程：30ms 高频轮询，与截图循环解耦。
        // 鼠标在任意交互区域（工具栏/预览窗）→ set_ignore_cursor_events(false)（可点击）；
        // 离开 → set_ignore_cursor_events(true)（滚动穿透）。不调 activate/deactivate。
        let mon_labels = scroll_labels.clone();
        let mon_ah = ah.clone();
        let mon_winx = win_origin_x;
        let mon_winy = win_origin_y;
        let mon_rects = interactive_rects;
        tauri::async_runtime::spawn(async move {
            use core_graphics::event::CGEvent;
            use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
            let mut poll = tokio::time::interval(std::time::Duration::from_millis(16));
            let mut cur_passthrough = true;
            let mut last_active_pid: i32 = 0;
            let mut activate_check_count = 0u32;
            while SCROLL_RECORDING.load(std::sync::atomic::Ordering::SeqCst) {
                poll.tick().await;
                let (mouse_x, mouse_y) = if let Ok(src) = CGEventSource::new(CGEventSourceStateID::HIDSystemState) {
                    if let Ok(evt) = CGEvent::new(src) {
                        let loc = evt.location();
                        (loc.x, loc.y)
                    } else { (0.0, 0.0) }
                } else { (0.0, 0.0) };

                let lx = mouse_x - mon_winx;
                let ly = mouse_y - mon_winy;
                let in_interactive = mon_rects.iter().any(|r| {
                    lx >= r.x && lx <= r.x + r.width && ly >= r.y && ly <= r.y + r.height
                });
                let want = !in_interactive;
                if want != cur_passthrough {
                    for label in &mon_labels {
                        if let Some(win) = mon_ah.get_webview_window(label) {
                            set_window_ignores_mouse_events(&win, want);
                        }
                    }
                    cur_passthrough = want;
                }

                // 每 ~500ms（每 17 个 tick）检测鼠标下方的应用，如果未激活则激活它。
                // 这样用户不需要先点击目标应用，直接在选区内滚动即可。
                activate_check_count += 1;
                if want && activate_check_count >= 17 {
                    activate_check_count = 0;
                    #[cfg(target_os = "macos")]
                    {
                        if let Some(pid) = get_window_pid_at_point(mouse_x, mouse_y) {
                            if pid != last_active_pid {
                                activate_app_by_pid(&mon_ah, pid);
                                last_active_pid = pid;
                            }
                        }
                    }
                }
            }
        });


        // ── 首帧（只截选区区域，排除 overlay 窗口）──
        let target_wid_first = target_wid;
        let first_result = tokio::task::spawn_blocking(move || {
            #[cfg(target_os = "macos")]
            {
                let cap = if let Some(wid) = target_wid_first {
                    octopus_capx::capture::capture_window_region(
                        wid, sel_global_x, sel_global_y, w, h,
                    )?
                } else {
                    octopus_capx::capture::capture_region_excluding_window(
                        exclude_wid, sel_global_x, sel_global_y, w, h,
                    )?
                };
                let img = image::RgbaImage::from_raw(cap.width, cap.height, cap.rgba_bytes)
                    .ok_or_else(|| anyhow::anyhow!("failed to create RgbaImage"))?;
                anyhow::Ok(img)
            }
            #[cfg(not(target_os = "macos"))]
            {
                let captures = octopus_capx::capture::capture_all_monitors()?;
                let full = captures.iter()
                    .find(|c| c.monitor_x == mon_phys_x && c.monitor_y == mon_phys_y)
                    .or_else(|| captures.first())
                    .ok_or_else(|| anyhow::anyhow!("no matching monitor"))?;
                let png = octopus_capx::capture::crop_region(full, px, py, pw, ph)?;
                let img = image::load_from_memory(&png)?.to_rgba8();
                anyhow::Ok(img)
            }
        }).await;

        let first_img = match first_result { Ok(Ok(img)) => img, _ => return };
        let mut stitcher = octopus_capx::stitch::Stitcher::new(first_img, Default::default());

        let _ = ah.emit("scroll://started", ());

        let frame_duration = std::time::Duration::from_millis(30);
        let mut interval = tokio::time::interval(frame_duration);
        interval.tick().await;

        let ah2 = ah.clone();
        let mut no_progress_count = 0u32;
        let mut last_frame: Option<image::RgbaImage> = None;

        // manual 模式：由用户手动滚动触控板/滚轮，后台只进行高频截帧与拼接
        while SCROLL_RECORDING.load(std::sync::atomic::Ordering::SeqCst) {
            interval.tick().await;

            // 截屏：只截选区区域，CGWindowList 排除 overlay 窗口（只截底层应用内容）
            let target_wid_loop = target_wid;
            let capture_result = tokio::task::spawn_blocking(move || {
                #[cfg(target_os = "macos")]
                {
                    let cap = if let Some(wid) = target_wid_loop {
                        octopus_capx::capture::capture_window_region(
                            wid, sel_global_x, sel_global_y, w, h,
                        )?
                    } else {
                        octopus_capx::capture::capture_region_excluding_window(
                            exclude_wid, sel_global_x, sel_global_y, w, h,
                        )?
                    };
                    let img = image::RgbaImage::from_raw(cap.width, cap.height, cap.rgba_bytes)
                        .ok_or_else(|| anyhow::anyhow!("failed to create RgbaImage"))?;
                    anyhow::Ok(img)
                }
                #[cfg(not(target_os = "macos"))]
                {
                    let captures = octopus_capx::capture::capture_all_monitors()?;
                    let full = captures.iter()
                        .find(|c| c.monitor_x == mon_phys_x && c.monitor_y == mon_phys_y)
                        .or_else(|| captures.first())
                        .ok_or_else(|| anyhow::anyhow!("no matching monitor"))?;
                    let png = octopus_capx::capture::crop_region(full, px, py, pw, ph)?;
                    let img = image::load_from_memory(&png)?.to_rgba8();
                    anyhow::Ok(img)
                }
            }).await;

            let frame_rgba = match capture_result { Ok(Ok(img)) => img, _ => continue };
            last_frame = Some(frame_rgba.clone());

            // 选区实时画面 JPEG
            let mut frame_jpg = Vec::new();
            let frame_rgb = image::DynamicImage::ImageRgba8(frame_rgba.clone()).into_rgb8();
            let mut jpg_enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut frame_jpg, 80);
            let _ = jpg_enc.encode(&frame_rgb, frame_rgb.width(), frame_rgb.height(), image::ExtendedColorType::Rgb8);
            let frame_b64 = general_purpose::STANDARD.encode(&frame_jpg);

            let added = stitcher.process_frame(&frame_rgba).unwrap_or(false);

            // 自动停止检测：连续无新内容超过 ~6 秒（200 帧 @ 30ms 间隔）
            // 判定滚到底部或用户已停止，自动结束拼接
            if added {
                no_progress_count = 0;
            } else {
                no_progress_count += 1;
                if no_progress_count >= 200 {
                    log::info!("Scroll: no progress for {} frames (~6s), auto-stopping", no_progress_count);
                    break;
                }
            }

            // 预览：只取画布底部最新区域，让用户看到最新的拼接内容
            let canvas = stitcher.canvas();
            let preview_w = 400u32;
            let max_preview_h = 1200u32;
            // 计算等效预览高度对应的源像素高度（考虑缩放比）
            let src_h = (canvas.height() as u64 * canvas.width() as u64 / preview_w as u64).min(canvas.height() as u64) as u32;
            let crop_src_h = src_h.min(max_preview_h * canvas.width() / preview_w).min(canvas.height());
            let crop_y = canvas.height() - crop_src_h;
            let canvas_cropped = image::imageops::crop_imm(canvas, 0, crop_y, canvas.width(), crop_src_h).to_image();
            let preview_h = (preview_w * canvas_cropped.height() / canvas_cropped.width()).min(max_preview_h);
            let preview = image::imageops::resize(&canvas_cropped, preview_w, preview_h, image::imageops::FilterType::CatmullRom);
            let mut preview_png = Vec::new();
            let _ = preview.write_to(&mut std::io::Cursor::new(&mut preview_png), image::ImageFormat::Png);
            let preview_b64 = general_purpose::STANDARD.encode(&preview_png);

            let _ = ah2.emit("scroll://frame", serde_json::json!({
                "frame": frame_b64,
                "preview": preview_b64,
                "height": stitcher.height(),
                "phys_height": (stitcher.height() as f64 / scale) as u32,
            }));
        }

        // 录制结束：先恢复鼠标事件 + 重新激活 app（避免假死）
        for label in &scroll_labels {
            if let Some(win) = ah.get_webview_window(label) {
                set_window_ignores_mouse_events(&win, false);
            }
        }
        #[cfg(target_os = "macos")]
        set_app_active_on_main(&sel_win, true);

        // 补全最后一帧的完整可见区域（含底部 sticky footer）
        if let Some(ref lf) = last_frame {
            let _ = stitcher.finalize(lf);

            // finalize 后再 emit 一帧预览（spawn_blocking 避免阻塞事件循环）
            let canvas = stitcher.canvas().clone();
            let preview_b64 = tokio::task::spawn_blocking(move || {
                let preview_w = 400u32;
                let max_preview_h = 1200u32;
                let crop_src_h = canvas.height().min(max_preview_h * canvas.width() / preview_w);
                let crop_y = canvas.height() - crop_src_h;
                let canvas_cropped = image::imageops::crop_imm(&canvas, 0, crop_y, canvas.width(), crop_src_h).to_image();
                let preview_h = (preview_w * canvas_cropped.height() / canvas_cropped.width()).min(max_preview_h);
                let preview = image::imageops::resize(&canvas_cropped, preview_w, preview_h, image::imageops::FilterType::CatmullRom);
                let mut preview_png = Vec::new();
                let _ = preview.write_to(&mut std::io::Cursor::new(&mut preview_png), image::ImageFormat::Png);
                general_purpose::STANDARD.encode(&preview_png)
            }).await.unwrap_or_default();
            let final_height = stitcher.height();
            let _ = ah2.emit("scroll://frame", serde_json::json!({
                "frame": preview_b64,
                "preview": preview_b64,
                "height": final_height,
                "phys_height": (final_height as f64 / scale) as u32,
            }));
        }

        // 写入 DB（不在此处关窗口，等 emit scroll://done 后前端处理完再关）
        let stop_mode = match SCROLL_STOP_MODE.load(std::sync::atomic::Ordering::SeqCst) {
            1 => ScrollStopMode::Save,
            2 => ScrollStopMode::Cancel,
            _ => ScrollStopMode::Copy,
        };
        SCROLL_STOP_MODE.store(0, std::sync::atomic::Ordering::SeqCst);

        if stop_mode == ScrollStopMode::Cancel {
            // 取消：不入库，直接关窗口
            close_all_screenshot_windows(&ah);
            return;
        }

        let canvas = stitcher.canvas().clone();
        let ah3 = ah.clone();
        let result = tokio::task::spawn_blocking(move || {
            // 直接从 canvas 转 DynamicImage，避免 PNG 编码→解码冗余往返
            let img = image::DynamicImage::ImageRgba8(canvas);

            // PNG 编码用快速压缩（写剪贴板/base64 传前端，不需要高压缩率）
            let mut png_bytes = Vec::new();
            let mut cursor = std::io::Cursor::new(&mut png_bytes);
            let png_encoder = image::codecs::png::PngEncoder::new_with_quality(
                &mut cursor,
                image::codecs::png::CompressionType::Fast,
                image::codecs::png::FilterType::Up,
            );
            let _ = img.write_with_encoder(png_encoder);

            let hash = octopus_clipboard::image::sha256_hex(&png_bytes);
            let encoded = match octopus_clipboard::image::encode_to_webp(&img) { Ok(e) => e, Err(_) => return None };

            let item_id = octopus_clipboard::store::chrono_millis();
            let _ = octopus_infra::db::with_db(|conn| {
                octopus_clipboard::store::insert_image_data(conn, &hash, &encoded.webp_blob, &encoded.thumb_blob, img.width() as i64, img.height() as i64)
            });
            let _ = octopus_infra::db::with_db(|conn| {
                octopus_clipboard::store::insert_clipboard_item(conn, &octopus_clipboard::store::NewClipboardItem {
                    id: item_id, item_type: octopus_clipboard::ItemType::Image,
                    content: hash.clone(), search_text: String::new(),
                    created_at: octopus_clipboard::store::iso_now(),
                    blob_hash: Some(hash.clone()), width: Some(img.width() as i64),
                    height: Some(img.height() as i64), has_thumbnail: Some(1),
                    file_count: None, is_rich: false,
                })
            });

            // 写入系统剪贴板
            if let Some(handle) = ah3.try_state::<std::sync::Arc<ClipboardHandle>>() {
                let _ = handle.write_image(&png_bytes);
            }

            Some((serde_json::json!({ "id": item_id }), png_bytes))
        }).await.unwrap_or(None);

        if let Some((done_payload, png_bytes)) = result {
            let _ = ah.emit("scroll://done", done_payload);
            let _ = ah.emit("clipboard://changed", ());

            // 保存模式：Rust 端直接弹对话框（无需前端中转 base64）
            if stop_mode == ScrollStopMode::Save {
                use tauri_plugin_dialog::DialogExt;
                let save_path = ah.dialog()
                    .file()
                    .add_filter("PNG 图片", &["png"])
                    .set_file_name("scroll-screenshot.png")
                    .blocking_save_file();
                if let Some(path) = save_path {
                    if let Some(p) = path.as_path() {
                        let _ = std::fs::write(p, &png_bytes);
                    }
                }
            }
        }

        // 关闭截图窗口
        close_all_screenshot_windows(&ah);
    });

    Ok(())
}

#[tauri::command]
pub fn stop_scroll_recording() {
    SCROLL_RECORDING.store(false, std::sync::atomic::Ordering::SeqCst);
}

/// 前端设置停止模式（保存/复制/取消），然后停止录制
#[tauri::command]
pub fn stop_scroll_recording_with_mode(mode: String) {
    let m = match mode.as_str() {
        "save" => ScrollStopMode::Save,
        "cancel" => ScrollStopMode::Cancel,
        _ => ScrollStopMode::Copy,
    };
    SCROLL_STOP_MODE.store(m as u8, std::sync::atomic::Ordering::SeqCst);
    SCROLL_RECORDING.store(false, std::sync::atomic::Ordering::SeqCst);
}
