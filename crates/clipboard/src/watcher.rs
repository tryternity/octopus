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
        (self.on_change)();
    }
}

/// 处理剪贴板变化：判断类型 → 读内容 → 去重 → 存 DB。
/// 在 watcher 回调线程中调用。
pub fn handle_clipboard_change(handle: &crate::ClipboardHandle) {
    use clipboard_rs::common::{ContentFormat, RustImage};
    use crate::model::*;
    use crate::store;
    use crate::image;

    // 按优先级判断类型：files > image > text
    let result: anyhow::Result<()> = (|| {
        if handle.has(ContentFormat::Files) {
            // file 类型
            let files = handle.read_files()?;
            if files.is_empty() {
                return Ok(());
            }
            let paths_json = serde_json::to_string(&files).unwrap_or_default();
            let search_text = files.join(" ");
            let count = files.len();

            // 去重
            let existing = octopus_infra::db::with_db(|conn| {
                store::find_by_text(conn, &paths_json)
            })?;

            if let Some(id) = existing {
                octopus_infra::db::with_db(|conn| store::touch_created_at(conn, id))?;
            } else {
                octopus_infra::db::with_db(|conn| {
                    store::insert_clipboard_item(conn, &store::NewClipboardItem {
                        id: store::chrono_millis(),
                        item_type: ItemType::File,
                        content: paths_json,
                        search_text,
                        created_at: store::iso_now(),
                        blob_hash: None,
                        width: None,
                        height: None,
                        has_thumbnail: None,
                        file_count: Some(count as i64),
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
            let (png_bytes, hash) = image::encode_and_hash(&rgba, w, h)?;

            // 去重
            let existing = octopus_infra::db::with_db(|conn| {
                store::find_by_content_hash(conn, &hash)
            })?;

            if let Some(id) = existing {
                octopus_infra::db::with_db(|conn| store::touch_created_at(conn, id))?;
            } else {
                // 保存图片文件
                let save_result = image::save_image(&png_bytes, &hash);
                let has_thumb = save_result.is_ok();

                octopus_infra::db::with_db(|conn| {
                    store::insert_clipboard_item(conn, &store::NewClipboardItem {
                        id: store::chrono_millis(),
                        item_type: ItemType::Image,
                        content: hash.clone(),
                        search_text: String::new(),
                        created_at: store::iso_now(),
                        blob_hash: Some(hash),
                        width: Some(w as i64),
                        height: Some(h as i64),
                        has_thumbnail: if has_thumb { Some(1) } else { Some(0) },
                        file_count: None,
                        is_rich: false,
                    })
                })?;

                if let Err(e) = save_result {
                    error!("Failed to save image: {}", e);
                }
            }
        } else {
            // text 类型
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
                store::find_by_text(conn, &text)
            })?;

            if let Some(id) = existing {
                octopus_infra::db::with_db(|conn| store::touch_created_at(conn, id))?;
            } else {
                octopus_infra::db::with_db(|conn| {
                    store::insert_clipboard_item(conn, &store::NewClipboardItem {
                        id: store::chrono_millis(),
                        item_type: ItemType::Text,
                        content: text.clone(),
                        search_text: text,
                        created_at: store::iso_now(),
                        blob_hash: None,
                        width: None,
                        height: None,
                        has_thumbnail: None,
                        file_count: None,
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
