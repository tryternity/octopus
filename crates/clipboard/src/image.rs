use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

pub fn clipboard_images_dir() -> PathBuf {
    octopus_infra::paths::octopus_config_home().join("clipboard_images")
}

pub fn encode_and_hash(rgba: &[u8], width: u32, height: u32) -> Result<(Vec<u8>, String)> {
    let img = ::image::RgbaImage::from_raw(width, height, rgba.to_vec())
        .context("Failed to create RgbaImage from raw pixels")?;
    let mut png_bytes = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png_bytes), ::image::ImageFormat::Png)
        .context("Failed to encode PNG")?;
    let hash = sha256_hex(&png_bytes);
    Ok((png_bytes, hash))
}

pub struct ImageSaveResult {
    pub orig_path: PathBuf,
    pub thumb_path: PathBuf,
}

pub fn save_image(png_bytes: &[u8], hash: &str) -> Result<ImageSaveResult> {
    let dir = clipboard_images_dir();
    std::fs::create_dir_all(&dir).context("Failed to create clipboard_images dir")?;
    let orig_path = dir.join(format!("{}.png", hash));
    let thumb_path = dir.join(format!("{}_thumb.png", hash));
    std::fs::write(&orig_path, png_bytes).context("Failed to write original image")?;
    if !thumb_path.exists() {
        generate_thumbnail(&orig_path, &thumb_path, 240)?;
    }
    Ok(ImageSaveResult { orig_path, thumb_path })
}

fn generate_thumbnail(orig: &std::path::Path, thumb: &std::path::Path, max_size: u32) -> Result<()> {
    let img = ::image::open(orig).context("Failed to open image for thumbnail")?;
    let thumbnail = img.resize(max_size, max_size, ::image::imageops::FilterType::Lanczos3);
    thumbnail.save(thumb).context("Failed to save thumbnail")?;
    Ok(())
}

pub fn cleanup_orphaned_blobs(referenced_hashes: &std::collections::HashSet<String>) -> Result<usize> {
    let dir = clipboard_images_dir();
    if !dir.exists() {
        return Ok(0);
    }
    let mut deleted = 0;
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let filename = entry.file_name();
        let filename_str = filename.to_string_lossy();
        let hash = if let Some(name) = filename_str.strip_suffix(".png") {
            name.trim_end_matches("_thumb")
        } else {
            continue;
        };
        if !referenced_hashes.contains(hash) {
            if std::fs::remove_file(entry.path()).is_ok() {
                deleted += 1;
            }
        }
    }
    Ok(deleted)
}

pub fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let result = hasher.finalize();
    let mut hex = String::with_capacity(64);
    for byte in result {
        hex.push_str(&format!("{:02x}", byte));
    }
    hex
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_and_hash() {
        let rgba = vec![255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255];
        let (png, hash) = encode_and_hash(&rgba, 2, 2).unwrap();
        assert!(!png.is_empty());
        assert_eq!(hash.len(), 64);
    }

    #[test]
    fn test_dedup_same_image() {
        let rgba = vec![255, 0, 0, 255, 0, 255, 0, 255];
        let (_, hash1) = encode_and_hash(&rgba, 2, 1).unwrap();
        let (_, hash2) = encode_and_hash(&rgba, 2, 1).unwrap();
        assert_eq!(hash1, hash2);
    }
}
