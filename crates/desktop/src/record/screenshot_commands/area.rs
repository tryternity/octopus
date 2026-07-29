//! screenshot_commands 区域截图子模块（选区 / 确认 / 保存 / OCR / 二维码 / pin）。
//!
//! 2026-07-30 从原 screenshot_commands.rs 拆出（Task 1.3）。
//! 包含：cg_event_source_ffi（macOS 鼠标按键状态 FFI）+ right_mouse_button_down +
//! ScreenCaptureClone + 全部截图静态量（ALL_CAPTURES / PENDING_IMAGES / READY_COUNT /
//! TOTAL_WINDOWS / SCREENSHOT_BUSY / LAST_SCREENSHOT_OCR）+ register_screenshot_shortcut +
//! start_screenshot / save_screenshot_to_history / ocr_screenshot / scan_qrcode_screenshot /
//! get_last_screenshot_ocr / show_screenshot_window / save_screenshot_dialog /
//! confirm_screenshot_with_data / get_screenshot_image(_size) / confirm_screenshot /
//! cancel_screenshot / pin_screenshot。
//!
//! `TOTAL_WINDOWS` 被 scroll.rs 的 close_all_screenshot_windows 复位，故为 pub(crate)。

use parking_lot::Mutex;
use tauri::{Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};
use crate::error_util::{e2s, e2s_ctx};
use octopus_clipboard::ClipboardHandle;

use super::{
    close_all_screenshot_windows,
    format_file_size,
    get_window_cocoa_frame,
    save_frontmost_app,
};

// macOS：查询当前鼠标按键是否按下（HIDSystemState 反映硬件状态）。
//
// 用于 scrolling 模式右键取消——选区外鼠标穿透到下层应用，前端 onContextMenu
// 收不到。后端在 16ms 轮询里检查右键状态 + 边沿检测，选区外按下右键则停止 scroll。
//
// button：0=左键、1=右键、2=中键（CGMouseButton 值）。
// state_id：0=CombinedSessionState、1=HIDSystemState（硬件状态，不受其他 app 影响）。
//
// 详见 CGEventSourceButtonState（CGEventSource.h）。
#[cfg(target_os = "macos")]
mod cg_event_source_ffi {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        pub(crate) fn CGEventSourceButtonState(state_id: i32, button: i32) -> bool;
    }
}

/// 便捷封装：当前右键是否按下。
#[cfg(target_os = "macos")]
pub(crate) fn right_mouse_button_down() -> bool {
    // state_id=1（HIDSystemState）查硬件按键状态；button=1（右键）
    unsafe { cg_event_source_ffi::CGEventSourceButtonState(1, 1) }
}

/// 截图数据副本（不含 monitor 坐标，仅像素数据用于裁剪）
#[derive(Clone)]
struct ScreenCaptureClone {
    rgba_bytes: Vec<u8>,
    width: u32,
    height: u32,
}

/// 所有显示器的截图数据，按 window label 索引。
static ALL_CAPTURES: Mutex<Vec<(String, ScreenCaptureClone)>> = Mutex::new(Vec::new());
/// 待处理的图片 RGBA bytes + 宽高，按 window label 索引（前端 mount 后拉取）。
///
/// 2026-07-20 perf：原存 JPEG base64 string（每屏 3840×2160 编码 ~1.7s），
/// 改存 RGBA bytes 后省去 JPEG 编码 + base64 round-trip（~3s/双屏），
/// 前端用 createImageBitmap(ImageData) 直接 GPU-friendly 解码。
static PENDING_IMAGES: Mutex<Vec<(String, Vec<u8>, u32, u32)>> = Mutex::new(Vec::new());
/// 窗口 ready 计数（前端报告 ready 后累加，达到总数后统一 show）。
static READY_COUNT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
/// 窗口总数（scroll.rs 的 close_all_screenshot_windows 复位时跨模块写，故 pub(crate)）。
pub(crate) static TOTAL_WINDOWS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
/// 截图并发门控：防止狂按快捷键导致多个 start_screenshot 并发 clear/push 静态量。
/// CAS true → 进入；已是 true → 直接返回（上一次截图仍在进行）。
static SCREENSHOT_BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// 最近一次截图 OCR 结果（关联 image_id）。emit("ocr-screenshot://result") 早于
/// 新窗 React mount 会被丢，ImagePreview mount 后用 get_last_screenshot_ocr 主动拉取兜底。
/// 截图 OCR 全局互斥（OcrLockGuard），单槽即可，无需并发保护。
static LAST_SCREENSHOT_OCR: Mutex<Option<(i64, crate::clipboard::clipboard_commands::OcrResult)>> = Mutex::new(None);

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


#[tauri::command]
pub async fn start_screenshot(app_handle: tauri::AppHandle) -> Result<(), String> {
    // 并发门控：狂按快捷键/重复点击托盘时 CAS false→true 才进入；已在进行则忽略。
    if SCREENSHOT_BUSY.compare_exchange(
        false, true,
        std::sync::atomic::Ordering::SeqCst,
        std::sync::atomic::Ordering::SeqCst,
    ).is_err() {
        log::debug!("screenshot already in progress, ignoring");
        return Err("screenshot already in progress".to_string());
    }
    // RAII guard：函数退出时（无论 Ok/Err）释放门控
    struct BusyGuard;
    impl Drop for BusyGuard {
        fn drop(&mut self) {
            SCREENSHOT_BUSY.store(false, std::sync::atomic::Ordering::SeqCst);
        }
    }
    let _guard = BusyGuard;

    #[cfg(target_os = "macos")]
    save_frontmost_app();

    crate::tray::update_tray_screenshot_label(true);


    // 1. 截所有显示器（多屏并行）。
    // spawn_blocking：capture_all_monitors 内部 std::thread::scope 并行截图，
    // 整体仍为 CPU/IO 密集（CGWindowListCreateImage + GPU 回读 + BGRA→RGBA swap），
    // 隔离 Tokio worker 避免阻塞录音/VAD/剪贴板监听（与同文件 L298/L439/L477 同范式）。
    let mut captures = tokio::task::spawn_blocking(octopus_capx::capture::capture_all_monitors)
        .await
        .map_err(|e| e2s_ctx("截图任务 join 失败: {}", e))?
        .map_err(|e| e2s_ctx("截图失败: {}", e))?;

    // 3. 获取 Tauri 的显示器列表（逻辑坐标）
    let tauri_monitors = app_handle.available_monitors()
        .map_err(|e| e2s_ctx("获取显示器失败: {}", e))?;

    // 清理旧数据 + 旧窗口
    ALL_CAPTURES.lock().clear();
    PENDING_IMAGES.lock().clear();
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
        let scale = tauri_mon.scale_factor();
        let pos_x = tauri_mon.position().x as f64 / scale;  // 物理 → 逻辑
        let pos_y = tauri_mon.position().y as f64 / scale;
        let log_w = phys_w / scale;
        let log_h = phys_h / scale;

        // 用物理坐标匹配 xcap capture（避免双相同分辨率显示器匹配到同一个）。
        // 三级 fallback：精确坐标 → 相同分辨率 → 索引。逐级 if let 避免多次 iter_mut 借用冲突。
        let target_x = tauri_mon.position().x;
        let target_y = tauri_mon.position().y;
        let capture = if let Some(c) = captures.iter_mut().find(|c| c.monitor_x == target_x && c.monitor_y == target_y) {
            Some(c)
        } else if let Some(c) = captures.iter_mut().find(|c| c.width as f64 == phys_w && c.height as f64 == phys_h) {
            Some(c)
        } else {
            captures.get_mut(i)
        };

        let capture = match capture {
            Some(c) => c,
            None => continue,
        };

        let label = format!("screenshot_{}_{}", session_id, i);

        // 取走 capture 的 rgba_bytes——capture 在循环内仅此处使用，取走后 captures
        // 集合仍有该 capture（rgba_bytes 为空 Vec）但不影响后续匹配（已匹配完）。
        // 2026-07-20 perf：删 JPEG/base64 编码（省 ~3s/双屏），rgba_bytes 一份给
        // PENDING_IMAGES（前端用 ImageData 直接 createImageBitmap），一份给 ALL_CAPTURES。
        let (width, height) = (capture.width, capture.height);
        let rgba_bytes = std::mem::take(&mut capture.rgba_bytes);
        // 4K RGBA ≈ 32MB，clone 一次给前端消费；ALL_CAPTURES 拿原数据 move。
        let rgba_for_frontend = rgba_bytes.clone();

        PENDING_IMAGES.lock().push((label.clone(), rgba_for_frontend, width, height));
        ALL_CAPTURES.lock().push((label.clone(), ScreenCaptureClone {
            rgba_bytes,
            width,
            height,
        }));

        // 串行创建窗口（同时创建多个全屏 WebView 会导致 macOS segfault）
        // 2026-07-20 perf：实测 macOS 26 双屏同时创建无 segfault，去掉 150ms sleep。
        // 如未来复现 segfault，恢复 sleep 并调研根本原因（150ms 本就是不可靠 workaround）。

        let window_result = WebviewWindowBuilder::new(
            &app_handle,
            &label,
            // 截图窗口独立 entry（screenshot.html → screenshot-main.tsx）：
            // 只含 React + tauri api + annotation + Screenshot（~200KB），
            // 不带 CodeMirror/markdown-it/lucide-react（主入口才用）。
            // 截图窗口 ready 时间 ~3s → <1s。
            WebviewUrl::App("screenshot.html".into()),
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
    // 用 tokio::spawn + tokio::time::sleep：task 在 runtime 回收，避免 std::thread 泄漏，
    // 且 std::thread::sleep 在 async 上下文会阻塞 OS 线程。
    {
        let ah = app_handle.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
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

/// 截图入库共用核心：SHA-256 去重 → WebP 编码 → image_data + clipboard_history 入库，返回 image_id。
/// 已存在同 hash 则 touch created_at 并返回既有 id。ocr_screenshot / confirm_screenshot_with_data /
/// confirm_screenshot 三入口共用。decode+encode 为 CPU 密集，调用方应置于 spawn_blocking。
fn save_screenshot_to_history(
    png_bytes: &[u8],
    predecoded: Option<&::image::DynamicImage>,
) -> Result<i64, String> {
    let hash = octopus_clipboard::image::hash_bytes(png_bytes);
    let existing = octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::find_by_content_hash(conn, &hash)
    }).map_err(e2s)?;
    if let Some(id) = existing {
        octopus_infra::db::with_db(|conn| {
            octopus_clipboard::store::touch_created_at(conn, id)
        }).map_err(e2s)?;
        Ok(id)
    } else {
        // 优先使用调用方已解码的图像，避免重复解码
        let img = if let Some(img) = predecoded {
            img.clone()
        } else {
            ::image::load_from_memory(png_bytes)
                .map_err(|e| e2s_ctx("解码失败: {:?}", e))?
        };
        let crop_w = img.width();
        let crop_h = img.height();
        let encoded = octopus_clipboard::image::encode_image(&img)
            .map_err(|e| e2s_ctx("WebP 编码失败: {}", e))?;
        octopus_infra::db::with_db(|conn| {
            octopus_clipboard::store::insert_image_data(
                conn, &hash, &encoded.image_blob, &encoded.thumb_blob,
                crop_w as i64, crop_h as i64,
            )
        }).map_err(e2s)?;
        let id = octopus_clipboard::store::chrono_millis();
        octopus_infra::db::with_db(|conn| {
            octopus_clipboard::store::insert_clipboard_item(conn, &octopus_clipboard::store::NewClipboardItem {
                id,
                item_type: octopus_clipboard::ItemType::Image,
                content: String::new(),
                ref_data: Some(hash.clone()),
                meta_info: Some(octopus_clipboard::MetaInfo {
                    w: Some(crop_w), h: Some(crop_h), size: Some(format_file_size(encoded.image_blob.len() as u64)),
                    ..Default::default()
                }),
                created_at: octopus_clipboard::store::iso_now(),
                has_thumbnail: Some(1),
                is_rich: false,
            })
        }).map_err(e2s)?;
        Ok(id)
    }
}

/// 截图 OCR：合成选区 → 图片入库 → OCR 识别 → 新建 ocr 条目 → 打开 CompactEditor tab。
/// 图片仍入库为剪贴板图片条目（截图历史）；识别文本独立进 source=ocr 条目并在编辑 tab 打开。
#[tauri::command]
pub async fn ocr_screenshot(
    request: tauri::ipc::Request<'_>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    // 全局 OCR 互斥：已有 OCR 在跑则立即拒绝，避免多任务并发进入推理。
    let _ocr_lock = octopus_ocr::engine::OcrLockGuard::try_acquire()
        .ok_or_else(|| "前一个 OCR 还未完成，请稍后".to_string())?;
    let tauri::ipc::InvokeBody::Raw(png_bytes) = request.body() else {
        return Err("expected raw binary body".into());
    };
    let png_bytes = png_bytes.clone();

    ALL_CAPTURES.lock().clear();
    PENDING_IMAGES.lock().clear();

    // current_ocr_meta 闭包外取（与 insert_ocr_clipboard_item 同习惯，详见其注释）：
    // 闭包内调虽已不再死锁（db.rs 已换 ReentrantMutex），仍避免 DB 锁嵌套持有。
    let (ocr_engine, ocr_model) = crate::clipboard::clipboard_commands::current_ocr_meta();

    // 入库（decode+encode CPU）+ OCR 识别（秒级 CPU）+ ocr 条目入库：移入 spawn_blocking，
    // 隔离 Tokio worker，避免 recognize 秒级阻塞拖累录音/VAD/剪贴板监听。
    let (image_id, text, blocks, ocr_id_opt) = tokio::task::spawn_blocking(move || {
        // 解码一次，save + OCR 共用——避免双重解码（4K 截图省 ~100-300ms）
        let img = ::image::load_from_memory(&png_bytes)
            .map_err(|e| e2s_ctx("解码失败: {:?}", e))?;
        let image_id = save_screenshot_to_history(&png_bytes, Some(&img))?;
        log::info!("[ocr-screenshot] before instance");
        let engine = octopus_ocr::engine::OcrEngine::instance()
            .map_err(e2s)?;
        log::info!(
            "[ocr-screenshot] after instance; before recognize png_bytes={} bytes",
            png_bytes.len()
        );
        let (text, blocks) = engine.recognize_with_blocks_from_image(&img).map_err(e2s)?;
        log::info!("[ocr-screenshot] after recognize text_len={} blocks={}", text.len(), blocks.len());

        let ocr_id_opt: Option<i64> = if !text.trim().is_empty() {
            log::info!("[ocr-screenshot] before insert_ocr_item");
            let ocr_id = octopus_infra::db::with_db(|conn| {
                octopus_clipboard::store::insert_ocr_item(conn, &text, &ocr_engine, &ocr_model)
            })
            .map_err(e2s)?;
            log::info!("[ocr-screenshot] after insert_ocr_item id={}", ocr_id);
            Some(ocr_id)
        } else {
            None
        };
        Ok::<_, String>((image_id, text, blocks, ocr_id_opt))
    })
    .await
    .map_err(e2s)??;
    let image_id_opt: Option<i64> = Some(image_id);

    let _ = app_handle.emit("clipboard://changed", ());

    // 关截图窗 → 开编辑器 + 图片预览 + 推送 OCR blocks 给预览窗
    let ocr_result = crate::clipboard::clipboard_commands::OcrResult {
        text: text.clone(),
        blocks: blocks.iter().map(|b| crate::clipboard::clipboard_commands::OcrTextBlock {
            text: b.text.clone(), x: b.x, y: b.y, w: b.w, h: b.h, score: b.score,
        }).collect(),
    };

    log::info!("[ocr-screenshot] dispatch close+open to main thread");
    let ah = app_handle.clone();
    let _ = app_handle.run_on_main_thread(move || {
        log::info!("[ocr-screenshot] main: closing screenshot windows");
        close_all_screenshot_windows(&ah);
        // 合并双开为一次 open_compact_editor_tabs：避免连续单开在「窗口刚 build、
        // React 未 mount」中间态丢失第二个 tab（首个经 URL 注入幸存，第二个被
        // push 覆盖 + emit 丢 → ocr 文本 tab 丢失）。批量调用只走一次 create/emit。
        let mut tabs: Vec<(i64, Option<&str>)> = Vec::new();
        if let Some(img_id) = image_id_opt {
            log::info!("[ocr-screenshot] main: open image tab {}", img_id);
            tabs.push((img_id, None));
        }
        if let Some(ocr_id) = ocr_id_opt {
            log::info!("[ocr-screenshot] main: open ocr text tab {}", ocr_id);
            tabs.push((ocr_id, None));
        }
        crate::commands::compact_editor_commands::open_compact_editor_tabs(&tabs, &ah);
    });

    // emit OCR blocks（图片预览 mount 后 listen 到，或已 mount 则直接收到）。
    // 同时缓存（关联 image_id）——emit 早于新窗 React mount 会被丢，ImagePreview
    // mount 时用 get_last_screenshot_ocr 主动拉取兜底。
    use tauri::Emitter;
    let _ = app_handle.emit("ocr-screenshot://result", &ocr_result);
    *LAST_SCREENSHOT_OCR.lock() = Some((image_id, ocr_result));

    Ok(())
}

/// 截图二维码识别：前端传 Raw body PNG（与 ocr_screenshot 同协议）。
/// spawn_blocking 内解码 PNG → qrcode::scan。
/// 返回识别到的二维码内容列表（可能空）。不入库、不开编辑器、不自动写剪贴板
/// （前端白卡提供单个复制 + 复制所有按钮）。
#[tauri::command]
pub async fn scan_qrcode_screenshot(
    request: tauri::ipc::Request<'_>,
) -> Result<Vec<String>, String> {
    let tauri::ipc::InvokeBody::Raw(png_bytes) = request.body() else {
        return Err("expected raw binary body".into());
    };
    let png_bytes = png_bytes.clone();

    let codes = tokio::task::spawn_blocking(move || -> Result<Vec<String>, String> {
        let img = ::image::load_from_memory(&png_bytes)
            .map_err(|e| e2s_ctx("解码失败: {:?}", e))?;
        octopus_ocr::qrcode::scan(&img).map_err(e2s)
    })
    .await
    .map_err(e2s)??;
    Ok(codes)
}

/// ImagePreview mount 时拉取缓存（按 image_id 校验：匹配返回并清空，不匹配放回）。
/// 治 emit("ocr-screenshot://result") 早于新窗 React mount 的竞态——截图 OCR 后
/// 新窗 ImagePreview mount 时主动拉高亮遮罩（emit 已被丢）。截图 OCR 全局互斥，单槽即可。
#[tauri::command]
pub fn get_last_screenshot_ocr(image_id: i64) -> Option<crate::clipboard::clipboard_commands::OcrResult> {
    let mut g = LAST_SCREENSHOT_OCR.lock();
    if let Some((id, res)) = g.take() {
        if id == image_id {
            Some(res)
        } else {
            *g = Some((id, res)); // 不匹配：放回（非本次截图，保留待对应图片）
            None
        }
    } else {
        None
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
    request: tauri::ipc::Request<'_>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    let tauri::ipc::InvokeBody::Raw(png_bytes) = request.body() else {
        return Err("expected raw binary body".into());
    };
    let png_bytes = png_bytes.clone();

    ALL_CAPTURES.lock().clear();
    PENDING_IMAGES.lock().clear();

    // 先关闭截图窗口，恢复正常屏幕，再弹保存对话框
    close_all_screenshot_windows(&app_handle);

    // blocking_save_file（弹原生对话框，等用户选路径可达数秒）+ fs::write 均为同步阻塞，
    // 全部移入 spawn_blocking 避免卡住 Tokio worker 线程（与 clipboard_commands::save_image_dialog
    // 同模式：不用 plugin 回调式 save_file()，因回调内写入错误无法回传前端）。
    tokio::task::spawn_blocking(move || {
        use tauri_plugin_dialog::DialogExt;
        let save_path = app_handle
            .dialog()
            .file()
            .add_filter("PNG 图片", &["png"])
            .set_file_name("screenshot.png")
            .blocking_save_file();
        if let Some(path) = save_path {
            let path = path.as_path().ok_or("无效路径")?;
            std::fs::write(path, &png_bytes).map_err(e2s)?;
            log::info!("Screenshot saved to {}", path.display());
        }
        Ok::<(), String>(())
    })
    .await
    .map_err(e2s)?
}

/// 前端合成标注+裁剪后，直接发送最终 PNG（Raw body 二进制）
/// 元数据（label/width/height）通过 headers 传递
#[tauri::command]
pub async fn confirm_screenshot_with_data(
    request: tauri::ipc::Request<'_>,
    app_handle: tauri::AppHandle,
    handle: State<'_, std::sync::Arc<ClipboardHandle>>,
) -> Result<(), String> {
    let tauri::ipc::InvokeBody::Raw(png_bytes) = request.body() else {
        return Err("expected raw binary body".into());
    };
    let png_bytes = png_bytes.clone();

    // 清空所有暂存
    ALL_CAPTURES.lock().clear();
    PENDING_IMAGES.lock().clear();

    // 入库（decode+encode CPU）+ 写系统剪贴板：移入 spawn_blocking 隔离 Tokio worker。
    let handle_clone = handle.inner().clone();
    tokio::task::spawn_blocking(move || {
        save_screenshot_to_history(&png_bytes, None)?;
        handle_clone.write_image(&png_bytes).map_err(e2s)?;
        Ok::<(), String>(())
    })
    .await
    .map_err(e2s)??;
    let _ = app_handle.emit("clipboard://changed", ());
    close_all_screenshot_windows(&app_handle);

    Ok(())
}
#[tauri::command]
pub fn get_screenshot_image(label: String) -> Result<tauri::ipc::Response, String> {
    // 取出对应的 RGBA bytes（克隆而非 remove，兼容 StrictMode 双 mount）
    // 2026-07-20 perf：原 JPEG base64 → JPEG bytes，省 base64 decode + JPEG 编码。
    let rgba = {
        let pending = PENDING_IMAGES.lock();
        pending
            .iter()
            .find(|(l, _, _, _)| *l == label)
            .map(|(_, bytes, _, _)| bytes.clone())
    }
    .ok_or("无待处理截图数据")?;

    Ok(tauri::ipc::Response::new(rgba))
}

/// 返回截图宽高（前端构造 ImageData 需要）。
/// 2026-07-20 perf：随 get_screenshot_image 拆出，避免 Tauri Response bytes 只能传裸数据。
#[tauri::command]
pub fn get_screenshot_image_size(label: String) -> Result<(u32, u32), String> {
    let pending = PENDING_IMAGES.lock();
    pending
        .iter()
        .find(|(l, _, _, _)| *l == label)
        .map(|(_, _, w, h)| (*w, *h))
        .ok_or_else(|| "无待处理截图数据".to_string())
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
        let mut all = ALL_CAPTURES.lock();
        all.iter()
            .position(|(l, _)| *l == label)
            .map(|i| all.remove(i).1)
    }
    .ok_or("无截图数据")?;

    // 清空所有暂存
    ALL_CAPTURES.lock().clear();
    PENDING_IMAGES.lock().clear();

    // 裁剪选区 + 入库（decode+encode CPU）+ 写系统剪贴板：移入 spawn_blocking 隔离 Tokio worker。
    let handle_clone = handle.inner().clone();
    tokio::task::spawn_blocking(move || {
        let fake_full = octopus_capx::capture::ScreenCapture {
            rgba_bytes: full.rgba_bytes.clone(),
            width: full.width,
            height: full.height,
            monitor_x: 0,
            monitor_y: 0,
        };
        let png_bytes = octopus_capx::capture::crop_region(&fake_full, x, y, w, h)
            .map_err(|e| e2s_ctx("裁剪失败: {}", e))?;
        save_screenshot_to_history(&png_bytes, None)?;
        handle_clone.write_image(&png_bytes).map_err(e2s)?;
        Ok::<(), String>(())
    })
    .await
    .map_err(e2s)??;

    // 通知前端刷新
    let _ = app_handle.emit("clipboard://changed", ());

    // 10. 关闭所有截图窗口
    close_all_screenshot_windows(&app_handle);

    Ok(())
}

/// 取消截图：关所有窗口
#[tauri::command]
pub async fn cancel_screenshot(app_handle: tauri::AppHandle) -> Result<(), String> {
    ALL_CAPTURES.lock().clear();
    PENDING_IMAGES.lock().clear();
    close_all_screenshot_windows(&app_handle);
    Ok(())
}

/// 贴图到桌面：裁剪选区 → 创建原生浮动窗口显示截图
#[tauri::command]
pub async fn pin_screenshot(
    request: tauri::ipc::Request<'_>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    // 2026-07-20 perf：自定义二进制协议（仿 Solana ts sdk 风格），省 base64 round-trip。
    // 协议：[u32 BE: label_len][label UTF-8][f64 BE: x][f64 BE: y][f64 BE: w][f64 BE: h][PNG bytes]
    // label_len 后跟 label 字节，4 个 f64 是选区几何（CSS 像素），剩余字节是 composeAndCropBytes 产出的 PNG。
    let tauri::ipc::InvokeBody::Raw(body) = request.body() else {
        return Err("pin_screenshot expects raw binary body".into());
    };
    if body.len() < 4 {
        return Err("pin_screenshot body too short".into());
    }
    let label_len = u32::from_be_bytes([body[0], body[1], body[2], body[3]]) as usize;
    if body.len() < 4 + label_len + 32 {
        return Err(format!("pin_screenshot body truncated: need at least {} bytes, got {}", 4 + label_len + 32, body.len()));
    }
    let label = String::from_utf8(body[4..4 + label_len].to_vec())
        .map_err(|e| e2s_ctx("label UTF-8 decode failed: {}", e))?;
    let mut off = 4 + label_len;
    let read_f64 = |off: &mut usize| -> f64 {
        let v = f64::from_be_bytes([
            body[*off], body[*off+1], body[*off+2], body[*off+3],
            body[*off+4], body[*off+5], body[*off+6], body[*off+7],
        ]);
        *off += 8;
        v
    };
    let x = read_f64(&mut off);
    let y = read_f64(&mut off);
    let w = read_f64(&mut off);
    let h = read_f64(&mut off);
    let png_bytes: Vec<u8> = body[off..].to_vec();
    if png_bytes.is_empty() {
        return Err("pin_screenshot: missing PNG bytes".into());
    }

    let sel_win = app_handle
        .get_webview_window(&label)
        .ok_or("截图窗口不存在")?;

    ALL_CAPTURES.lock().clear();
    PENDING_IMAGES.lock().clear();

    #[cfg(target_os = "macos")]
    let (pin_x, pin_y) = if let Some((cx, cy, _cw, ch)) = get_window_cocoa_frame(&sel_win) {
        (cx + x, cy + ch - y - h)
    } else {
        (x, y)
    };

    #[cfg(not(target_os = "macos"))]
    let (pin_x, pin_y) = {
        let sf = sel_win.scale_factor().unwrap_or(1.0);
        let (wx, wy) = match sel_win.outer_position() {
            Ok(p) => (p.x as f64 / sf, p.y as f64 / sf),
            Err(_) => (0.0, 0.0),
        };
        (wx + x, wy + y)
    };

    let (tx, rx) = std::sync::mpsc::channel();
    let _ = sel_win.run_on_main_thread(move || {
        <crate::pin_window::PinWindowImpl as crate::pin_window::PinWindow>::create(
            &png_bytes, pin_x, pin_y, w, h,
        );
        let _ = tx.send(());
    });
    let _ = rx.recv_timeout(std::time::Duration::from_secs(2));

    close_all_screenshot_windows(&app_handle);
    Ok(())
}
