//! 完整性校验：SHA256 流式 hash（spawn_blocking）。
//!
//! 注：Etag 变体保留用于未来 If-Range 续传校验（衔接第 17/23 轮留后续 P2-dl1），
//! 当前 downloader 不发 If-Range 头，Etag 校验实际为 no-op（见 [`Hash::Etag`] 文档）。

use std::path::Path;
use std::io::Read;
use sha2::{Sha256, Digest};
use tokio::task;

/// 期望校验值。Sha256 为 hex 字符串；Etag 为 opaque 字符串。
#[derive(Debug, Clone)]
pub enum Hash {
    Sha256(String),
    /// ⚠️ 当前**不校验**——etag 无法本地重算，需通过下载请求层 If-Range 头让服务端比对
    /// （206=匹配继续，200=不匹配从头）。downloader 尚未实现 If-Range（第 17/23 轮
    /// 留后续 P2-dl1），故 [`verify`] 对 Etag 返回 `Ok(true)` 占位。manifest 仅配 Etag
    /// （无 Sha256）时完整性校验真空——建议 manifest 同时配 Sha256 强校验。
    Etag(String),
}

/// 流式算文件 SHA256，返回 hex。用 spawn_blocking 避免阻塞 runtime。
pub async fn compute_sha256(path: &Path) -> std::io::Result<String> {
    let path = path.to_path_buf();
    task::spawn_blocking(move || -> std::io::Result<String> {
        let mut file = std::fs::File::open(&path)?;
        let mut hasher = Sha256::new();
        let mut buf = [0u8; 8192];
        loop {
            let n = file.read(&mut buf)?;
            if n == 0 { break; }
            hasher.update(&buf[..n]);
        }
        Ok(hasher.finalize().iter().map(|b| format!("{:02x}", b)).collect())
    }).await.map_err(std::io::Error::other)?
}

/// 校验文件是否符合期望 hash。
///
/// - `Sha256`：本地重算 hex 比对（强校验）
/// - `Etag`：**当前 no-op 返回 Ok(true)**——见 [`Hash::Etag`] 文档，需 If-Range 实现
pub async fn verify(path: &Path, expected: &Hash) -> Result<bool, std::io::Error> {
    match expected {
        Hash::Sha256(expected_hex) => {
            let actual = compute_sha256(path).await?;
            Ok(actual.eq_ignore_ascii_case(expected_hex))
        }
        Hash::Etag(_) => {
            // etag 无法本地重算。If-Range 续传校验未实现（第 17/23 轮留后续 P2-dl1），
            // 当前作为"已标记通过"占位。manifest 应同时配 Sha256 保证完整性。
            let _ = path;
            Ok(true)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn sha256_known_vector() {
        // "abc" 的 SHA256
        let dir = tempdir().unwrap();
        let p = dir.path().join("f");
        std::fs::write(&p, b"abc").unwrap();
        let h = compute_sha256(&p).await.unwrap();
        assert_eq!(
            h,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[tokio::test]
    async fn verify_sha256_match_and_mismatch() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("f");
        std::fs::write(&p, b"abc").unwrap();
        let good = Hash::Sha256(
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".into(),
        );
        let bad = Hash::Sha256("0000000000000000000000000000000000000000000000000000000000000000".into());
        assert!(verify(&p, &good).await.unwrap());
        assert!(!verify(&p, &bad).await.unwrap());
    }
}
