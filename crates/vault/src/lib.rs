//! octopus-vault：密码 vault 核心库。
//!
//! 纯逻辑库，不依赖 tauri / tokio。负责：
//! - 加密（crypto/）
//! - SQLite 存储（storage/）
//! - 密码生成器（generator/）
//! - URL 匹配（matcher/）
//! - 密码健康检查（health/）
//! - Bitwarden 导入（importer/）
//! - TOTP、解锁态管理
//!
//! 依赖方向：infra ← vault ← desktop

pub use zeroize::Zeroizing;

pub mod crypto;
pub mod error;
pub mod storage;
pub mod types;
pub mod unlock;
pub mod keychain;
pub mod generator;
pub mod totp;
pub mod matcher;
pub mod health;
pub mod importer;
