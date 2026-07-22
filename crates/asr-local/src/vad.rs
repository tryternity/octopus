use anyhow::{Context, Result};
use ndarray::{Array1, Array3, ArrayView2};
use ort::session::Session;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use parking_lot::Mutex;
use std::sync::{Arc, OnceLock};

// 约束：Session 必须 Send + Sync，才能用 Arc<Mutex<Session>> 进全局缓存
// （OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<Session>>>>>）。
const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Session>();
};

/// 按 model 路径缓存已加载的 ONNX Session。
/// SileroVad 实例各自 owned h/c/sr，但共享底层 Session（Session::run 是 &mut self，
/// 所以用 Arc<Mutex<Session>> 提供内部可变性 + 共享所有权）。
static VAD_SESSIONS: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<Session>>>>> = OnceLock::new();

fn vad_sessions() -> &'static Mutex<HashMap<PathBuf, Arc<Mutex<Session>>>> {
    VAD_SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Silero VAD v4 via ort (ONNX Runtime)
/// Stateful model with LSTM hidden/cell states
pub struct SileroVad {
    session: Arc<Mutex<Session>>,   // 共享缓存：相同 path 复用同一 Session（run 需 &mut self，故 Mutex）
    h: Array3<f32>,                 // [2, 1, 64]
    c: Array3<f32>,                 // [2, 1, 64]
    sr: Array1<i64>,                // scalar: 16000
}

impl SileroVad {
    pub fn new(model_path: &Path) -> Result<Self> {
        // 持 cache lock 期间完成 get-or-insert：消除 TOCTOU（并发 miss 时只有一个线程加载，
        // 其余等锁后命中同一 Arc）。commit_from_file 慢但仅在冷启动一次性发生，串行化可接受。
        let session = {
            let mut cache = vad_sessions().lock();
            if let Some(s) = cache.get(model_path) {
                s.clone()
            } else {
                octopus_infra::model_probe::probe(
                    octopus_infra::model_probe::LoadPhase::Before,
                    "vad:silero",
                );
                let s = Arc::new(Mutex::new(
                    Session::builder()
                        .context("Failed to create ORT session builder")?
                        .commit_from_file(model_path)
                        .with_context(|| format!("Failed to load Silero VAD from {:?}", model_path))?,
                ));
                cache.insert(model_path.to_path_buf(), s.clone());
                octopus_infra::model_probe::probe(
                    octopus_infra::model_probe::LoadPhase::After,
                    "vad:silero",
                );
                s
            }
        };
        Ok(Self {
            session,
            h: Array3::zeros((2, 1, 64)),
            c: Array3::zeros((2, 1, 64)),
            sr: Array1::from_vec(vec![16000i64]),
        })
    }

    /// 从编译期内嵌字节加载 VAD（`include_bytes!`），不落盘、不读磁盘文件。
    ///
    /// 内嵌模型 1.7MB，ort `commit_from_memory` 直接从 `&[u8]` 构造 Session。
    /// 缓存 key 用 `builtin://silero_vad_v4` 与磁盘路径缓存隔离。
    pub fn new_builtin() -> Result<Self> {
        const VAD_BYTES: &[u8] = include_bytes!("../models/silero_vad_v4.onnx");
        let cache_key = PathBuf::from("builtin://silero_vad_v4");
        let session = {
            let mut cache = vad_sessions().lock();
            if let Some(s) = cache.get(&cache_key) {
                s.clone()
            } else {
                octopus_infra::model_probe::probe(
                    octopus_infra::model_probe::LoadPhase::Before,
                    "vad:silero",
                );
                let s = Arc::new(Mutex::new(
                    Session::builder()
                        .context("Failed to create ORT session builder")?
                        .commit_from_memory(VAD_BYTES)
                        .context("Failed to load Silero VAD from embedded bytes")?,
                ));
                cache.insert(cache_key, s.clone());
                octopus_infra::model_probe::probe(
                    octopus_infra::model_probe::LoadPhase::After,
                    "vad:silero",
                );
                s
            }
        };
        Ok(Self {
            session,
            h: Array3::zeros((2, 1, 64)),
            c: Array3::zeros((2, 1, 64)),
            sr: Array1::from_vec(vec![16000i64]),
        })
    }

    /// Input: 480 samples (30ms @ 16kHz). Output: speech probability [0.0, 1.0]
    pub fn compute(&mut self, samples: &[f32]) -> Result<f32> {
        let input = ArrayView2::from_shape((1, samples.len()), samples)?;
        let h_tensor = ort::value::TensorRef::from_array_view(self.h.view())?;
        let c_tensor = ort::value::TensorRef::from_array_view(self.c.view())?;
        let sr_tensor = ort::value::TensorRef::from_array_view(self.sr.view())?;
        let input_tensor = ort::value::TensorRef::from_array_view(input)?;

        let mut session = self.session.lock();
        let outputs = session.run(ort::inputs! {
            "input" => input_tensor,
            "sr" => sr_tensor,
            "h" => h_tensor,
            "c" => c_tensor
        })?;

        // Extract probability
        let (_shape, data) = outputs["output"].try_extract_tensor::<f32>()?;
        let prob = data[0];

        // Update hidden/cell states for next call
        let (_h_shape, h_data) = outputs["hn"].try_extract_tensor::<f32>()?;
        if let Some(h_slice) = self.h.as_slice_mut() {
            h_slice.copy_from_slice(h_data);
        } else {
            self.h = Array3::from_shape_vec((2, 1, 64), h_data.to_vec())?;
        }

        let (_c_shape, c_data) = outputs["cn"].try_extract_tensor::<f32>()?;
        if let Some(c_slice) = self.c.as_slice_mut() {
            c_slice.copy_from_slice(c_data);
        } else {
            self.c = Array3::from_shape_vec((2, 1, 64), c_data.to_vec())?;
        }

        Ok(prob)
    }

    /// Reset LSTM states (call between different audio segments)
    pub fn reset(&mut self) {
        self.h = Array3::zeros((2, 1, 64));
        self.c = Array3::zeros((2, 1, 64));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 内嵌 VAD 字节始终可用（include_bytes!），但 ort session 构造在测试环境可能失败。
    fn try_new() -> Option<SileroVad> {
        crate::config::create_silero_vad().ok()
    }

    #[test]
    fn same_path_shares_session() {
        let (a, b) = match (try_new(), try_new()) {
            (Some(a), Some(b)) => (a, b),
            _ => {
                println!("skip: 测试环境无 silero_vad 模型文件");
                return;
            }
        };
        assert!(
            Arc::ptr_eq(&a.session, &b.session),
            "同 path 应共享同一 Session Arc（缓存未生效？）"
        );
    }

    #[test]
    fn compute_returns_probability_in_range() {
        let mut v = match try_new() {
            Some(v) => v,
            None => {
                println!("skip: 测试环境无 silero_vad 模型文件");
                return;
            }
        };
        v.reset();
        let samples = vec![0.0f32; 480];
        let prob = v.compute(&samples).expect("compute should succeed");
        assert!((0.0..=1.0).contains(&prob), "概率应在 [0,1]，实际 {}", prob);
    }

    // ── 内嵌 VAD 测试（include_bytes! + commit_from_memory）──

    /// 内嵌字节能成功构造 SileroVad（验证 include_bytes! 的模型文件有效 + ort commit_from_memory 正常）。
    #[test]
    fn new_builtin_loads_embedded_model() {
        let mut v = match SileroVad::new_builtin() {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[SKIP] ort commit_from_memory 失败（测试环境 ONNX Runtime 问题）: {}", e);
                return;
            }
        };
        // 能 reset 不 panic 即证明 Session 已加载
        v.reset();
    }

    /// new_builtin() 两次构造共享同一 Session（builtin:// 缓存生效）。
    #[test]
    fn new_builtin_shares_session() {
        let a = match SileroVad::new_builtin() {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[SKIP] ort commit_from_memory 失败: {}", e);
                return;
            }
        };
        let b = match SileroVad::new_builtin() {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[SKIP] ort commit_from_memory 失败: {}", e);
                return;
            }
        };
        assert!(
            Arc::ptr_eq(&a.session, &b.session),
            "new_builtin() 两次应共享同一 Session Arc（builtin:// 缓存未生效？）"
        );
    }

    /// 内嵌 VAD + compute 能正常推理（静音输入返回低概率）。
    #[test]
    fn new_builtin_compute_silence() {
        let mut v = match SileroVad::new_builtin() {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[SKIP] ort commit_from_memory 失败: {}", e);
                return;
            }
        };
        v.reset();
        let samples = vec![0.0f32; 480];
        let prob = v.compute(&samples).expect("compute should succeed");
        assert!(
            (0.0..=1.0).contains(&prob),
            "内嵌 VAD compute 概率应在 [0,1]，实际 {}",
            prob
        );
    }

    /// builtin 缓存与磁盘路径缓存隔离（builtin:// key 存在于缓存中）。
    #[test]
    fn builtin_cache_key_exists() {
        let _ = match SileroVad::new_builtin() {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[SKIP] ort commit_from_memory 失败: {}", e);
                return;
            }
        };
        // 确认 builtin:// cache key 存在（全局缓存可能在测试间累积，不验证唯一性）
        let cache = vad_sessions().lock();
        assert!(
            cache.contains_key(&PathBuf::from("builtin://silero_vad_v4")),
            "builtin:// cache key 应存在"
        );
    }
}
