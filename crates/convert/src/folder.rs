//! 多文件/文件夹合并（spec §4.1 复用设计 + §4.2 守卫 + §4.3 文档形态）。
//! convert_folder 与 convert_files 共用 convert_one + merge_sections——一条转换核心。

use crate::convert::{convert_one, FileSection};
use crate::error::ConvertError;
use std::path::{Path, PathBuf};

/// 文件夹递归守卫（spec §4.2）。
pub const MAX_FILES: usize = 200;
pub const MAX_TOTAL_BYTES: u64 = 50 * 1024 * 1024;
/// 忽略目录（隐藏文件/目录另行按 `.` 前缀跳过）。
pub const IGNORED_DIRS: &[&str] = &[
    ".git", "node_modules", "target", "__pycache__", ".venv", "dist", "build",
];

fn is_ignored(entry: &walkdir::DirEntry) -> bool {
    let name = entry.file_name().to_string_lossy();
    if entry.file_type().is_dir() {
        name.starts_with('.') || IGNORED_DIRS.contains(&name.as_ref())
    } else {
        name.starts_with('.')
    }
}

/// 递归收集文件（排序确定：路径字典序）+ 数量/体积守卫。
pub(crate) fn collect_files(root: &Path) -> Result<Vec<PathBuf>, ConvertError> {
    let mut out = Vec::new();
    for entry in walkdir::WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || !is_ignored(e))
    {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue, // 单个 entry 读取失败（权限等）跳过不中断
        };
        if entry.file_type().is_file() {
            out.push(entry.into_path());
        }
    }
    out.sort();
    if out.len() > MAX_FILES {
        return Err(ConvertError::TooManyFiles { count: out.len(), max: MAX_FILES });
    }
    let total: u64 = out
        .iter()
        .filter_map(|p| std::fs::metadata(p).ok())
        .map(|m| m.len())
        .sum();
    if total > MAX_TOTAL_BYTES {
        return Err(ConvertError::TooLarge { bytes: total, max_bytes: MAX_TOTAL_BYTES });
    }
    Ok(out)
}

/// 文件树（缩进路径形式，确定性输出，spec §4.3）。
fn render_tree(root_name: &str, rel_paths: &[String]) -> String {
    let mut out = format!("{}/\n", root_name);
    for rel in rel_paths {
        let depth = rel.matches('/').count();
        let name = rel.rsplit('/').next().unwrap_or(rel);
        out.push_str(&"  ".repeat(depth + 1));
        out.push_str(name);
        out.push('\n');
    }
    out
}

/// 唯一合并单元（spec §4.1）：树头 + 逐节输出 + 失败节降级 skipped 标注。
fn merge_sections(root_name: &str, sections: &[FileSection]) -> String {
    let rel_paths: Vec<String> = sections.iter().map(|s| s.rel_path.clone()).collect();
    let mut out = format!(
        "# {}\n\n## 文件树\n\n```\n{}```\n",
        root_name,
        render_tree(root_name, &rel_paths)
    );
    for s in sections {
        out.push_str(&format!("\n## {}\n\n", s.rel_path));
        match &s.content {
            Ok(c) => {
                out.push_str(c);
                out.push('\n');
            }
            Err(e) => out.push_str(&format!("> ⚠️ skipped（{}）\n", e)),
        }
    }
    out
}

fn common_parent_name(paths: &[PathBuf]) -> Option<String> {
    let parent = paths.first()?.parent()?;
    parent.file_name().map(|n| n.to_string_lossy().to_string())
}

/// 多文件入口（spec §4.1）：单文件直接输出内容（无树头）；多文件合并（标题=公共父目录名）。
pub fn convert_files(paths: &[PathBuf]) -> Result<String, ConvertError> {
    if paths.is_empty() {
        return Err(ConvertError::Empty);
    }
    if paths.len() == 1 {
        let p = &paths[0];
        let name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
        return convert_one(p, &name).content;
    }
    let root_name = common_parent_name(paths).unwrap_or_else(|| "文件".to_string());
    let sections: Vec<FileSection> = paths
        .iter()
        .map(|p| {
            let name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
            convert_one(p, &name)
        })
        .collect();
    Ok(merge_sections(&root_name, &sections))
}

/// 文件夹入口：递归收集 → 与 convert_files 共用 convert_one/merge_sections。
pub fn convert_folder(root: &Path) -> Result<String, ConvertError> {
    let files = collect_files(root)?;
    if files.is_empty() {
        return Err(ConvertError::Empty);
    }
    let root_name = root.file_name().unwrap_or_default().to_string_lossy().to_string();
    let sections: Vec<FileSection> = files
        .iter()
        .map(|p| {
            let rel = p.strip_prefix(root).unwrap_or(p).to_string_lossy().to_string();
            convert_one(p, &rel)
        })
        .collect();
    Ok(merge_sections(&root_name, &sections))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("octopus-convert-folder-{}-{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(rel_root: &Path, rel: &str, bytes: &[u8]) -> PathBuf {
        let p = rel_root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, bytes).unwrap();
        p
    }

    #[test]
    fn test_convert_files_single_no_tree_header() {
        let root = tmp_root("single");
        let p = write(&root, "a.md", b"only one");
        let md = convert_files(&[p]).unwrap();
        assert_eq!(md, "only one");
        assert!(!md.contains("文件树"), "单文件不应有树头");
    }

    #[test]
    fn test_convert_files_multi_merge_with_tree() {
        let root = tmp_root("multi");
        let a = write(&root, "a.py", b"print(1)");
        let b = write(&root, "b.md", b"doc");
        let md = convert_files(&[a, b]).unwrap();
        assert!(md.starts_with(&format!("# {}\n", root.file_name().unwrap().to_string_lossy())));
        assert!(md.contains("## 文件树"));
        assert!(md.contains("## a.py"));
        assert!(md.contains("```python"));
        assert!(md.contains("## b.md"));
        assert!(md.contains("doc"));
    }

    #[test]
    fn test_convert_folder_recursive_sorted_and_ignored() {
        let root = tmp_root("recur");
        write(&root, "sub/deep/note.txt", b"deep");
        write(&root, "z_last.py", b"x=1");
        write(&root, "a_first.md", b"first");
        write(&root, ".hidden.md", b"no");
        write(&root, "node_modules/junk.js", b"no");
        write(&root, ".git/config", b"no");
        let md = convert_folder(&root).unwrap();
        // 隐藏/垃圾目录不出现
        assert!(!md.contains("hidden"), "隐藏文件应被排除");
        assert!(!md.contains("junk"), "node_modules 应被排除");
        assert!(!md.contains(".git"), ".git 应被排除");
        // 递归 + 排序确定（a_first.md < sub/... < z_last.py）
        let i_first = md.find("## a_first.md").unwrap();
        let i_deep = md.find("## sub/deep/note.txt").unwrap();
        let i_last = md.find("## z_last.py").unwrap();
        assert!(i_first < i_deep && i_deep < i_last);
        assert!(md.contains("## 文件树"));
    }

    #[test]
    fn test_convert_folder_binary_skipped_not_fatal() {
        let root = tmp_root("skipped");
        write(&root, "ok.md", b"fine");
        write(&root, "img.bin", b"\x00\x01");
        let md = convert_folder(&root).unwrap();
        assert!(md.contains("## ok.md"));
        assert!(md.contains("> ⚠️ skipped"), "二进制应标注 skipped，md={}", md);
    }

    #[test]
    fn test_collect_files_max_files_guard() {
        let root = tmp_root("toomany");
        for i in 0..=MAX_FILES {
            write(&root, &format!("f{:03}.txt", i), b"x");
        }
        let err = convert_folder(&root).unwrap_err();
        assert!(err.to_string().contains(&format!("{} 个文件超出上限", MAX_FILES + 1)));
    }

    #[test]
    fn test_convert_empty_inputs() {
        assert!(matches!(
            convert_files(&[]).unwrap_err(),
            ConvertError::Empty
        ));
        let empty_root = tmp_root("emptydir");
        assert!(matches!(
            convert_folder(&empty_root).unwrap_err(),
            ConvertError::Empty
        ));
    }
}
