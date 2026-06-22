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
use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// 文件清单条目：sha256 + size（path 是 [`Manifest`] map 的 key）。
#[derive(Serialize, Deserialize)]
pub struct ManifestFile {
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
            let data = std::fs::read(&path)?; // follow symlink 读实际字节
            out.insert(
                rel,
                ManifestFile {
                    sha256: hex_sha256(&data),
                    size: data.len() as u64,
                },
            );
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
            let ok = std::fs::read(&full)
                .ok()
                .map(|d| hex_sha256(&d) == f.sha256)
                .unwrap_or(false);
            if ok { None } else { Some(path.clone()) }
        })
        .collect()
}

fn hex_sha256(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let bytes = hasher.finalize();
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        write!(s, "{:02x}", b).unwrap();
    }
    s
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

    /// 未篡改空；篡改/删除返回损坏清单。
    #[test]
    fn verify_detects_tamper() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.onnx"), b"hello").unwrap();
        let mut manifest = Manifest::new();
        manifest.insert(
            "a.onnx".into(),
            ManifestFile {
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
