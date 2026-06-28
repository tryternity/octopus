use std::sync::Mutex;
use tauri::{Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};
use base64::{Engine, engine::general_purpose};
use octopus_clipboard::ClipboardHandle;

static SCREENSHOT_DATA: Mutex<Option<octopus_capx::capture::ScreenCapture>> = Mutex::new(None);
static PENDING_IMAGE: Mutex<Option<String>> = Mutex::new(None);
const WINDOW_LABEL: &str = "screenshot_window";

/// 启动截图：截全屏 → 创建截图窗口 → emit 图片给前端
#[tauri::command]
pub async fn start_screenshot(app_handle: tauri::AppHandle) -> Result<(), String> {
    // 1. 截全屏
    let capture = octopus_capx::capture::capture_full_screen()
        .map_err(|e| format!("截图失败: {}", e))?;

    // 2. RGBA → PNG base64（前端 Canvas 渲染用）
    let img = ::image::RgbaImage::from_raw(capture.width, capture.height, capture.rgba_bytes.clone())
        .ok_or("图像处理失败: RgbaImage::from_raw returned None")?;
    let mut png_bytes = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png_bytes), ::image::ImageFormat::Png)
        .map_err(|e| format!("PNG 编码失败: {:?}", e))?;

    let width = capture.width;
    let height = capture.height;

    // 3. 暂存全屏数据 + base64 图片
    let b64 = general_purpose::STANDARD.encode(&png_bytes);
    *PENDING_IMAGE.lock().unwrap() = Some(b64.clone());
    *SCREENSHOT_DATA.lock().unwrap() = Some(capture);

    // 4. 创建/重建截图窗口
    if let Some(old) = app_handle.get_webview_window(WINDOW_LABEL) {
        let _ = old.destroy();
    }

    let _ = WebviewWindowBuilder::new(
        &app_handle,
        WINDOW_LABEL,
        WebviewUrl::default(),
    )
    .title("")
    .fullscreen(true)
    .decorations(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .resizable(false)
    .build();

    Ok(())
}

/// 前端 mount 后调用，拉取截图数据
#[tauri::command]
pub fn get_screenshot_image() -> Result<serde_json::Value, String> {
    let b64 = PENDING_IMAGE.lock().unwrap().take()
        .ok_or("无待处理截图数据")?;
    let full = SCREENSHOT_DATA.lock().unwrap();
    let (w, h) = full.as_ref()
        .map(|c| (c.width, c.height))
        .unwrap_or((0, 0));
    Ok(serde_json::json!({
        "image": b64,
        "width": w,
        "height": h,
    }))
}

/// 确认截图：从全屏图裁剪选区 → 写剪贴板历史 → 关窗口
#[tauri::command]
pub async fn confirm_screenshot(
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    app_handle: tauri::AppHandle,
    handle: State<'_, std::sync::Arc<ClipboardHandle>>,
) -> Result<(), String> {
    // 1. 取全屏数据
    let full = SCREENSHOT_DATA.lock().unwrap().take()
        .ok_or("无截图数据")?;

    // 2. 裁剪选区 → PNG bytes
    let png_bytes = octopus_capx::capture::crop_region(&full, x, y, w, h)
        .map_err(|e| format!("裁剪失败: {}", e))?;

    // 3. SHA-256 去重（直接对裁剪后的 PNG bytes 算 hash）
    let hash = octopus_clipboard::image::sha256_hex(&png_bytes);

    // 4. 检查 DB 中是否已有此 hash
    let existing = octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::find_by_content_hash(conn, &hash)
    }).map_err(|e| e.to_string())?;

    if let Some(id) = existing {
        // 已存在：更新 created_at
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

    // 10. 关闭截图窗口
    if let Some(window) = app_handle.get_webview_window(WINDOW_LABEL) {
        let _ = window.destroy();
    }

    Ok(())
}

/// 取消截图：关窗口
#[tauri::command]
pub async fn cancel_screenshot(app_handle: tauri::AppHandle) -> Result<(), String> {
    *SCREENSHOT_DATA.lock().unwrap() = None;
    if let Some(window) = app_handle.get_webview_window(WINDOW_LABEL) {
        let _ = window.destroy();
    }
    Ok(())
}
