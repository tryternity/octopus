//! 全局「模型加载探针」——依赖反转：asr-local / ocr 在加载点调用 `probe`，
//! 由 desktop 在启动时通过 `set_probe` 注入「读 RSS 差值写入 registry」的闭包。
//! infra 本身不依赖 sysinfo / desktop，只持有闭包。

use parking_lot::Mutex;
use std::sync::Arc;

/// 加载阶段：模型实例化前 / 后。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadPhase {
    Before,
    After,
}

/// 探针闭包：`(阶段, 模型 id)`。id 形如 `"asr:paraformer"` / `"ocr:PP-OCRv4"` / `"vad:silero"`。
pub type ProbeFn = Arc<dyn Fn(LoadPhase, &str) + Send + Sync>;

static PROBE: Mutex<Option<ProbeFn>> = parking_lot::const_mutex(None);

/// desktop 启动时注入探针（覆盖式：重复调用以最后一次为准）。
pub fn set_probe(f: ProbeFn) {
    *PROBE.lock() = Some(f);
}

/// 加载点调用：未注入时为 no-op。
pub fn probe(phase: LoadPhase, id: &str) {
    if let Some(f) = PROBE.lock().as_ref() {
        f(phase, id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn probe_is_noop_when_not_set() {
        probe(LoadPhase::Before, "asr:x");
        probe(LoadPhase::After, "asr:x");
    }

    #[test]
    fn probe_invokes_injected_closure_with_phase_and_id() {
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
}
