//! 断点续传 sidecar：<dest>.part.resume.json。
//! 记录各段 downloaded + total + url_hash（基于 dest 路径，镜像无关）。
//! 原子写（tmp+rename），加载时三重校验。

use std::path::{Path, PathBuf};
use sha2::{Sha256, Digest};

use crate::core::segment::Segment;

const SIDECAR_TYPE: &str = "octopus-segmented";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ResumeState {
    pub r#type: String,
    pub url_hash: String,
    pub total_bytes: u64,
    pub segments: Vec<Segment>,
}

/// dest 路径的稳定 hash（镜像无关）。前 16 hex 字符。
pub(crate) fn dest_hash(dest: &Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(dest.to_string_lossy().as_bytes());
    let hex = hasher.finalize();
    hex.iter().take(8).map(|b| format!("{:02x}", b)).collect::<String>()
}

/// sidecar 文件路径：<dest>.part.resume.json
pub(crate) fn sidecar_path(dest: &Path) -> PathBuf {
    let mut p = dest.as_os_str().to_os_string();
    p.push(".part.resume.json");
    PathBuf::from(p)
}

/// 原子写 sidecar：写 .tmp → fsync → rename → fsync 父目录（best-effort）。
///
/// #11 对齐修复（2026-08-03）：原实现 `std::fs::write` + `rename` 无 fsync，
/// 与 vault/keychain.rs save_machine_key 的 #11 修复不对称。断电恰在 rename 后
/// （目录项已更新但数据未落盘）→ sidecar 空/半 → JSON 解析失败 → 续传进度丢失。
/// POSIX `rename(2)` 只原子地切目录项，内容持久化需 fsync；目录项本身的更新
/// （rename 的可见性）需 fsync 父目录才抗断电。详见 keychain.rs:347-401 注释。
pub fn save(dest: &Path, state: &ResumeState) -> std::io::Result<()> {
    use std::io::Write;
    let path = sidecar_path(dest);
    let mut tmp = path.clone();
    tmp.set_extension("json.tmp");
    let bytes = serde_json::to_vec(state)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    // 1. 写 tmp + fsync 内容（保证 rename 前数据已落盘）
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(&bytes)?;
        f.sync_all()?;
    }
    // 2. rename（原子切目录项）
    std::fs::rename(&tmp, &path)?;
    // 3. fsync 父目录（best-effort）：保证 rename 的目录项更新也落盘，扛断电。
    //    失败不阻断——与 keychain.rs #11 修复一致（目录 fsync 在某些 FS 上不可用）。
    if let Some(parent) = path.parent() {
        if let Ok(dir) = std::fs::File::open(parent) {
            let _ = dir.sync_all();
        }
    }
    Ok(())
}

/// 加载 sidecar 并三重校验。任一不符返回 None（调用方丢弃、重新规划）。
/// 校验：type == SIDECAR_TYPE && total_bytes == expected_total && url_hash == dest_hash(dest)。
pub fn load(dest: &Path, expected_total: u64) -> Option<ResumeState> {
    let path = sidecar_path(dest);
    let bytes = std::fs::read(&path).ok()?;
    let state: ResumeState = serde_json::from_slice(&bytes).ok()?;
    let expect_hash = dest_hash(dest);
    if state.r#type == SIDECAR_TYPE
        && state.total_bytes == expected_total
        && state.url_hash == expect_hash
    {
        Some(state)
    } else {
        None
    }
}

/// 删除 sidecar（下载成功或致命错误后）。
pub fn remove(dest: &Path) {
    let _ = std::fs::remove_file(sidecar_path(dest));
}

/// 从已有参数造一个 ResumeState（初始 downloaded 由调用方设置的 segments 决定）。
///
/// 第三十五轮 P2-3：删除 etag 字段——downloader 从未实现 If-Range（etag 探测后存
/// sidecar 但续传时不发头、不比对），是纯 dead code。对齐 verify.rs 第二十三轮 P2-dl1
/// 删 Etag 变体的先例（所有 manifest 配 Sha256，无一配 Etag）。
pub(crate) fn new_state(dest: &Path, total_bytes: u64, segments: Vec<Segment>) -> ResumeState {
    ResumeState {
        r#type: SIDECAR_TYPE.to_string(),
        url_hash: dest_hash(dest),
        total_bytes,
        segments,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn seg(begin: u64, end: u64, downloaded: u64) -> Segment {
        Segment { begin, end, downloaded }
    }

    #[test]
    fn save_load_roundtrip_passes_triple_check() {
        let dir = tempdir().unwrap();
        let dest = dir.path().join("model.onnx");
        let state = new_state(&dest, 1000, vec![seg(0, 999, 300)]);
        save(&dest, &state).unwrap();
        let loaded = load(&dest, 1000).expect("三重校验应通过");
        assert_eq!(loaded.segments.len(), 1);
        assert_eq!(loaded.segments[0].downloaded, 300);
    }

    #[test]
    fn load_total_mismatch_returns_none() {
        let dir = tempdir().unwrap();
        let dest = dir.path().join("model.onnx");
        save(&dest, &new_state(&dest, 1000, vec![seg(0, 999, 0)])).unwrap();
        assert!(load(&dest, 2000).is_none(), "total 不符应丢弃");
    }

    #[test]
    fn load_wrong_type_returns_none() {
        let dir = tempdir().unwrap();
        let dest = dir.path().join("model.onnx");
        let mut state = new_state(&dest, 1000, vec![seg(0, 999, 0)]);
        state.r#type = "something-else".into();
        // 直接写坏 type
        let path = sidecar_path(&dest);
        std::fs::write(&path, serde_json::to_vec(&state).unwrap()).unwrap();
        assert!(load(&dest, 1000).is_none(), "type 不符应丢弃");
    }

    #[test]
    fn load_missing_file_returns_none() {
        let dir = tempdir().unwrap();
        let dest = dir.path().join("nope.onnx");
        assert!(load(&dest, 1000).is_none());
    }

    #[test]
    fn dest_hash_stable_and_mirror_invariant() {
        let p = Path::new("/a/b/onnx/model.onnx");
        assert_eq!(dest_hash(p).len(), 16);
        assert_eq!(dest_hash(p), dest_hash(p), "稳定");
    }

    #[test]
    fn remove_deletes_sidecar() {
        let dir = tempdir().unwrap();
        let dest = dir.path().join("model.onnx");
        save(&dest, &new_state(&dest, 1000, vec![seg(0, 999, 0)])).unwrap();
        assert!(sidecar_path(&dest).exists());
        remove(&dest);
        assert!(!sidecar_path(&dest).exists());
    }
}
