//! vault 解锁态管理。
//!
//! AppState：在 Tauri 进程内持有 user_vault_key / app_key。
//! 设计原则：
//!   - app_key 在进程启动时解密并常驻内存（用 K_machine 解 app_key_local_enc）
//!   - user_vault_key 在用户主动解锁后常驻，15 分钟超时清零（仅清 user_vault_key，app_key 不动）
//!   - 所有 key 用 Arc 共享，零拷贝传递；不暴露明文 slice 给外部

use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::RwLock;

use octopus_vault::crypto::DerivedKey;

/// user_vault_key 超时阈值（15 分钟）。
/// 仅 user_vault_key 受此约束——app_key 不超时（进程生命周期内常驻）。
pub const DEFAULT_USER_VAULT_TIMEOUT_SECS: u64 = 15 * 60;

/// Tauri AppState：进程内持有解锁态的 vault keys。
///
/// `user_vault_key` 与 `app_key` 均用 `Arc<DerivedKey>` 共享——Zeroizing Drop
/// 仍生效，外部只能拿到 Arc 句柄，无法直接取出明文 slice。
///
/// 字段在 Task 17+ 才会被真正读写，此处保留完整结构以便 AppState 早期落地。
pub struct VaultSession {
    /// None = 未解锁（用户密码 vault 锁定）
    pub user_vault_key: Option<Arc<DerivedKey>>,
    /// None = 未初始化 / 启动失败（少见）
    pub app_key: Option<Arc<DerivedKey>>,
    pub unlocked_at: Option<Instant>,
}

impl Default for VaultSession {
    fn default() -> Self {
        Self {
            user_vault_key: None,
            app_key: None,
            unlocked_at: None,
        }
    }
}

impl VaultSession {
    /// user_vault_key 是否仍处于解锁有效期内（非空且未超时）。
    pub fn is_user_vault_unlocked(&self) -> bool {
        if self.user_vault_key.is_none() {
            return false;
        }
        // 超时检查
        if let Some(t) = self.unlocked_at {
            if t.elapsed() > Duration::from_secs(DEFAULT_USER_VAULT_TIMEOUT_SECS) {
                return false;
            }
        }
        true
    }

    /// 写入 user_vault_key 并刷新 unlocked_at。Task 17 在用户输主密码成功后调。
    pub fn set_user_vault_unlocked(&mut self, key: Arc<DerivedKey>) {
        self.user_vault_key = Some(key);
        self.unlocked_at = Some(Instant::now());
    }

    /// 锁定 user_vault_key（仅清 user_vault_key，不动 app_key）。
    pub fn lock_user_vault(&mut self) {
        self.user_vault_key = None;
        self.unlocked_at = None;
    }
}

pub type SharedVaultSession = Arc<RwLock<VaultSession>>;

/// 启动时调一次：尝试用 K_machine 解 app_key 注入。
///
/// - 成功 → AppState 的 app_key 字段填好，后续 Task 17+ 直接读
/// - vault 未初始化 / K_machine 缺失 → app_key 留 None，等待用户主动解锁
/// - Keychain 错误 → 仅记日志，不阻塞启动
pub fn bootstrap_app_key(session: &SharedVaultSession) {
    match octopus_vault::unlock::unlock_app_key_local() {
        Ok(Some(app_key)) => {
            log::info!("vault app_key 已通过 K_machine 解锁（无感启动）");
            session.write().app_key = Some(Arc::new(app_key));
        }
        Ok(None) => {
            log::info!("vault 需主密码（K_machine 缺失或 vault 未初始化）");
        }
        Err(e) => {
            log::warn!("vault app_key 解锁失败: {}", e);
        }
    }
}
