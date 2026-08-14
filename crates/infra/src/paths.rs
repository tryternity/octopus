// crates/infra/src/paths.rs
// 路径工具：跨 crate 共享的根目录定位。
// asr / llm / dlp / desktop / cli / server 统一调用，不再各自定义。

use once_cell::sync::Lazy;
use std::path::{Path, PathBuf};

/// $HOME/.octopus — 全局根目录，所有配置 / 模型 / 数据都基于此。
static OCTOPUS_HOME: Lazy<PathBuf> = Lazy::new(|| {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".octopus")
});

/// 获取 ~/.octopus 路径（Lazy 缓存，进程内首次调用后固定）。
pub fn octopus_config_home() -> &'static Path {
    OCTOPUS_HOME.as_path()
}

// ── 录屏 ───────────────────────────────────────────────────────────

/// 录屏输出目录：读 DB `record_output_dir` 配置（绝对路径，支持 `~` 展开）。
/// 空/未配置时 fallback `~/Documents/octopus/recordings/`。
/// 不存在时由调用方在 start_recording 前创建。
pub fn recordings_dir() -> PathBuf {
    let configured = crate::db::load_config_key("record_output_dir")
        .ok()
        .flatten()
        .filter(|s| !s.is_empty());
    match configured {
        Some(dir) => expand_tilde(&dir),
        None => expand_tilde("~/Documents/octopus/recordings"),
    }
}

// ── 截图 / 剪贴板图片 ──────────────────────────────────────────────

/// 图片文件存储目录：读 DB `screen_output_dir` 配置（绝对路径，支持 `~` 展开）。
/// 空/未配置时 fallback `~/Documents/octopus/screens/`。
/// 不存在时由调用方在写入前创建（mkdir -p）。
pub fn screens_dir() -> PathBuf {
    let configured = crate::db::load_config_key("screen_output_dir")
        .ok()
        .flatten()
        .filter(|s| !s.is_empty());
    match configured {
        Some(dir) => expand_tilde(&dir),
        None => expand_tilde("~/Documents/octopus/screens"),
    }
}

/// 图片文件完整路径：`<screens_dir>/<hash>.jpg`。
/// hash 作为文件名天然去重（同图复用同一文件）。
///
/// 第四十八轮 P2-低：补 hash 格式校验——原无校验，sync ref_data 间接可达（虽 payload
/// 加密限制攻击面）。hash 应为 hex（MD5），含 `/` `\\` `..` 可跳出 screens_dir。
/// 对称 clipboard favorite_file_path validate_favorite_uuid + vault validate_uuid。
pub fn image_file_path(hash: &str) -> PathBuf {
    // hash 应为 hex 字符（MD5 = 32 hex）。防御性校验防 path traversal。
    if hash.is_empty() || hash.contains('/') || hash.contains('\\') || hash.contains("..") || hash.contains('\0') {
        log::warn!("[paths] 拒绝非法规格 image hash（path traversal 防御）：{}", hash);
        // 返回一个不存在的安全路径，调用方的文件操作会自然失败
        return screens_dir().join("invalid_hash_rejected.jpg");
    }
    screens_dir().join(format!("{}.jpg", hash))
}

/// 展开 `~` 为 $HOME（macOS/Linux）。已是绝对路径则原样返回。
/// 不引入 shellexpand 依赖——手动展开足够（录屏 macOS-only）。
fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        PathBuf::from(home).join(rest)
    } else if path == "~" {
        PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
    } else {
        PathBuf::from(path)
    }
}

/// 解析 recordings 表里的 file_path 为绝对路径。
/// 2026-07-27 起 file_path 直接存**绝对路径**（用户可配置保存目录），
/// 此函数对绝对路径原样返回；防御性 fallback：相对路径 join octopus_config_home()。
pub fn resolve_recording_path(file_path: &str) -> PathBuf {
    let p = PathBuf::from(file_path);
    if p.is_absolute() {
        p
    } else {
        octopus_config_home().join(p)
    }
}

/// 录屏 helper 子进程的 stdout/stderr 日志路径：~/.octopus/logs/record-helper.log
/// （logs 目录约定与 desktop/action_bar_commands.rs、desktop/perf_log.rs 一致。）
pub fn record_helper_log() -> PathBuf {
    octopus_config_home().join("logs").join("record-helper.log")
}

/// 探测 Tauri `.app` bundle 内 resource 路径（exe-relative 几何）。
///
/// macOS `.app` 结构：exe 在 `Contents/MacOS/<binary>`，Tauri `bundle.resources` 映射的
/// 资源在 `Contents/Resources/<rel>`。本函数传相对路径（如 `seeds` /
/// `binaries/octopus-sck-helper`），命中则返回绝对路径。
///
/// 非 `.app` 环境（`cargo run` dev / 裸二进制 release）返回 `None`——
/// 此时 exe 不在 `Contents/MacOS/` 下，`parent().parent()` 不指向 `Contents`。
///
/// 复用方：
/// - `seeds_dir()`（crates/infra/src/seeds.rs）—— Tauri .app bundle 第 3 路探测
/// - `MacOSProvider::resolve_helper_path`（crates/record/src/platform/macos.rs）—— helper 路径
pub fn tauri_app_resource(rel: &str) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let macos_dir = exe.parent()?;      // Contents/MacOS
    let contents = macos_dir.parent()?; // Contents
    let candidate = contents.join("Resources").join(rel);
    candidate.exists().then_some(candidate)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// dev / 裸二进制环境：current_exe() 不在 .app/Contents/MacOS/ 下，
    /// tauri_app_resource 必须返回 None（不能误报）。
    /// （.app 环境难在单测里构造，只验证 None 分支）
    #[test]
    fn tauri_app_resource_returns_none_in_non_app_env() {
        // cargo test 的 exe 在 target/debug/deps/ 或 target/release/deps/，
        // parent().parent() 是 target/debug 或 target/release，不含 Resources 目录。
        assert!(tauri_app_resource("seeds").is_none());
        assert!(tauri_app_resource("binaries/octopus-sck-helper").is_none());
        assert!(tauri_app_resource("nonexistent").is_none());
    }
}
