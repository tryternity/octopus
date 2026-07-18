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

/// 进程级全局 session 句柄（clone 自 manage() 注入 Tauri State 的那份 Arc）。
///
/// 用于 cloud API 推理热路径（AliyunEngine::transcribe / config::llm_config_ignore_mode /
/// action_bar 云端翻译）——这些位置拿不到 Tauri `State<SharedVaultSession>`，但需要
/// 解密 `v1:` 前缀的 secret_key（follow-up #7）。
///
/// 启动时由 [`set_global_session`] 注入；未注入或 vault 未初始化时 [`try_global_session`]
/// 返回 None，调用方按「vault 未启用」语义走原 raw 路径（明文 / 本地 manifest）。
static GLOBAL_SESSION: std::sync::OnceLock<SharedVaultSession> = std::sync::OnceLock::new();

/// 注入全局 session 句柄。main.rs 在 `app.manage(vault_session)` 之后调用一次。
///
/// 用 OnceLock：进程级单例，重复调用（如测试）幂等——后一次调用被忽略。
pub fn set_global_session(session: SharedVaultSession) {
    let _ = GLOBAL_SESSION.set(session);
}

/// 取全局 session 句柄（clone Arc，零拷贝）。
/// 未注入返回 None——调用方按 vault 未启用处理（返回 raw secret_key）。
pub fn try_global_session() -> Option<SharedVaultSession> {
    GLOBAL_SESSION.get().cloned()
}

/// user_vault_key 超时阈值（15 分钟）。
/// 仅 user_vault_key 受此约束——app_key 不超时（进程生命周期内常驻）。
///
/// 5 分钟：用户离开保险库页面 / 关闭设置窗口后，5 分钟内回来不需重新输主密码；
/// 超过 5 分钟视为已离开太久，防偷窥。配合 VaultPanel unmount 时的主动 lock，
/// 实现「关闭设置窗口立即锁 / 离开 5 分钟超时锁」的双层防护。
pub const DEFAULT_USER_VAULT_TIMEOUT_SECS: u64 = 5 * 60;

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
    /// 解锁时刻（首次输主密码时设）
    pub unlocked_at: Option<Instant>,
    /// 最后一次心跳时刻——前端保险库 tab 处于前台时每 30s 调一次 vault_heartbeat。
    /// 超过 5min 未心跳视为「保险库 tab 已离开」，触发超时锁。
    pub last_active_at: Option<Instant>,
}

impl Default for VaultSession {
    fn default() -> Self {
        Self {
            user_vault_key: None,
            app_key: None,
            unlocked_at: None,
            last_active_at: None,
        }
    }
}

impl VaultSession {
    /// user_vault_key 是否仍处于解锁有效期内（非空且 last_active_at 未超时）。
    ///
    /// **注意：需要 `&mut self`**——超时分支会主动清零 user_vault_key
    /// （Zeroizing Drop 立即生效），避免过期 key 在内存残留到下次访问。
    ///
    /// 超时基准是 `last_active_at`（前端心跳维护），而非 `unlocked_at`：
    /// 只要保险库 tab 在前台就持续心跳，永不超时；
    /// tab 切走 / 窗口关闭 / 应用失焦满 5 分钟 → 心跳停止 → 自动锁定。
    pub fn is_user_vault_unlocked(&mut self) -> bool {
        if self.user_vault_key.is_none() {
            return false;
        }
        // 超时检查：超时则主动清零（不仅返回 false，还 free key）
        // 用 last_active_at 作为「最近活动」基准
        if let Some(t) = self.last_active_at {
            if t.elapsed() > Duration::from_secs(DEFAULT_USER_VAULT_TIMEOUT_SECS) {
                self.user_vault_key = None;
                self.unlocked_at = None;
                self.last_active_at = None;
                log::info!(
                    "vault user_vault_key 失活超 {}s，已主动清零",
                    DEFAULT_USER_VAULT_TIMEOUT_SECS
                );
                return false;
            }
        }
        true
    }

    /// 写入 user_vault_key 并初始化 unlocked_at + last_active_at。
    /// Task 17 在用户输主密码成功后调。
    pub fn set_user_vault_unlocked(&mut self, key: Arc<DerivedKey>) {
        let now = Instant::now();
        self.user_vault_key = Some(key);
        self.unlocked_at = Some(now);
        self.last_active_at = Some(now);
    }

    /// 前端保险库 tab 处于前台时每 30s 调用一次，刷新 last_active_at。
    /// 前端卸载（切 tab / 关窗口）后心跳停止，5 分钟后自动锁定。
    pub fn heartbeat(&mut self) {
        if self.user_vault_key.is_some() {
            self.last_active_at = Some(Instant::now());
        }
    }

    /// 锁定 user_vault_key（仅清 user_vault_key，不动 app_key）。
    pub fn lock_user_vault(&mut self) {
        self.user_vault_key = None;
        self.unlocked_at = None;
        self.last_active_at = None;
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

#[cfg(test)]
mod tests {
    use super::*;
    use octopus_vault::crypto::DerivedKey;
    use octopus_vault::Zeroizing;

    fn make_key(byte: u8) -> Arc<DerivedKey> {
        Arc::new(DerivedKey(Zeroizing::new([byte; 32])))
    }

    #[test]
    fn test_timeout_default_is_5min() {
        assert_eq!(DEFAULT_USER_VAULT_TIMEOUT_SECS, 5 * 60);
    }

    #[test]
    fn test_unlocked_within_timeout() {
        let mut session = VaultSession::default();
        assert!(!session.is_user_vault_unlocked());

        session.set_user_vault_unlocked(make_key(1));
        assert!(session.is_user_vault_unlocked());
    }

    #[test]
    fn test_timeout_proactively_zeroes_key() {
        let mut session = VaultSession::default();
        session.set_user_vault_unlocked(make_key(1));
        assert!(session.user_vault_key.is_some());

        // 人为把 last_active_at 设到很久以前（>5min）
        session.last_active_at = Some(Instant::now() - Duration::from_secs(6 * 60));

        // is_user_vault_unlocked 应返回 false 并清零
        assert!(!session.is_user_vault_unlocked());
        assert!(session.user_vault_key.is_none(), "key 应被主动清零");
        assert!(session.unlocked_at.is_none());
        assert!(session.last_active_at.is_none());
    }

    #[test]
    fn test_heartbeat_resets_timeout() {
        let mut session = VaultSession::default();
        session.set_user_vault_unlocked(make_key(1));

        // 假装 4 分钟过去（未超时）
        session.last_active_at = Some(Instant::now() - Duration::from_secs(4 * 60));
        assert!(session.is_user_vault_unlocked());

        // 心跳刷新
        session.heartbeat();
        // 再次假装 4 分钟过去（自上次心跳起）—— 仍未超时
        session.last_active_at = Some(Instant::now() - Duration::from_secs(4 * 60));
        assert!(session.is_user_vault_unlocked());

        // 假装 6 分钟过去（超时）
        session.last_active_at = Some(Instant::now() - Duration::from_secs(6 * 60));
        assert!(!session.is_user_vault_unlocked());
    }

    #[test]
    fn test_heartbeat_ignored_when_locked() {
        // 未解锁时调 heartbeat 不应崩溃也不应设 last_active_at
        let mut session = VaultSession::default();
        session.heartbeat();
        assert!(session.last_active_at.is_none());
    }

    #[test]
    fn test_lock_user_vault_clears_key() {
        let mut session = VaultSession::default();
        session.set_user_vault_unlocked(make_key(1));
        session.lock_user_vault();
        assert!(session.user_vault_key.is_none());
        assert!(session.unlocked_at.is_none());
        assert!(session.last_active_at.is_none());
        assert!(!session.is_user_vault_unlocked());
    }

    #[test]
    fn test_boundary_exactly_5min_still_unlocked() {
        let mut session = VaultSession::default();
        session.set_user_vault_unlocked(make_key(1));
        // 5:00 整 - 1s 还在有效期内（边界 > 而非 >=）
        session.last_active_at = Some(Instant::now() - Duration::from_secs(5 * 60 - 1));
        assert!(session.is_user_vault_unlocked());
    }
}
