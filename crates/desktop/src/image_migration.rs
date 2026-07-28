//! 一次性迁移：~/.octopus/clipboard_images/ → image_data DB BLOB。
//! 幂等：已存在的 hash 跳过。迁移完成后删除目录。

use std::path::PathBuf;

fn clipboard_images_dir() -> PathBuf {
    octopus_infra::paths::octopus_config_home().join("clipboard_images")
}

/// 迁移文件系统图片到 DB。成功后删除目录。
pub fn migrate_images_to_db() {
    let dir = clipboard_images_dir();
    if !dir.exists() {
        return;
    }

    log::info!("Migrating clipboard_images/ to DB...");

    let mut migrated = 0;
    let mut skipped = 0;
    let mut errors = 0;

    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) => {
            log::warn!("Failed to read clipboard_images/: {}", e);
            return;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let filename = path.file_name().unwrap_or_default().to_string_lossy().to_string();

        if !filename.ends_with(".png") || filename.contains("_thumb") {
            continue;
        }

        let hash = filename.trim_end_matches(".png").to_string();

        let exists = octopus_infra::db::with_db(|conn| {
            octopus_clipboard::store::get_image_blob(conn, &hash)
        }).map(|v| v.is_some()).unwrap_or(false);

        if exists {
            skipped += 1;
            continue;
        }

        match std::fs::read(&path) {
            Ok(png_bytes) => {
                match ::image::load_from_memory_with_format(&png_bytes, ::image::ImageFormat::Png) {
                    Ok(img) => {
                        let w = img.width();
                        let h = img.height();
                        match octopus_clipboard::image::encode_image(&img) {
                            Ok(encoded) => {
                                let result = octopus_infra::db::with_db(|conn| {
                                    octopus_clipboard::store::insert_image_data(
                                        conn, &hash, &encoded.image_blob, &encoded.thumb_blob,
                                        w as i64, h as i64,
                                    )
                                });
                                match result {
                                    Ok(_) => migrated += 1,
                                    Err(e) => {
                                        log::warn!("Failed to insert {}: {}", hash, e);
                                        errors += 1;
                                    }
                                }
                            }
                            Err(e) => {
                                log::warn!("Failed to encode {}: {}", hash, e);
                                errors += 1;
                            }
                        }
                    }
                    Err(e) => {
                        log::warn!("Failed to decode {}: {}", hash, e);
                        errors += 1;
                    }
                }
            }
            Err(e) => {
                log::warn!("Failed to read {}: {}", path.display(), e);
                errors += 1;
            }
        }
    }

    log::info!(
        "Image migration: {} migrated, {} skipped, {} errors",
        migrated, skipped, errors
    );

    if errors == 0 {
        if let Err(e) = std::fs::remove_dir_all(&dir) {
            log::warn!("Failed to remove clipboard_images/: {}", e);
        } else {
            log::info!("Removed clipboard_images/ directory");
        }
    }
}
