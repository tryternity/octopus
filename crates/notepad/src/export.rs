//! 导入/导出文件 I/O。落盘到 ~/Documents/octopus/notes/。
//! 格式转换（HTML↔md↔txt）在前端 TipTap，后端只读/写文件。

use anyhow::{Context, Result};
use std::path::PathBuf;

/// 导出根目录：~/Documents/octopus/notes/（跨平台 dirs::document_dir）。
pub fn notes_dir() -> Result<PathBuf> {
    let docs = dirs::document_dir().context("无法定位 Documents 目录")?;
    Ok(docs.join("octopus").join("notes"))
}

/// 把内容写到 ~/Documents/octopus/notes/<safe_stem>.<ext>。
/// stem 中的路径分隔符/非法字符替换为 `_`，避免目录穿越。
/// 文件名冲突时追加 `-2/-3`。返回写入的绝对路径。
pub fn write_export(filename_stem: &str, ext: &str, content: &str) -> Result<PathBuf> {
    let dir = notes_dir()?;
    std::fs::create_dir_all(&dir).with_context(|| format!("创建目录失败: {}", dir.display()))?;
    let safe_stem = sanitize_stem(filename_stem);
    let safe_ext = ext.trim_start_matches('.').to_lowercase();
    let path = unique_path(&dir, &safe_stem, &safe_ext);
    std::fs::write(&path, content)
        .with_context(|| format!("写入文件失败: {}", path.display()))?;
    Ok(path)
}

/// 读 .md 文件原文返回（md→HTML 解析在前端）。
pub fn read_import(path: &std::path::Path) -> Result<String> {
    std::fs::read_to_string(path)
        .with_context(|| format!("读取文件失败: {}", path.display()))
}

fn sanitize_stem(s: &str) -> String {
    let trimmed = s.trim();
    let stem = if trimmed.is_empty() { "note" } else { trimmed };
    stem.chars()
        .map(|c| {
            if c.is_ascii_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect::<String>()
        .trim()
        .to_string()
}

fn unique_path(dir: &std::path::Path, stem: &str, ext: &str) -> PathBuf {
    let first = dir.join(format!("{}.{}", stem, ext));
    if !first.exists() {
        return first;
    }
    for i in 2..1000 {
        let cand = dir.join(format!("{}-{}.{}", stem, i, ext));
        if !cand.exists() {
            return cand;
        }
    }
    dir.join(format!("{}.{}", stem, ext))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("octopus-notepad-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn sanitize_replaces_path_chars() {
        assert_eq!(sanitize_stem("a/b\\c:d"), "a_b_c_d");
        assert_eq!(sanitize_stem("   "), "note");
        assert_eq!(sanitize_stem("正常标题"), "正常标题");
    }

    #[test]
    fn write_and_read_roundtrip() {
        let dir = temp_dir();
        std::fs::create_dir_all(&dir).unwrap();
        // 注意：dirs::document_dir 在 macOS 走系统 API 读真实 ~/Documents，单测无法精准控制；
        // 故直接测 unique_path + read_import 的组合逻辑，write_export 的端到端留 Task 17 集成测试覆盖。
        let p = unique_path(&dir, "我的笔记", "md");
        std::fs::write(&p, "# 标题\n正文").unwrap();
        assert_eq!(read_import(&p).unwrap(), "# 标题\n正文");
        // 冲突 → -2
        let p2 = unique_path(&dir, "我的笔记", "md");
        assert_ne!(p, p2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
