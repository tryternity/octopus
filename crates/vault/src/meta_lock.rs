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
//! **复审 #2 修复**（2026-07-19）：锁下沉到 `save_vault_meta` / `update_security_stamp`
//! 写函数内部（meta.rs），覆盖**所有** meta 写路径（不仅是 change/refresh）。
//! 改用 `ReentrantMutex` 让外层 `change_master_password` 已持锁时，内层
//! `save_vault_meta` 再次 lock 不会死锁（同线程可重入）。
//!
//! 注意：仅防进程内并发（Tauri 命令并发）。跨进程并发（多实例 octopus-desktop）
//! 由 SQLite 自身 WAL + 单文件存储 + single-instance plugin 兜底。

use parking_lot::ReentrantMutex;
use std::sync::OnceLock;

/// 全局 meta 写锁单例。首次访问时惰性初始化。
///
/// 用 `ReentrantMutex`（同线程可重入）——外层 `change_master_password` 持锁后
/// 内层调 `save_vault_meta` 再次 lock 不死锁。这样锁可以下沉到写函数内部，
/// 覆盖所有 meta 写路径（含 regenerate_security_stamp / setup_vault），
/// 而非依赖每个调用方显式 acquire。
static META_WRITE_LOCK: OnceLock<ReentrantMutex<()>> = OnceLock::new();

fn lock() -> &'static ReentrantMutex<()> {
    META_WRITE_LOCK.get_or_init(|| ReentrantMutex::new(()))
}

/// 获取 meta 写锁的 RAII guard。持有期间其他 meta 写操作（change_master_password /
/// refresh_app_key_local_enc / regenerate_security_stamp / setup_vault /
/// save_vault_meta / update_security_stamp 内部）阻塞，保证 read-modify-write 整段原子。
///
/// **ReentrantMutex 语义**：同线程已持锁时再次 acquire 不阻塞（parking_lot
/// ReentrantMutex 计数 +1，guard 全部 drop 后才释放）——这是下沉到写函数内部
/// 的前提（外层 change_master_password 持锁 → 内层 save_vault_meta 再 lock）。
///
/// 调用方一般不需要显式调本函数——`save_vault_meta` / `update_security_stamp`
/// 内部已自动加锁。**仅当调用方做 read-modify-write 整段事务**时才需显式持锁
/// （保证读到的数据在写之前不被其他写者改动）：
/// ```ignore
/// let _guard = acquire_meta_write_lock();  // 整段 RMW 持锁
/// let meta = read_vault_meta()?;
/// let new_input = modify(meta);
/// save_vault_meta(&new_input)?;  // 内部再 lock（同线程重入 OK）
/// ```
pub fn acquire_meta_write_lock() -> parking_lot::ReentrantMutexGuard<'static, ()> {
    lock().lock()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    /// 多线程并发 acquire——应该是串行的（每次只能一个线程持有）。
    /// ReentrantMutex 跨线程行为同 Mutex（同线程才可重入）。
    #[test]
    fn test_lock_serializes_concurrent_writers() {
        let counter = Arc::new(parking_lot::Mutex::new(0u32));
        let max_concurrent = Arc::new(parking_lot::Mutex::new(0u32));

        let handles: Vec<_> = (0..4)
            .map(|_| {
                let counter = counter.clone();
                let max_concurrent = max_concurrent.clone();
                thread::spawn(move || {
                    let _guard = acquire_meta_write_lock();
                    {
                        let mut c = counter.lock();
                        *c += 1;
                        let cur = *c;
                        let mut m = max_concurrent.lock();
                        if cur > *m {
                            *m = cur;
                        }
                    }
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

        assert_eq!(*max_concurrent.lock(), 1, "锁未串行化并发写者");
    }

    /// 同线程可重入：外层持锁后内层再 acquire 不死锁。
    /// 这是下沉到写函数内部的前提。
    #[test]
    fn test_lock_is_reentrant_same_thread() {
        let _outer = acquire_meta_write_lock();
        // 内层再 lock——若非 reentrant 会死锁，测试会 hang 超时
        let _inner = acquire_meta_write_lock();
        // 走到这里说明 reentrant 正常
    }
}
