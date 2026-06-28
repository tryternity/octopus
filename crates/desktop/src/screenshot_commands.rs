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

    // 4. 按 Tauri monitor 匹配 xcap capture（用物理尺寸近似匹配）
    for (i, tauri_mon) in tauri_monitors.iter().enumerate() {
        let phys_w = tauri_mon.size().width as f64;
        let phys_h = tauri_mon.size().height as f64;
        let scale = tauri_mon.scale_factor() as f64;
        let pos_x = tauri_mon.position().x as f64 / scale;  // 物理 → 逻辑
        let pos_y = tauri_mon.position().y as f64 / scale;
        let log_w = phys_w / scale;
        let log_h = phys_h / scale;

        // 找到物理尺寸匹配的 xcap capture
        let capture = captures.iter()
            .find(|c| c.width as f64 == phys_w && c.height as f64 == phys_h)
            .or_else(|| captures.get(i));

        let capture = match capture {
            Some(c) => c,
            None => continue,
        };

        let label = if i == 0 {
            "screenshot_window".to_string()
        } else {
            format!("screenshot_window_{}", i)
        };

        // RGBA → PNG base64
        let img = ::image::RgbaImage::from_raw(capture.width, capture.height, capture.rgba_bytes.clone())
            .ok_or("图像处理失败")?;
        let mut png_bytes = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut png_bytes), ::image::ImageFormat::Png)
            .map_err(|e| format!("PNG 编码失败: {:?}", e))?;
        let b64 = general_purpose::STANDARD.encode(&png_bytes);

        // 暂存
        PENDING_IMAGES.lock().unwrap().push((label.clone(), b64));
        ALL_CAPTURES.lock().unwrap().push((label.clone(), ScreenCaptureClone {
            rgba_bytes: capture.rgba_bytes.clone(),
            width: capture.width,
            height: capture.height,
        }));

        // 用 Tauri 的逻辑坐标 + 逻辑尺寸创建窗口
        // visible=false：等前端渲染完成后再显示，避免白屏闪烁
        let _ = WebviewWindowBuilder::new(
            &app_handle,
            &label,
            WebviewUrl::default(),
        )
        .title("")
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .visible(false)       // 初始不可见
        .position(pos_x, pos_y)
        .inner_size(log_w, log_h)
        .build();

        log::info!(
            "Screenshot window '{}' at ({},{}) {}x{} (monitor phys {}x{}, scale {})",
            label, pos_x, pos_y, log_w, log_h, phys_w, phys_h, tauri_mon.scale_factor(),
        );
    }

    Ok(())
}

/// 前端渲染完成后调用，显示指定截图窗口
#[tauri::command]
pub fn show_screenshot_window(label: String, app_handle: tauri::AppHandle) {
    if let Some(window) = app_handle.get_webview_window(&label) {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// 前端 mount 后调用，拉取当前窗口的截图数据
#[tauri::command]
pub fn get_screenshot_image(label: String) -> Result<serde_json::Value, String> {
    // 取出对应的 base64
    let b64 = {
        let mut pending = PENDING_IMAGES.lock().unwrap();
        pending
            .iter()
            .position(|(l, _)| *l == label)
            .map(|i| pending.remove(i).1)
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
        let encoded = octopus_clipboard::image::encode_to_webp(&png_bytes, crop_w, crop_h)
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
