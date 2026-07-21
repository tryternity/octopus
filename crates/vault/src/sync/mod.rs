//! Vault Git 同步模块（2026-07-21 Phase 1）。
//!
//! 用 git repo（GitHub/Gitee private repo）作为密码箱同步后端。SSH key 认证
//! （系统已配），octopus 完全不接触凭证。
//!
//! 存储布局（`~/.octopus/.vault/`）：
//! ```text
//! ~/.octopus/.vault/                  ← git repo
//! ├── .git/                           ← git 元数据
//! ├── meta.json                       ← vault_meta 同步字段
//! ├── outline.json                    ← 增量索引（uuid → sha256）
//! └── ciphers/
//!     ├── a1/                         ← uuid 前 2 hex 分桶（256 桶）
//!     │   ├── <full-uuid1>.json       ← 单 cipher 加密 blob
//!     │   └── <full-uuid2>.json
//!     └── b2/
//! ```
//!
//! 加密层复用现有 user_vault_key + AES-256-GCM（零改动）——文件存储格式与 SQLite
//! 完全一致（v1: 前缀密文）。
//!
//! 详见 spec：`docs/superpowers/specs/2026-07-21-vault-git-sync-design.md`

pub mod error;
pub mod outline;
pub mod store;

pub use error::SyncError;
pub use outline::{Outline, OutlineEntry};
pub use store::{
    cipher_file_path, folder_file_path, meta_path, outline_path, vault_root,
    CipherFile, FolderFile, MetaFile,
};
