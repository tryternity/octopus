//! Vault Git 同步模块（2026-07-21 Phase 1）。
//!
//! 用 git repo（GitHub/Gitee private repo）作为密码箱同步后端。SSH key 认证
//! （系统已配），octopus 完全不接触凭证。
//!
//! 存储布局（`~/.octopus/.sync/vault/`）：
//! ```text
//! ~/.octopus/.sync/                   git repo 根（octopus_sync::store::sync_root）
//! ├── .git/
//! └── vault/                          vault 数据子目录（vault_dir）
//!     ├── meta.json                   vault_meta 同步字段
//!     ├── outline.json                增量索引（uuid → md5）
//!     ├── ciphers/<2hex>/<uuid>.json  单 cipher 加密 blob
//!     └── folders/<2hex>/<uuid>.json  folder 加密 blob
//! ```
//!
//! 加密层复用现有 user_vault_key + AES-256-GCM（零改动）——文件存储格式与 SQLite
//! 完全一致（v1: 前缀密文）。
//!
//! ## 2026-07-22 抽离说明
//!
//! 通用 sync 代码（git wrapper / outline / error / privacy / store 工具）已抽到
//! 独立 `octopus-sync` crate。本模块只保留 vault 业务数据相关：
//! - `store.rs`：vault 文件格式（MetaFile / CipherFile / FolderFile）
//! - `fingerprint.rs`：cipher/folder 的 md5 指纹
//! - `engine.rs`：vault sync 引擎（pull/push/enable/disable）
//!
//! 详见 spec：`docs/superpowers/specs/2026-07-21-vault-git-sync-design.md`

pub mod engine;
pub mod fingerprint;
pub mod store;

// re-export：通用 sync 类型从 octopus_sync 透传，方便外部用 `octopus_vault::sync::SyncError` 等
pub use octopus_sync::error::{classify_git_error, SyncError, SyncResult};
pub use octopus_sync::git;
pub use octopus_sync::outline::{merge_outlines, Outline, OutlineEntry};
pub use octopus_sync::privacy;
// octopus_sync::store 不 re-export（与本模块的 vault::sync::store 同名冲突）——
// 需用 sync crate 通用工具时走全路径 `octopus_sync::store::sync_root` 等

pub use engine::{
    add_remote, clone_from, disable_sync, enable_sync, get_sync_status, list_remotes,
    remove_remote, sync_now, test_connection, SyncReport, SyncStatus,
};
pub use store::{
    cipher_file_path, folder_file_path, meta_path, outline_path, vault_dir, vault_root,
    CipherFile, FolderFile, MetaFile,
};

