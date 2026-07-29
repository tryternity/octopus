//! 图片编码：RGBA → MD5 hash（去重）→ JPEG q85 原图存文件系统 + 缩略图存 DB BLOB。
//! 原图存 ~/Documents/octopus/screens/<hash>.jpg（2026-07-29 从 DB BLOB 改文件系统）。

use anyhow::Result;

/// RGBA 像素 → MD5 hash（直接 hash 原始像素，不做 PNG 编码）。
/// hash 用于去重 + 文件名（同一张图只存一个文件）。
/// 不再生成 PNG bytes——watcher 只需 hash，PNG 编码纯粹浪费 CPU。
/// 调用方如需 PNG bytes 请用其他函数。
/// 2026-07-29：SHA-256 → MD5（更快，剪贴板去重场景无需密码学强度）。
pub fn hash_rgba(rgba: &[u8]) -> String {
    format!("{:x}", md5::compute(rgba))
}

/// 任意 bytes → MD5 hex（截图用 PNG bytes hash 去重）。
/// 替代旧 sha256_hex（2026-07-29 hash 算法统一为 MD5）。
pub fn hash_bytes(bytes: &[u8]) -> String {
    format!("{:x}", md5::compute(bytes))
}

/// 编码结果：WebP 无损原图 + WebP 缩略图。
pub struct EncodedImage {
    pub image_blob: Vec<u8>,
    pub thumb_blob: Vec<u8>,
}

/// 单次编码尝试策略：lossless WebP / 有损 WebP(q) / JPEG(q)。
/// 由 `consts::IMAGE_SAVE_QUALITY` 解析得到。
///
/// 注：`WebpLossless` 不再默认插入链首（2026-07-20 perf：lossless 对大图极慢，
/// 3176×1866 = 6s，有损 q80 = 50ms，100x 加速）。保留 variant 供未来按场景启用。
enum EncodeAttempt {
    #[allow(dead_code)]
    WebpLossless,
    WebpLossy(u8),
    Jpeg(u8),
}

impl EncodeAttempt {
    fn label(&self) -> String {
        match self {
            EncodeAttempt::WebpLossless => "lossless WebP".to_string(),
            EncodeAttempt::WebpLossy(q) => format!("WebP q{}", q),
            EncodeAttempt::Jpeg(q) => format!("JPEG q{}", q),
        }
    }

    /// 执行一次编码，返回非空 BLOB；编码 panic（超大图常见）或失败或空 → None。
    fn try_encode(&self, img: &::image::DynamicImage, rgba: &::image::RgbaImage) -> Option<Vec<u8>> {
        let (w, h) = (rgba.width(), rgba.height());
        match self {
            EncodeAttempt::WebpLossless => {
                let encoder = webp::Encoder::from_rgba(rgba, w, h);
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| encoder.encode_lossless().to_vec()))
                    .ok()
                    .filter(|b| !b.is_empty())
            }
            EncodeAttempt::WebpLossy(q) => {
                let encoder = webp::Encoder::from_rgba(rgba, w, h);
                let qf = *q as f32;
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| encoder.encode(qf).to_vec()))
                    .ok()
                    .filter(|b| !b.is_empty())
            }
            EncodeAttempt::Jpeg(q) => {
                let qv = *q;
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let mut buf = Vec::new();
                    let mut enc = ::image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, qv);
                    enc.encode_image(img).map(|_| buf)
                }))
                .ok()
                .and_then(|r| r.ok())
                .filter(|b| !b.is_empty())
            }
        }
    }
}

/// 解析 `IMAGE_SAVE_QUALITY` 常量（如 `"webp:80;jpeg:80"`）→ 降级尝试列表。
/// 按 `;` 分割条目，每条 `<格式>:<质量>`；未知格式跳过。
fn parse_image_fallbacks(s: &str) -> Vec<EncodeAttempt> {
    s.split(';')
        .filter_map(|entry| {
            let (fmt, q) = entry.trim().split_once(':')?;
            let q: u8 = q.trim().parse().ok()?;
            match fmt.trim().to_ascii_lowercase().as_str() {
                "webp" => Some(EncodeAttempt::WebpLossy(q)),
                "jpeg" | "jpg" => Some(EncodeAttempt::Jpeg(q)),
                _ => None,
            }
        })
        .collect()
}

/// DynamicImage → 主图编码（按 `IMAGE_SAVE_QUALITY` 链）+ 缩略图 WebP 20%（240×240 Triangle）。
///
/// 接收已解码的 `DynamicImage`，**不再**做 PNG 解码（旧实现接收 PNG bytes 内部
/// `load_from_memory` 解码，致「RGBA→PNG(编码)→RGBA(解码)→WebP(编码)」冗余；
/// watcher / screenshot / migration 手里本就有解码好的图像，直接传入省一次 PNG 解码）。
///
/// **函数名 `encode_image` 历史遗留**：实际按 `consts::IMAGE_SAVE_QUALITY` 链编码，
/// 2026-07-30 默认 `"jpeg:100"` —— JPEG 优先（8.6x 快于 WebP lossy，体积翻倍可接受）。
/// 返回 BLOB 可能是 JPEG 或 WebP（兜底），`image_data.blob` 字段不区分格式，前端用 MIME sniff。
///
/// **编码链**：按 `consts::IMAGE_SAVE_QUALITY` 顺序尝试，首个成功即返回。
/// 每次 WebP/JPEG 编码经 `catch_unwind` 兜底，防超大图编码 panic。返回的 BLOB 可能是
/// WebP 或 JPEG（兜底产物），统一存入 `image_data.blob`。
pub fn encode_image(img: &::image::DynamicImage) -> Result<EncodedImage> {
    let rgba = img.to_rgba8();
    let w = rgba.width();
    let h = rgba.height();

    // 组装尝试链：有损优先（截图/剪贴板历史场景对画质要求不高，lossless 编码对大图极慢，
    // 实测 3176×1866 lossless = 6s，有损 q80 ≈ 50ms，100x 加速）。
    // 超长图（VP8 尺寸上限 16383）仍走降级链，lossless 本就必失败。
    // IMAGE_SAVE_QUALITY 默认 "jpeg:100" 已是有损链，无需 insert lossless。
    let chain = parse_image_fallbacks(octopus_infra::consts::IMAGE_SAVE_QUALITY);
    if w > 16383 || h > 16383 {
        log::warn!("[clipboard] Image exceeds WebP max dimension ({}×{}), relying on fallback chain", w, h);
    }

    let image_blob = chain
        .iter()
        .find_map(|attempt| {
            let blob = attempt.try_encode(img, &rgba);
            match &blob {
                Some(b) => log::info!("[clipboard] {} succeeded: {} bytes ({}×{})", attempt.label(), b.len(), w, h),
                None => log::warn!("[clipboard] {} failed ({}×{})", attempt.label(), w, h),
            }
            blob
        })
        .ok_or_else(|| anyhow::anyhow!("All image encoding failed for {}×{}", w, h))?;

    // 缩略图：thumbnail 240×240（nearest-neighbor，快 N 倍）→ 按 THUMB_SAVE_QUALITY 编码。
    // 2026-07-20 perf：原用 `resize(240, 240, Triangle)` —— Triangle 是双线性卷积，
    // release build 实测 3176×1866 = 15ms，但 debug build 高达 674ms。
    // `thumbnail(240, 240)` 用 nearest-neighbor，release 7ms，debug ~50ms，肉眼基本无差异。
    let thumb_img = img.thumbnail(240, 240);
    let thumb_rgba = thumb_img.to_rgba8();
    let thumb_chain = parse_image_fallbacks(octopus_infra::consts::THUMB_SAVE_QUALITY);
    let (tw, th) = (thumb_rgba.width(), thumb_rgba.height());
    let thumb_blob = thumb_chain
        .iter()
        .find_map(|attempt| {
            let dyn_thumb = ::image::DynamicImage::ImageRgba8(thumb_rgba.clone());
            attempt.try_encode(&dyn_thumb, &thumb_rgba)
        })
        .ok_or_else(|| anyhow::anyhow!("All thumb encoding failed for {}×{}", tw, th))?;
    log::info!("[clipboard] thumb encoded: {} bytes ({}×{})", thumb_blob.len(), tw, th);

    Ok(EncodedImage { image_blob, thumb_blob })
}

/// 写图片文件到文件系统（`<screens_dir>/<hash>.jpg`）。
/// 目录不存在时自动创建（mkdir -p）。同 hash 覆盖（幂等）。
pub fn save_image_to_file(hash: &str, blob: &[u8]) -> Result<()> {
    let path = octopus_infra::paths::image_file_path(hash);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, blob)?;
    Ok(())
}

/// 读图片文件（原图）。文件不存在返回 Ok(None)（与旧 DB 查询语义一致）。
pub fn read_image_file(hash: &str) -> Result<Option<Vec<u8>>> {
    let path = octopus_infra::paths::image_file_path(hash);
    match std::fs::read(&path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// 删除图片文件（引用计数归零时调）。文件不存在静默成功（幂等）。
pub fn delete_image_file(hash: &str) {
    let path = octopus_infra::paths::image_file_path(hash);
    if path.exists() {
        if let Err(e) = std::fs::remove_file(&path) {
            log::warn!("[clipboard] 删除图片文件失败 {:?}: {}", path, e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dedup_same_image() {
        let rgba = vec![255, 0, 0, 255, 0, 255, 0, 255];
        let hash1 = hash_rgba(&rgba);
        let hash2 = hash_rgba(&rgba);
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_encode_image() {
        let rgba = vec![255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255];
        let img = ::image::DynamicImage::ImageRgba8(
            ::image::RgbaImage::from_raw(2, 2, rgba).unwrap()
        );
        let encoded = encode_image(&img).unwrap();
        assert!(!encoded.image_blob.is_empty());
        assert!(!encoded.thumb_blob.is_empty());
        // 主 blob 按 IMAGE_SAVE_QUALITY 链首个成功格式（默认 jpeg:100 → SOI magic [FF D8 FF]）
        // 缩略图按 THUMB_SAVE_QUALITY 链（默认 jpeg:10 → 也是 SOI magic）
        // 链可配置，不强制 magic；测试只验证非空 + 至少匹配已知 magic 之一
        for blob in [&encoded.image_blob, &encoded.thumb_blob] {
            let head = &blob[..4];
            let is_jpeg = head.starts_with(&[0xFF, 0xD8, 0xFF]);
            let is_webp = head == b"RIFF";
            assert!(is_jpeg || is_webp, "blob head = {:02X?}", head);
        }
    }

    #[test]
    fn test_parse_image_fallbacks() {
        // 标准常量解析为编码链（2026-07-30 起默认 jpeg:100，无 fallback）
        let chain = parse_image_fallbacks(octopus_infra::consts::IMAGE_SAVE_QUALITY);
        assert_eq!(chain.len(), 1);
        assert!(matches!(chain[0], EncodeAttempt::Jpeg(100)));

        // thumb 链（默认 jpeg:10）
        let thumb_chain = parse_image_fallbacks(octopus_infra::consts::THUMB_SAVE_QUALITY);
        assert_eq!(thumb_chain.len(), 1);
        assert!(matches!(thumb_chain[0], EncodeAttempt::Jpeg(10)));

        // 容错：空白容忍、未知格式跳过、质量非数字跳过
        assert_eq!(parse_image_fallbacks(" webp : 70 ; png:90 ; jpeg:60 ; bad").len(), 2);
        assert!(parse_image_fallbacks("").is_empty());
    }
}
