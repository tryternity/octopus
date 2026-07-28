//! 测试专用辅助函数（`#[cfg(test)]`，仅测试编译）。
//!
//! 统一 zipformer / streaming_zipformer 的 HuggingFace 模型缓存查找逻辑。
//!（streaming_paraformer 保留自己的 `hf_snapshot`，因为接口不同——返回 test_wavs 而非 snapshot 目录。）

/// 查找 HuggingFace Hub 本地缓存的模型 snapshot 目录。
///
/// `repo` 需含 `models--` 前缀（HF cache 目录名），如
/// `models--csukuangfj--sherpa-onnx-streaming-zipformer-zh-int8-2025-06-30`。
///
/// 路径模式：`~/.cache/huggingface/hub/<repo>/snapshots/<hash>`
/// 返回第一个 snapshot 目录（若存在）；无缓存 → None。
/// 调用方按需 `.join("test_wavs")` 或其它子目录。
///
/// real_model 测试用它判断是否 skip（`#[ignore]` 后此函数仅在 `--ignored` 跑时调用）。
pub(crate) fn hf_snapshot(repo: &str) -> Option<std::path::PathBuf> {
    let snapshots = std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default())
        .join(".cache/huggingface/hub")
        .join(repo)
        .join("snapshots");
    if !snapshots.is_dir() {
        return None;
    }
    // 取 snapshots 下第一个子目录（HF 每次拉取用 commit hash 命名）
    std::fs::read_dir(&snapshots)
        .ok()?
        .filter_map(|e| e.ok())
        .find_map(|e| {
            let p = e.path();
            if p.is_dir() { Some(p) } else { None }
        })
}
