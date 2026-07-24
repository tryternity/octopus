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

/// user_vault_key 默认超时阈值（仅供历史 / 测试参考用）。
///
/// 实际运行时超时由 `AppConfig.vault_lock_timeout_secs` 决定，通过
/// [`VaultSession::is_user_vault_unlocked`] 的参数传入。保留本常量是
/// 为了向后兼容历史测试与文档参考——不再用于实际超时判定。
///
/// 仅 user_vault_key 受此约束——app_key 不超时（进程生命周期内常驻）。
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
    /// 超过 `vault_lock_timeout_secs` 未心跳视为「保险库 tab 已离开」，触发超时锁
    /// （0 表示永不锁定）。超时阈值来自 `AppConfig.vault_lock_timeout_secs`。
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
    /// `timeout_secs` 由运行时 config 提供（`AppConfig.vault_lock_timeout_secs`）：
    ///   - `0`  = 永不超时（用户选了 "Never"，UI 应警告）
    ///   - `>0` = 离开焦点后多少秒锁定
    ///
    /// **注意：需要 `&mut self`**——超时分支会主动丢弃 session 持有的 `Arc<DerivedKey>`
    /// 强引用。
    ///
    /// **Zeroizing Drop 触发时机**（订正 #8 误导注释）：
    /// - 若此时无飞行中 Tauri 命令持有 Arc 克隆 → refcount 立即归零 → Drop 立即 zeroize
    /// - 若有命令持有克隆（如 `vault_autotype` 注入按键期间，可能持有数秒）→
    ///   key 在该命令返回栈帧销毁后才被 zeroize
    ///
    /// 即"立即生效"仅在无并发命令时成立。深度防御的完整方案需要取消令牌让飞行中
    /// 命令在 lock 时中止——当前未实现，视为已知窗口（autotype 数秒）。
    ///
    /// 超时基准是 `last_active_at`（前端心跳维护），而非 `unlocked_at`：
    /// 只要保险库 tab 在前台就持续心跳，永不超时；
    /// tab 切走 / 窗口关闭 / 应用失焦满 `timeout_secs` → 心跳停止 → 自动锁定。
    pub fn is_user_vault_unlocked(&mut self, timeout_secs: u64) -> bool {
        if self.user_vault_key.is_none() {
            return false;
        }
        if timeout_secs == 0 {
            // 永不锁定（用户显式选了 "Never"）
            return true;
        }
        // 超时检查：超时则主动清零（不仅返回 false，还 free key）
        // 用 last_active_at 作为「最近活动」基准
        if let Some(t) = self.last_active_at {
            if t.elapsed() > Duration::from_secs(timeout_secs) {
                self.user_vault_key = None;
                self.unlocked_at = None;
                self.last_active_at = None;
                log::info!("vault user_vault_key 失活超 {}s，已主动清零", timeout_secs);
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
    /// 前端卸载（切 tab / 关窗口）后心跳停止，超过 `vault_lock_timeout_secs`
    /// （运行时配置，0=永不）后自动锁定。
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

/// 热键触发时抓到的浏览器 URL 缓存（共享可变状态）。
///
/// **修复 VaultPicker 时序 bug**（2026-07-19 e2e 测试）：
/// 原实现热键 callback `show + set_focus(vault_picker_window)` **之后**才 emit 让前端
/// 调 `vault_detect_and_match`——此时 VaultPicker 已抢前台，`frontmost_bundle_id()` 取到
/// 的是 octopus-desktop 自己，`script_for_browser(octopus_id)` = None → URL 检测失败 →
/// 走 fallback 列出最近 20 条 cipher，用户看到的是全部密码而非当前站点匹配项。
///
/// 修复：热键 callback 在 show VaultPicker **之前**先抓 URL，存入此 Mutex；
/// `vault_detect_and_match` 优先读缓存，没有才走原 `current_browser_url()` 路径（兼容
/// 已 show 后用户手动刷新的场景——此时浏览器仍非前台，会 fallback，符合预期）。
///
/// 用 Mutex 而非 RwLock：写多读少 + 短临界区；URL 字符串 Clone 廉价。
pub type SharedPickerUrlCache = Arc<std::sync::Mutex<Option<String>>>;

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
        Arc::new(DerivedKey::from_raw([byte; 32]))
    }

    #[test]
    fn test_timeout_default_is_5min() {
        assert_eq!(DEFAULT_USER_VAULT_TIMEOUT_SECS, 5 * 60);
    }

    #[test]
    fn test_unlocked_within_timeout() {
        let mut session = VaultSession::default();
        assert!(!session.is_user_vault_unlocked(180));

        session.set_user_vault_unlocked(make_key(1));
        assert!(session.is_user_vault_unlocked(180));
    }

    #[test]
    fn test_timeout_proactively_zeroes_key() {
        let mut session = VaultSession::default();
        session.set_user_vault_unlocked(make_key(1));
        assert!(session.user_vault_key.is_some());

        // 人为把 last_active_at 设到很久以前（>180s = 3min）
        session.last_active_at = Some(Instant::now() - Duration::from_secs(4 * 60));

        // is_user_vault_unlocked 应返回 false 并清零
        assert!(!session.is_user_vault_unlocked(180));
        assert!(session.user_vault_key.is_none(), "key 应被主动清零");
        assert!(session.unlocked_at.is_none());
        assert!(session.last_active_at.is_none());
    }

    #[test]
    fn test_heartbeat_resets_timeout() {
        let mut session = VaultSession::default();
        session.set_user_vault_unlocked(make_key(1));

        // 假装 2 分钟过去（未超时，阈值 180s）
        session.last_active_at = Some(Instant::now() - Duration::from_secs(2 * 60));
        assert!(session.is_user_vault_unlocked(180));

        // 心跳刷新
        session.heartbeat();
        // 再次假装 2 分钟过去（自上次心跳起）—— 仍未超时
        session.last_active_at = Some(Instant::now() - Duration::from_secs(2 * 60));
        assert!(session.is_user_vault_unlocked(180));

        // 假装 4 分钟过去（超时，阈值 180s）
        session.last_active_at = Some(Instant::now() - Duration::from_secs(4 * 60));
        assert!(!session.is_user_vault_unlocked(180));
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
        assert!(!session.is_user_vault_unlocked(180));
    }

    #[test]
    fn test_boundary_exactly_3min_still_unlocked() {
        let mut session = VaultSession::default();
        session.set_user_vault_unlocked(make_key(1));
        // 3:00 整 - 1s 还在有效期内（边界 > 而非 >=，阈值 180s）
        session.last_active_at = Some(Instant::now() - Duration::from_secs(180 - 1));
        assert!(session.is_user_vault_unlocked(180));
    }

    /// `timeout_secs == 0` 表示永不锁定——即使 `last_active_at` 很久以前，
    /// `is_user_vault_unlocked` 也应返回 true 且不清零 key。
    #[test]
    fn test_timeout_zero_means_never_lock() {
        let mut session = VaultSession::default();
        session.set_user_vault_unlocked(make_key(1));
        // 假装很久没心跳
        session.last_active_at = Some(Instant::now() - Duration::from_secs(3600));
        // timeout=0 → 永不锁定
        assert!(session.is_user_vault_unlocked(0));
        assert!(session.user_vault_key.is_some(), "timeout=0 时 key 不应被清");
    }

    /// 显式 timeout 参数控制超时行为——同一份 session 状态在不同 timeout 下结果不同。
    #[test]
    fn test_timeout_param_overrides_default() {
        // 即使 session 按默认 180s 设计，显式 timeout 参数才是真正的判定基准。
        let mut session = VaultSession::default();
        session.set_user_vault_unlocked(make_key(1));
        // 2 分钟前的活动：timeout=60 应锁；timeout=180 不应锁。
        session.last_active_at = Some(Instant::now() - Duration::from_secs(120));
        assert!(!session.is_user_vault_unlocked(60));

        session.set_user_vault_unlocked(make_key(2));
        session.last_active_at = Some(Instant::now() - Duration::from_secs(120));
        assert!(session.is_user_vault_unlocked(180));
    }
}
