//! 主密码暴力破解防护（spec §7.3，复审 #3 修复）。
//!
//! 进程内 `UnlockAttemptGuard`——失败计数 + 指数退避（0/1/2/4/8/16/30s）。
//! 防在线脚本循环 `invoke('vault_unlock')` 无限制尝试。
//!
//! **进程内**：重启 app 重置计数（不做持久化锁定——单机桌面工具，持久化锁定
//! 反而被攻击者利用做 DoS）。Argon2id (t=3, m=64MiB) 本身 ~2-3 次/秒限速，
//! 本 guard 在此基础上叠加指数退避。
//!
//! 接入点（3 个解锁路径）：
//! - `unlock::unlock_with_master_password`（流程 C/D）
//! - `unlock::verify_master_password`（reprompt 二次验证）
//! - `unlock::change_master_password` 旧密码校验（flow E）

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 退避序列（秒）：第 1 次失败 0s，第 2 次 1s，第 3 次 2s，... 第 6+ 次 30s 封顶。
const BACKOFF_SECS: &[u64] = &[0, 1, 2, 4, 8, 16, 30];

static GUARD: OnceLock<UnlockAttemptGuard> = OnceLock::new();

/// 进程级 guard 单例（首次访问惰性初始化）。
pub fn guard() -> &'static UnlockAttemptGuard {
    GUARD.get_or_init(UnlockAttemptGuard::new)
}

/// 暴力破解防护 guard。
///
/// 用 `AtomicU32` / `AtomicU64` 无锁——三个解锁路径（可能并发）都安全。
/// 成功解锁调 `reset()`，失败调 `record_failure()`。
pub struct UnlockAttemptGuard {
    failures: AtomicU32,
    next_allowed_at: AtomicU64,
}

impl Default for UnlockAttemptGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl UnlockAttemptGuard {
    pub const fn new() -> Self {
        Self {
            failures: AtomicU32::new(0),
            next_allowed_at: AtomicU64::new(0),
        }
    }

    /// 记录一次失败，返回当前应等待的秒数（0 = 立即可重试）。
    pub fn record_failure(&self) -> Duration {
        let n = self.failures.fetch_add(1, Ordering::SeqCst) + 1;
        let delay_secs = BACKOFF_SECS
            .get((n as usize).saturating_sub(1))
            .copied()
            .unwrap_or(30);
        let now = now_unix();
        self.next_allowed_at
            .store(now + delay_secs, Ordering::SeqCst);
        Duration::from_secs(delay_secs)
    }

    /// 返回当前剩余等待时间——`Some(d)` 表示需等 d 后才能尝试，`None` 表示可立即尝试。
    pub fn remaining_wait(&self) -> Option<Duration> {
        let next = self.next_allowed_at.load(Ordering::SeqCst);
        if next == 0 {
            return None;
        }
        let now = now_unix();
        if now >= next {
            return None;
        }
        Some(Duration::from_secs(next - now))
    }

    /// 成功解锁——重置 failures 和 next_allowed_at，让下次失败从 0 退避开始。
    pub fn reset(&self) {
        self.failures.store(0, Ordering::SeqCst);
        self.next_allowed_at.store(0, Ordering::SeqCst);
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_backoff_sequence() {
        let g = UnlockAttemptGuard::new();
        assert_eq!(g.record_failure(), Duration::from_secs(0)); // 第 1 次
        assert_eq!(g.record_failure(), Duration::from_secs(1)); // 第 2 次
        assert_eq!(g.record_failure(), Duration::from_secs(2));
        assert_eq!(g.record_failure(), Duration::from_secs(4));
        assert_eq!(g.record_failure(), Duration::from_secs(8));
        assert_eq!(g.record_failure(), Duration::from_secs(16));
        assert_eq!(g.record_failure(), Duration::from_secs(30)); // 第 7 次封顶
        assert_eq!(g.record_failure(), Duration::from_secs(30)); // 第 8 次仍 30
    }

    #[test]
    fn test_reset_clears_count() {
        let g = UnlockAttemptGuard::new();
        g.record_failure();
        g.record_failure();
        g.record_failure();
        g.reset();
        // reset 后第 1 次失败应又是 0s（第 1 次延迟）
        assert_eq!(g.record_failure(), Duration::from_secs(0));
    }

    #[test]
    fn test_remaining_wait_after_failure() {
        let g = UnlockAttemptGuard::new();
        // 初始无等待
        assert!(g.remaining_wait().is_none());
        // 第 1 次失败 delay=0 → 仍无等待
        g.record_failure();
        assert!(g.remaining_wait().is_none());
        // 第 2 次失败 delay=1 → 有等待（最多 1s）
        g.record_failure();
        let wait = g.remaining_wait();
        assert!(wait.is_some(), "第 2 次失败后应有等待");
        let w = wait.unwrap();
        assert!(w.as_secs() <= 1, "等待应 ≤1s，实际 {:?}", w);
    }
}
