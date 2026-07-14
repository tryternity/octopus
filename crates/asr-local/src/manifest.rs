//! 模型文件清单（manifest）：记录模型目录下各文件的 sha256 + 大小，
//! 序列化为 JSON map（`{<path>: {"sha256","size"}, ...}`）存 DB `models.secret_key`
//!（local 模型重载该字段；api 模型仍是 API key），供完整性复核（detect 损坏/缺失）。
//!
//! - [`bootstrap_manifest`]：遍历目录常规文件（follow symlink，适配 HF cache snapshot
//!   下指向 blobs 的符号链接），算 sha256 生成清单 JSON。
//! - [`verify_against_manifest`]：按清单逐文件比对，返回损坏/缺失的相对路径。
//!
//! desktop 模型管理页（download/verify_model）与 cli `sync-models` 共用本模块。

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::{BufReader, Read};
use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// 文件清单条目：sha256 + size（path 是 [`Manifest`] map 的 key）。
#[derive(Serialize, Deserialize)]
pub struct ManifestFile {
    /// 下载来源 URL（支持 {*} 模板）。bootstrap 生成时为空串。
    pub source: String,
    pub sha256: String,
    pub size: u64,
}

/// 文件清单：`path → {sha256, size}`。BTreeMap 保证 key 字母序（输出稳定、diff 友好）。
pub type Manifest = BTreeMap<String, ManifestFile>;

/// 遍历目录常规文件（递归，跳过隐藏，follow symlink 读实际内容），生成清单 JSON。
pub fn bootstrap_manifest(dir: &Path) -> Result<String> {
    let mut map: Manifest = BTreeMap::new();
    collect_files(dir, dir, &mut map)?;
    Ok(serde_json::to_string(&map)?)
}

fn collect_files(root: &Path, dir: &Path, out: &mut Manifest) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue; // 跳过隐藏文件/目录（.DS_Store 等）
        }
        let path = entry.path();
        // is_file()/is_dir() 会 follow symlink，适配 HF snapshot 结构。
        if path.is_file() {
            let rel = path.strip_prefix(root)?.to_string_lossy().to_string();
            // 流式哈希 + metadata 取大小（不整文件读入，避免大模型文件的内存尖峰/OOM）。
            let size = std::fs::metadata(&path)?.len(); // follow symlink，实际字节数
            let sha256 = hex_sha256_file(&path)?;
            out.insert(rel, ManifestFile { source: String::new(), sha256, size });
        } else if path.is_dir() {
            collect_files(root, &path, out)?;
        }
    }
    Ok(())
}

/// 按 manifest 复核 dir 下文件，返回损坏/缺失的相对路径列表。
pub fn verify_against_manifest(dir: &Path, manifest: &Manifest) -> Vec<String> {
    manifest
        .iter()
        .filter_map(|(path, f)| {
            let full = dir.join(path);
            let ok = hex_sha256_file(&full)
                .ok()
                .map(|h| h == f.sha256)
                .unwrap_or(false);
            if ok { None } else { Some(path.clone()) }
        })
        .collect()
}

/// 流式计算文件 sha256（64KB 缓冲循环 update）。
///
/// 不用 `std::fs::read` 整文件读入——ASR 模型文件常达数百 MB ~ GB，整读会在客户端
/// 产生数倍于文件大小的临时堆分配尖峰，内存敏感机器易 OOM。流式版内存占用恒定
///（~64KB 缓冲），与文件大小无关。File::open / metadata 均 follow symlink，
/// 与原 `fs::read` 语义一致（适配 HF cache snapshot 下指向 blobs 的符号链接）。
fn hex_sha256_file(path: &Path) -> Result<String> {
    let mut hasher = Sha256::new();
    let mut reader = BufReader::with_capacity(64 * 1024, std::fs::File::open(path)?);
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let bytes = hasher.finalize();
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        write!(s, "{:02x}", b)?;
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    /// 造文件 → 清单为 map（path→{sha256,size}），隐藏文件跳过，key 字母序。
    #[test]
    fn bootstrap_manifest_hashes_files() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.onnx"), b"hello").unwrap();
        fs::write(dir.path().join("b.txt"), b"world!").unwrap();
        fs::write(dir.path().join(".DS_Store"), b"junk").unwrap(); // 应跳过
        let json = bootstrap_manifest(dir.path()).unwrap();
        let m: Manifest = serde_json::from_str(&json).unwrap();
        assert_eq!(m.len(), 2, "隐藏文件应跳过");
        let keys: Vec<&str> = m.keys().map(|s| s.as_str()).collect();
        assert_eq!(keys, vec!["a.onnx", "b.txt"], "BTreeMap 字母序");
        let a = m.get("a.onnx").unwrap();
        // sha256("hello") 标准已知值
        assert_eq!(
            a.sha256,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
        assert_eq!(a.size, 5);
    }

    /// 新格式（含 source）能正确反序列化。
    #[test]
    fn manifest_file_deserializes_with_source() {
        let json = r#"{"a.onnx":{"source":"https://x.com/a.onnx","sha256":"abc","size":123}}"#;
        let m: Manifest = serde_json::from_str(json).unwrap();
        let f = m.get("a.onnx").unwrap();
        assert_eq!(f.source, "https://x.com/a.onnx");
        assert_eq!(f.sha256, "abc");
        assert_eq!(f.size, 123);
    }

    /// 未篡改空；篡改/删除返回损坏清单。
    #[test]
    fn verify_detects_tamper() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.onnx"), b"hello").unwrap();
        let mut manifest = Manifest::new();
        manifest.insert(
            "a.onnx".into(),
            ManifestFile {
                source: String::new(),
                sha256: "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824".into(),
                size: 5,
            },
        );
        assert!(verify_against_manifest(dir.path(), &manifest).is_empty());
        // 篡改
        fs::write(dir.path().join("a.onnx"), b"HACKED").unwrap();
        assert_eq!(
            verify_against_manifest(dir.path(), &manifest),
            vec!["a.onnx".to_string()]
        );
        // 删除
        fs::remove_file(dir.path().join("a.onnx")).unwrap();
        assert_eq!(
            verify_against_manifest(dir.path(), &manifest),
            vec!["a.onnx".to_string()]
        );
    }
}
