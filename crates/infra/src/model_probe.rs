//! 全局「模型加载探针」——依赖反转：asr-local / ocr 在加载点调用 `probe`，
//! 由 desktop 在启动时通过 `set_probe` 注入「读 RSS 差值写入 registry」的闭包。
//! infra 本身不依赖 sysinfo / desktop，只持有闭包。

use parking_lot::Mutex;
use std::sync::Arc;

/// 加载/卸载阶段：模型实例化前（Before）/ 后（After）/ 从内存卸载（Unload）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadPhase {
    Before,
    After,
    Unload,
}

/// 探针闭包：`(阶段, 模型 id)`。id 形如 `"asr:paraformer"` / `"ocr:PP-OCRv4"` / `"vad:silero"`。
pub type ProbeFn = Arc<dyn Fn(LoadPhase, &str) + Send + Sync>;

static PROBE: Mutex<Option<ProbeFn>> = parking_lot::const_mutex(None);

/// desktop 启动时注入探针（覆盖式：重复调用以最后一次为准）。
pub fn set_probe(f: ProbeFn) {
    *PROBE.lock() = Some(f);
}

/// 加载点调用：未注入时为 no-op。
///
/// 先 clone 闭包（Arc 引用计数 +1，廉价）并释放锁，再调用——避免持锁执行用户闭包。
/// 闭包 fallback 路径（非 macOS / phys_footprint 读取失败）会 sysinfo 扫描全部进程，
/// 耗数 ms；持锁调用会阻塞其他线程的 probe。clone 后释放锁是更安全的并发模式。
pub fn probe(phase: LoadPhase, id: &str) {
    let f_opt = PROBE.lock().clone();
    if let Some(f) = f_opt {
        f(phase, id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// 探针注入相关测试共享全局 `PROBE`，必须串行执行，否则并发线程间互相
    /// 调用对方注入的闭包，导致计数 / phase 断言错乱。取该锁即「持有探针」。
    static TEST_SERIALIZER: Mutex<()> = parking_lot::const_mutex(());

    #[test]
    fn probe_is_noop_when_not_set() {
        // 不注入探针，仅验证 no-op；但仍串行以避免与其他测试的全局 PROBE 残留叠加。
        let _guard = TEST_SERIALIZER.lock();
        *PROBE.lock() = None;
        probe(LoadPhase::Before, "asr:x");
        probe(LoadPhase::After, "asr:x");
    }

    #[test]
    fn probe_invokes_injected_closure_with_phase_and_id() {
        let _guard = TEST_SERIALIZER.lock();
        let count = Arc::new(AtomicUsize::new(0));
        let last = Arc::new(Mutex::new((LoadPhase::Before, String::new())));
        let c = count.clone();
        let l = last.clone();
        set_probe(Arc::new(move |phase, id| {
            c.fetch_add(1, Ordering::SeqCst);
            *l.lock() = (phase, id.to_string());
        }));
        probe(LoadPhase::Before, "ocr:PP-OCRv4");
        probe(LoadPhase::After, "vad:silero");
        assert_eq!(count.load(Ordering::SeqCst), 2);
        assert_eq!(last.lock().0, LoadPhase::After);
        assert_eq!(last.lock().1, "vad:silero");
        *PROBE.lock() = None;
    }

    #[test]
    fn probe_unload_variant_reaches_closure() {
        let _guard = TEST_SERIALIZER.lock();
        let count = Arc::new(AtomicUsize::new(0));
        let last = Arc::new(Mutex::new((LoadPhase::Before, String::new())));
        let c = count.clone();
        let l = last.clone();
        set_probe(Arc::new(move |phase, id| {
            c.fetch_add(1, Ordering::SeqCst);
            *l.lock() = (phase, id.to_string());
        }));
        probe(LoadPhase::Unload, "ocr:PP-OCRv4");
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert_eq!(last.lock().0, LoadPhase::Unload);
        assert_eq!(last.lock().1, "ocr:PP-OCRv4");
        *PROBE.lock() = None;
    }
}
