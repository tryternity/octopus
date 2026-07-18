//! vault_meta 写锁（修复 #4：非原子 read-modify-write 防并发损坏）。
//!
//! `change_master_password` 与 `refresh_app_key_local_enc` 都是
//! read_vault_meta → 局部改 → save_vault_meta 整行覆盖，两次独立 `with_db` 调用
//! 之间无锁。Tauri 同步命令被 wrap 成 spawn_blocking，可在不同 worker 并发执行。
//!
//! 最坏交错（双 modal 同时操作）：
//! 1. T1 read → 拿旧密文 S1
//! 2. T2 read → 也拿 S1
//! 3. T1 save → 写新密文
//! 4. T2 save → 用 S1 + T2 自己改的字段整行覆盖（T1 改的字段丢失）
//! → 永久数据损坏（如新主密码失效但 app_key_local_enc 用旧主密码派生的 K_machine 解）
//!
//! 本模块提供进程内 Mutex，串行化所有 meta 写操作。Mutex 是 MutexGuard RAII，
//! 调用方 `let _guard = acquire_meta_write_lock()?;` 持有期间其他写操作阻塞。
//!
//! 注意：仅防进程内并发（Tauri 命令并发）。跨进程并发（多实例 octopus-desktop）
//! 由 SQLite 自身 WAL + 单文件存储 + single-instance plugin 兜底。

use parking_lot::Mutex;
use std::sync::OnceLock;

/// 全局 meta 写锁单例。首次访问时惰性初始化。
static META_WRITE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn lock() -> &'static Mutex<()> {
    META_WRITE_LOCK.get_or_init(|| Mutex::new(()))
}

/// 获取 meta 写锁的 RAII guard。持有期间其他 meta 写操作（change_master_password /
/// refresh_app_key_local_enc）阻塞，保证 read-modify-write 整段原子。
///
/// 用法：`let _guard = acquire_meta_write_lock();` —— guard 在作用域结束时自动释放。
///
/// 注意：本锁是 best-effort 防并发——只有所有 meta 写路径都显式调用本函数才生效。
/// 当前调用点：unlock.rs 的 change_master_password + refresh_app_key_local_enc。
pub fn acquire_meta_write_lock() -> parking_lot::MutexGuard<'static, ()> {
    lock().lock()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    /// 多线程并发 acquire——应该是串行的（每次只能一个线程持有）。
    #[test]
    fn test_lock_serializes_concurrent_writers() {
        let counter = Arc::new(Mutex::new(0u32));
        let max_concurrent = Arc::new(Mutex::new(0u32));

        let handles: Vec<_> = (0..4)
            .map(|_| {
                let counter = counter.clone();
                let max_concurrent = max_concurrent.clone();
                thread::spawn(move || {
                    let _guard = acquire_meta_write_lock();
                    // 模拟 read-modify-write 临界区
                    {
                        let mut c = counter.lock();
                        *c += 1;
                        let cur = *c;
                        let mut m = max_concurrent.lock();
                        if cur > *m {
                            *m = cur;
                        }
                    }
                    // 持有锁一会，让其他线程有机会尝试
                    thread::sleep(std::time::Duration::from_millis(10));
                    {
                        let mut c = counter.lock();
                        *c -= 1;
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        // 任意时刻最多 1 个线程在临界区
        assert_eq!(*max_concurrent.lock(), 1, "锁未串行化并发写者");
    }
}
