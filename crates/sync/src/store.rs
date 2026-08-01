//! 通用文件存储工具——`~/.octopus/.sync/` git repo 路径 + hash 工具 + 分桶。
//!
//! 与具体业务数据（cipher/folder/hotword）无关的通用同步基础设施。各业务数据
//! 的文件格式（MetaFile / CipherFile / HotwordSetMeta / HotwordWordFile 等）留在各自的模块：
//! - vault 业务：`octopus_vault::sync::store`
//! - hotword 业务：`octopus_sync::hotword`（待实现）
//!
//! ## 目录结构
//!
//! ```text
//! ~/.octopus/.sync/             git repo 根（sync_root）
//! ├── .git/
//! ├── vault/                    vault 数据（meta/outline/ciphers/folders，由 vault crate 管理）
//! └── hotword/                  hotword 数据（outline/sets/，待 hotword 模块实现）
//! ```
//!
//! 测试隔离用 thread_local override（与 infra::db::set_test_db 同模式）——
//! 进程内 `octopus_config_home` 是 Lazy 固定值，env var 重定向不生效。

use std::path::PathBuf;

use anyhow::Context;
use sha2::{Digest, Sha256};

// === 测试隔离 ===

// 测试专用：thread_local 覆盖 sync_root（与 infra::db::set_test_db 同模式）。
// 进程内 `octopus_config_home` 是 Lazy 固定值（首次调用后不变），无法用 env var
// 重定向。用 thread_local override 让每个测试线程独立隔离。
// （`thread_local!` 是宏，doc comment 不生效，用普通注释。）
//
// 不用 `#[cfg(test)]` gate——跨 crate 测试（vault/hotword）需要调本函数，
// cfg(test) 只在当前 crate 编译测试时生效，下游 crate 看不到。
// 改为始终 pub + #[doc(hidden)]（与 infra::db::set_test_db 同模式）。
thread_local! {
    static TEST_SYNC_ROOT: std::cell::RefCell<Option<PathBuf>> = std::cell::RefCell::new(None);
}

/// 测试专用：设置临时 sync_root（TempDir 路径）。
///
/// vault crate 测试用 `set_test_vault_root`（保留旧名兼容）—— 本函数是 sync crate
/// 的正名版本，vault 的 `set_test_vault_root` 内部调本函数。
#[doc(hidden)]
pub fn set_test_sync_root(path: PathBuf) {
    TEST_SYNC_ROOT.with(|cell| {
        *cell.borrow_mut() = Some(path);
    });
}

/// 测试专用：清除 sync_root override。
#[doc(hidden)]
pub fn clear_test_sync_root() {
    TEST_SYNC_ROOT.with(|cell| {
        *cell.borrow_mut() = None;
    });
}

// === 路径辅助 ===

/// `~/.octopus/.sync/`——所有同步数据的 git repo 根目录。
///
/// 2026-07-22 抽离：从 `vault::sync::store::sync_root` 搬到 `octopus_sync::store`。
/// vault/hotword 等各业务数据都在此 git repo 下作为子目录（`vault/` / `hotword/`），
/// 一个 sync 同步所有用户数据。
pub fn sync_root() -> PathBuf {
    // 不用 #[cfg(test)] gate——跨 crate 测试（vault/hotword）需要 thread_local override
    // 生效，cfg(test) 只在当前 crate 编译测试时生效，下游 crate 看不到。
    // thread_local 默认 None，生产环境不受影响。
    if let Some(p) = TEST_SYNC_ROOT.with(|cell| cell.borrow().clone()) {
        return p;
    }
    octopus_infra::octopus_config_home().join(".sync")
}

/// 取 uuid 的前 2 个 hex 字符作分桶目录名。
///
/// uuid v4 形如 `a1b2c3d4-...-e5f6`，filter 出 hex 字符取前 2 → `a1`。
/// 256 个桶，每桶 10000 条时平均 40 文件（git ls-tree 毫秒级）。
pub fn shard_dir(uuid: &str) -> String {
    uuid.chars()
        .filter(|c| c.is_ascii_hexdigit())
        .take(2)
        .collect()
}

// === hash 工具 ===

/// 算字符串的 sha256（hex），用于 outline 增量索引。
pub fn sha256_hex(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    let result = hasher.finalize();
    // hex encode
    let mut hex = String::with_capacity(64);
    for byte in result {
        hex.push_str(&format!("{:02x}", byte));
    }
    hex
}

/// 把字节算 md5，返 hex 字符串（32 字符小写）。
///
/// md5 在 sync 模块里**纯粹是内容指纹**，与加密破解无关。用于：
/// - 写 SQLite 时算 md5 存入 `sync_md5` 字段
/// - sync_now 时对比 SQLite.md5 vs outline.md5，决定是否需要重写文件
///
/// 2026-07-22 抽离自 vault::sync::fingerprint::md5_hex（private → pub），
/// 让 hotword 模块也能复用。
pub fn md5_hex(bytes: &[u8]) -> String {
    use md5::{Digest, Md5};
    let mut hasher = Md5::new();
    hasher.update(bytes);
    let result = hasher.finalize();
    let mut s = String::with_capacity(32);
    for b in result.iter() {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

// === 时间转换工具 ===

/// 把 SQLite `datetime('now')` 格式（`"2026-07-21 15:11:22"`）转为 Unix 毫秒。
///
/// SQLite 的 UTC 时间，直接解析为毫秒——outline 用数值比较（旧版 ISO 字符串比较不可靠）。
/// 解析失败返 0（让 merge_outlines 退化到「双方都 0 取本地」语义，安全）。
///
/// 2026-07-22 抽离自 vault::sync::store::iso_to_unix_ms（private → pub）。
pub fn iso_to_unix_ms(s: &str) -> i64 {
    // 格式："2026-07-21 15:11:22" 或 "2026-07-21T15:11:22Z" 等变体
    // 手写解析避免引入 chrono 依赖
    let s = s.trim();
    let bytes = s.as_bytes();
    if bytes.len() < 19 {
        return 0;
    }
    // 日期部分：YYYY-MM-DD HH:MM:SS（位置固定）
    let y: i64 = s[0..4].parse().unwrap_or(1970);
    let mo: i64 = s[5..7].parse().unwrap_or(1);
    let d: i64 = s[8..10].parse().unwrap_or(1);
    let h: i64 = s[11..13].parse().unwrap_or(0);
    let mi: i64 = s[14..16].parse().unwrap_or(0);
    let se: i64 = s[17..19].parse().unwrap_or(0);
    // civil_to_days 公式（Howard Hinnant）——精度无损，正确处理闰年。
    // ISO-COMMENT 修复（2026-07-25）：删旧的「简化、不考虑闰年」残留注释——
    // 那是被淘汰的旧简化方案的描述，与实际代码（完整 civil_to_days）矛盾。
    let y2 = if mo <= 2 { y - 1 } else { y };
    let era = if y2 >= 0 { y2 } else { y2 - 399 } / 400;
    let yoe = (y2 - era * 400) as u64;
    let doy = (153 * ((if mo > 2 { mo - 3 } else { mo + 9 }) as u64) + 2) / 5 + d as u64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe as i64 - 719468;
    let secs = days * 86400 + h * 3600 + mi * 60 + se;
    secs * 1000
}

// === 原子写（2026-08-01，对齐 vault store::write_atomically）===

/// 原子写文件：写临时文件 → fsync → rename → fsync 父目录（POSIX）。
///
/// 搬自 `crates/vault/src/sync/store.rs::write_atomically`（vault 的 private，跨 crate 拿不到，
/// 故在 sync crate 内联一份）。hotword set meta / word 文件都用它，对齐 vault 持久化保证：
/// ①临时文件同目录（同卷 → rename 原子）；②fsync 数据 + 目录项扛断电；
/// ③临时文件名前缀 `.`（隐藏，不被 `collect_json_files` 的 `.json` 扫描命中）。
pub(crate) fn write_atomically(path: &std::path::Path, content: &str) -> Result<(), anyhow::Error> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("创建目录失败：{}", parent.display()))?;
    }
    let file_name = path.file_name().and_then(|s| s.to_str()).unwrap_or("data");
    let tmp_path = path.with_file_name(format!(".{}.tmp", file_name));

    #[cfg(unix)]
    {
        use std::io::Write;
        {
            let mut f = std::fs::File::create(&tmp_path)
                .with_context(|| format!("创建临时文件失败：{}", tmp_path.display()))?;
            f.write_all(content.as_bytes())
                .with_context(|| format!("写入临时文件失败：{}", tmp_path.display()))?;
            f.sync_all()
                .with_context(|| format!("fsync 临时文件失败：{}", tmp_path.display()))?;
        }
        std::fs::rename(&tmp_path, path).with_context(|| {
            format!("原子替换失败：{} -> {}", tmp_path.display(), path.display())
        })?;
        // N3 修复（2026-07-24）：rename 后 fsync 父目录——POSIX 下目录项更新
        // 需 fsync 才能扛断电，否则断电恰在 rename 后可能丢 rename（恢复后看到旧版本）。
        if let Some(parent) = path.parent() {
            if let Ok(dir) = std::fs::File::open(parent) {
                let _ = dir.sync_all(); // 目录 fsync 失败不阻断（best-effort）
            }
        }
    }
    #[cfg(not(unix))]
    {
        use std::io::Write;
        {
            let mut f = std::fs::File::create(&tmp_path)
                .with_context(|| format!("创建临时文件失败：{}", tmp_path.display()))?;
            f.write_all(content.as_bytes())
                .with_context(|| format!("写入临时文件失败：{}", tmp_path.display()))?;
            f.sync_all()
                .with_context(|| format!("fsync 临时文件失败：{}", tmp_path.display()))?;
        }
        std::fs::rename(&tmp_path, path).with_context(|| {
            format!("原子替换失败：{} -> {}", tmp_path.display(), path.display())
        })?;
        // Windows: MoveFileEx(REPLACE_EXISTING) 已保证可见性，无需目录 fsync
    }
    Ok(())
}

// === 自动同步状态持久化（2026-07-22 Phase 2）===

/// 最近一次自动同步结果——存 `~/.octopus/.sync/last_auto_sync.json`。
/// 自动同步（scheduler 每小时触发）成功/失败后写入，SyncPanel 展示。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LastAutoSync {
    /// ISO 8601 时间戳（UTC）。
    pub timestamp: String,
    /// 同步是否成功。
    pub success: bool,
    /// 成功时的 report.message，失败时的 error.to_string()。
    pub message: String,
}

/// `~/.octopus/.sync/last_auto_sync.json` 路径。
fn last_auto_sync_path() -> PathBuf {
    sync_root().join("last_auto_sync.json")
}

/// 读最近一次自动同步状态。文件不存在（从未自动同步）时返 None。
pub fn read_last_auto_sync() -> Option<LastAutoSync> {
    let path = last_auto_sync_path();
    match std::fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str(&content).ok(),
        Err(_) => None,
    }
}

/// 写最近一次自动同步状态（覆盖写）。
pub fn write_last_auto_sync(status: &LastAutoSync) {
    let path = last_auto_sync_path();
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            log::warn!("[sync] 创建 .sync 目录失败：{}", e);
            return;
        }
    }
    match serde_json::to_string_pretty(status) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&path, format!("{}\n", json)) {
                log::warn!("[sync] 写 last_auto_sync.json 失败：{}", e);
            }
        }
        Err(e) => log::warn!("[sync] 序列化 last_auto_sync 失败：{}", e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// RAII guard：测试期间 set_test_sync_root，drop 时 clear。
    struct SyncRootGuard {
        _tmp: TempDir,
    }

    impl SyncRootGuard {
        fn new() -> Self {
            let tmp = TempDir::new().expect("tempdir");
            let sync_path = tmp.path().join(".sync");
            std::fs::create_dir_all(&sync_path).unwrap();
            set_test_sync_root(sync_path);
            Self { _tmp: tmp }
        }
    }

    impl Drop for SyncRootGuard {
        fn drop(&mut self) {
            clear_test_sync_root();
        }
    }

    #[test]
    fn sync_root_default_is_octopus_config_dot_sync() {
        // 不设 test override，返 ~/.octopus/.sync（实际路径用 infra::octopus_config_home）
        clear_test_sync_root();
        let root = sync_root();
        assert!(
            root.ends_with(".sync"),
            "默认 sync_root 应以 .sync 结尾: {}",
            root.display()
        );
    }

    #[test]
    fn sync_root_respects_test_override() {
        let _guard = SyncRootGuard::new();
        let root = sync_root();
        assert!(root.ends_with(".sync"), "test override 后仍应是 .sync 子目录");
        // test override 后 sync_root 的父目录是 tempdir，不应是 ~/.octopus
        let home = octopus_infra::octopus_config_home();
        let root_parent = root.parent().unwrap();
        assert_ne!(
            root_parent,
            home,
            "test override 后 sync_root 应指向 tempdir，而非 ~/.octopus"
        );
    }

    #[test]
    fn shard_dir_takes_first_2_hex() {
        assert_eq!(shard_dir("a1b2c3d4-..."), "a1");
        assert_eq!(shard_dir("abcdef"), "ab");
        assert_eq!(shard_dir("zz12"), "12"); // 非 hex 字符被 filter 掉
    }

    #[test]
    fn sha256_hex_is_deterministic() {
        let h1 = sha256_hex("hello");
        let h2 = sha256_hex("hello");
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64);

        let h3 = sha256_hex("world");
        assert_ne!(h1, h3);
    }

    #[test]
    fn md5_hex_returns_32_chars_lowercase() {
        let h = md5_hex(b"hello");
        assert_eq!(h.len(), 32);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        // md5("hello") 的已知值：5d41402abc4b2a76b9719d911017c592
        assert_eq!(h, "5d41402abc4b2a76b9719d911017c592");
    }

    #[test]
    fn md5_hex_is_deterministic() {
        assert_eq!(md5_hex(b"data"), md5_hex(b"data"));
        assert_ne!(md5_hex(b"data"), md5_hex(b"data2"));
    }

    #[test]
    fn iso_to_unix_ms_parses_sqlite_datetime() {
        // 2026-07-21 00:00:00 UTC 应是某固定毫秒值
        let ms = iso_to_unix_ms("2026-07-21 00:00:00");
        assert!(ms > 1_700_000_000_000, "2026 年的时间戳应 > 1.7e12: {}", ms);
    }

    #[test]
    fn iso_to_unix_ms_later_is_greater() {
        let earlier = iso_to_unix_ms("2026-07-21 10:00:00");
        let later = iso_to_unix_ms("2026-07-21 11:00:00");
        assert!(later > earlier, "晚 1 小时 ms 应更大");
    }

    #[test]
    fn iso_to_unix_ms_invalid_returns_zero() {
        assert_eq!(iso_to_unix_ms(""), 0);
        assert_eq!(iso_to_unix_ms("short"), 0);
        assert_eq!(iso_to_unix_ms("not-a-date-at-all-xxx"), 0);
    }
}
