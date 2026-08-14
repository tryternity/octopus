use anyhow::Result;
use clipboard_rs::{ClipboardHandler, ClipboardWatcherContext};
use clipboard_rs::ClipboardWatcher as _ClipboardWatcherTrait;
use log::error;
use std::sync::Arc;

/// 剪贴板监听器。start 在独立线程跑 start_watch()，stop 发信号终止。
pub struct ClipboardWatcher {
    shutdown: Option<clipboard_rs::WatcherShutdown>,
}

impl ClipboardWatcher {
    /// 启动监听线程。on_change 回调在 watcher 线程中调用（非主线程）。
    pub fn start<F>(handle: Arc<crate::ClipboardHandle>, on_change: F) -> Result<Self>
    where
        F: Fn() + Send + Sync + 'static,
    {
        let mut watcher = ClipboardWatcherContext::new()
            .map_err(|e| anyhow::anyhow!("Watcher init failed: {}", e))?;

        let handler = ChangeHandler {
            handle,
            on_change: Arc::new(on_change),
        };

        _ClipboardWatcherTrait::add_handler(&mut watcher, handler);
        let shutdown = _ClipboardWatcherTrait::get_shutdown_channel(&watcher);

        std::thread::spawn(move || {
            _ClipboardWatcherTrait::start_watch(&mut watcher);
        });

        Ok(Self {
            shutdown: Some(shutdown),
        })
    }

    pub fn stop(&mut self) {
        if let Some(s) = self.shutdown.take() {
            drop(s);
        }
    }
}

impl Drop for ClipboardWatcher {
    fn drop(&mut self) {
        self.stop();
    }
}

struct ChangeHandler<F: Fn() + Send + Sync> {
    handle: Arc<crate::ClipboardHandle>,
    on_change: Arc<F>,
}

impl<F: Fn() + Send + Sync + 'static> ClipboardHandler for ChangeHandler<F> {
    fn on_clipboard_change(&mut self) {
        if self.handle.check_and_clear_suppress() {
            return;
        }
        // 监听已禁用（clipboard_enabled=false）：不记录、不 emit，watcher 仍运行。
        if !self.handle.is_recording_enabled() {
            return;
        }
        (self.on_change)();
    }
}

/// 字节数 → 人类可读大小：<1M 显示 K（整数）、≥1M 显示 M（1 位小数）。
fn format_file_size(bytes: u64) -> String {
    if bytes < 1024 * 1024 {
        format!("{}K", (bytes + 511) / 1024)
    } else {
        format!("{:.1}M", bytes as f64 / 1024.0 / 1024.0)
    }
}

/// 处理剪贴板变化：判断类型 → 读内容 → 去重 → 存 DB。
/// 在 watcher 回调线程中调用。
pub fn handle_clipboard_change(handle: &crate::ClipboardHandle) {
    use clipboard_rs::common::{ContentFormat, RustImage};
    use crate::model::*;
    use crate::store;
    use crate::image;

    // ── ConcealedType / 密码管理器 hint 检测（跨平台）──
    // 密码管理器（1Password/Bitwarden/KeePassXC/iCloud Keychain 等）复制密码时
    // 会按平台约定标记一个特殊 pasteboard 类型，明确告知消费方「不要记录」。
    // 静默跳过避免密码明文入库 + FTS5 索引 + 跨设备 sync 传播。
    //
    // 平台常量：
    //   macOS   org.nspasteboard.ConcealedType（nspasteboard.org 社区约定）
    //   Windows ExcludeClipboardContentFromMonitorProcessing（MS 官方 clipboard format）
    //   Linux   x-kde-passwordManagerHint（KDE/KeePassXC 事实约定；GNOME 无统一标准）
    //
    // clipboard-rs 0.3.4 三平台后端（win.rs/x11.rs/wayland.rs）的 ContentFormat::Other
    // 均支持任意类型字符串检测，故复用同一模式。octopus autotype 走 suppress_next
    // 跨平台保底，此处检测是第三方密码管理器防护 + macOS 兜底。
    const CONCEALED_HINTS: &[&str] = &[
        #[cfg(target_os = "macos")]
        "org.nspasteboard.ConcealedType",
        #[cfg(target_os = "windows")]
        "ExcludeClipboardContentFromMonitorProcessing",
        // Linux X11/Wayland 共用此 MIME 约定（KeePassXC 等）
        #[cfg(any(target_os = "linux", target_os = "dragonfly", target_os = "freebsd", target_os = "netbsd", target_os = "openbsd"))]
        "x-kde-passwordManagerHint",
    ];
    for hint in CONCEALED_HINTS {
        if handle.has(ContentFormat::Other((*hint).to_string())) {
            return;
        }
    }

    // 按优先级判断类型：files > image > text
    let result: anyhow::Result<()> = (|| {
        if handle.has(ContentFormat::Files) {
            // file 类型
            let files = handle.read_files()?;
            if files.is_empty() {
                return Ok(());
            }
            let paths_json = serde_json::to_string(&files).unwrap_or_default();
            let file_metas: Vec<crate::model::FileEntry> = files.iter().map(|p| {
                let path = std::path::Path::new(p);
                let size = std::fs::metadata(path).ok().map(|m| format_file_size(m.len()));
                let file_type = path.extension().and_then(|e| e.to_str()).map(|s| s.to_lowercase());
                crate::model::FileEntry { size, file_type }
            }).collect();

            // 去重（第四十二轮 P2-2：文件项按 ref_data 查——content 是空串，路径在 ref_data）
            let existing = octopus_infra::db::with_db(|conn| {
                store::find_file_by_paths(conn, &paths_json)
            })?;

            if let Some(id) = existing {
                octopus_infra::db::with_db(|conn| store::touch_created_at(conn, &id))?;
            } else {
                octopus_infra::db::with_db(|conn| {
                    store::insert_clipboard_item(conn, &store::NewClipboardItem {
                        id: uuid::Uuid::new_v4().to_string(),
                        item_type: ItemType::File,
                        content: String::new(),
                        ref_data: Some(paths_json),
                        meta_info: Some(crate::model::MetaInfo {
                            files: Some(file_metas),
                            ..Default::default()
                        }),
                        created_at: store::iso_now(),
                        has_thumbnail: None,
                        is_rich: false,
                    })
                })?;
            }
        } else if handle.has(ContentFormat::Image) {
            // image 类型
            let img_data = handle.read_image()?;
            let (w, h) = img_data.get_size();

            // 超过 40MB 跳过
            let estimated_size = (w as usize) * (h as usize) * 4;
            if estimated_size > 40 * 1024 * 1024 {
                log::info!("Skipping large image ({}x{} ~{}MB)", w, h, estimated_size / 1024 / 1024);
                return Ok(());
            }

            let rgba_img = img_data.to_rgba8()
                .map_err(|e| anyhow::anyhow!("to_rgba8 failed: {}", e))?;
            let rgba = rgba_img.to_vec();
            // PNG bytes 仅用于算 SHA-256 去重 hash；WebP 编码改走 DynamicImage（复用 RGBA，
            // 不再让 encode_image 内部把刚编出的 PNG 又解码一遍）。
            // 直接 hash RGBA 像素（不编码 PNG）——省去大图 PNG 编码的 CPU 开销
            let hash = image::hash_rgba(&rgba);

            // 去重
            let existing = octopus_infra::db::with_db(|conn| {
                store::find_by_content_hash(conn, &hash)
            })?;

            if let Some(id) = existing {
                octopus_infra::db::with_db(|conn| store::touch_created_at(conn, &id))?;
            } else {
                // 编码 WebP 无损 + 缩略图：复用上面的 RGBA（不再二次解码 PNG）
                let dyn_img = ::image::DynamicImage::ImageRgba8(
                    ::image::RgbaImage::from_raw(w, h, rgba)
                        .ok_or_else(|| anyhow::anyhow!("RgbaImage::from_raw failed"))?,
                );
                let encoded = image::encode_image(&dyn_img)?;
                let img_size = format_file_size(encoded.image_blob.len() as u64);

                // 存 image_data BLOB
                octopus_infra::db::with_db(|conn| {
                    store::insert_image_data(conn, &hash, &encoded.image_blob, &encoded.thumb_blob, w as i64, h as i64)
                })?;

                // 存 clipboard_history 条目
                octopus_infra::db::with_db(|conn| {
                    store::insert_clipboard_item(conn, &store::NewClipboardItem {
                        id: uuid::Uuid::new_v4().to_string(),
                        item_type: ItemType::Image,
                        content: String::new(),
                        ref_data: Some(hash.clone()),
                        meta_info: Some(crate::model::MetaInfo {
                            w: Some(w), h: Some(h), size: Some(img_size),
                            ..Default::default()
                        }),
                        created_at: store::iso_now(),
                        has_thumbnail: Some(1),
                        is_rich: false,
                    })
                })?;
            }
        } else if handle.has(ContentFormat::Text) {
            // text 类型（非 files/image/text 的自定义二进制格式或空剪贴板 → 静默跳过，
            // 避免 read_text() 失败触发 error! 日志污染——Adobe/Office 等专有格式常见）
            let text = handle.read_text()?;
            if text.is_empty() {
                return Ok(());
            }
            if text.len() > 50 * 1024 * 1024 {
                log::info!("Skipping large text ({} bytes)", text.len());
                return Ok(());
            }

            let has_html = handle.has(ContentFormat::Html);
            let has_rtf = handle.has(ContentFormat::Rtf);
            let is_rich = has_html || has_rtf;

            // 去重
            let existing = octopus_infra::db::with_db(|conn| {
                store::find_by_text(conn, &text, ItemType::Text)
            })?;

            if let Some(id) = existing {
                octopus_infra::db::with_db(|conn| store::touch_created_at(conn, &id))?;
            } else {
                octopus_infra::db::with_db(|conn| {
                    store::insert_clipboard_item(conn, &store::NewClipboardItem {
                        id: uuid::Uuid::new_v4().to_string(),
                        item_type: ItemType::Text,
                        content: text.clone(),
                        ref_data: None,
                        meta_info: Some(crate::model::MetaInfo {
                            char_count: Some(text.chars().count()),
                            ..Default::default()
                        }),
                        created_at: store::iso_now(),
                        has_thumbnail: None,
                        is_rich,
                    })
                })?;
            }
        }
        Ok(())
    })();

    if let Err(e) = result {
        error!("Clipboard change handling failed: {}", e);
    }
}
