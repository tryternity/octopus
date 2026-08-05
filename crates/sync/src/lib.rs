//! octopus-sync —— 通用 git 同步基础设施（2026-07-22 抽离）。
//!
//! 从 `octopus-vault::sync` 抽出的通用同步代码，与具体业务数据（cipher/folder）
//! 无关。vault crate 通过依赖 octopus-sync 复用；未来 hotword/prompts 同步也在此扩展。
//!
//! ## 职责分层
//!
//! - **本 crate 提供**：git 命令 wrapper / outline 索引结构 + merge / SyncError /
//!   私有库检测 / `sync_root` 路径 / `shard_dir` 分桶 / `sha256_hex` + `md5_hex`
//!   hash 工具 / `iso_to_unix_ms` 时间转换
//! - **vault crate 提供**：vault 业务数据文件格式（MetaFile / CipherFile / FolderFile）
//!   + cipher/folder 的 md5 指纹 + vault sync engine
//! - **hotword 模块（本 crate）**：热词同步（两级 outline + HotwordSetMeta / HotwordWordFile + engine）
//! - **clipboard 模块（本 crate）**：剪贴板收藏同步（clipboard.key AES-256-GCM 加密 + outline + favorites/<2hex>/<uuid>.json）。clip 加密原语内联在本 crate（vault 已依赖 sync，不能反向依赖 vault）
//!
//! ## 跨 crate 依赖方向
//!
//! ```text
//! infra ← sync ← vault ← desktop
//!               ↑
//!               hotword 模块（依赖 infra 的 HotwordSet struct）
//! ```
//!
//! 详见 spec：`docs/superpowers/specs/2026-07-21-vault-git-sync-design.md`

pub mod clipboard;
pub mod error;
pub mod git;
pub mod hotword;
pub mod outline;
pub mod privacy;
pub mod store;

pub use error::{classify_git_error, SyncError, SyncResult};
pub use outline::{merge_outlines, Outline, OutlineEntry};
