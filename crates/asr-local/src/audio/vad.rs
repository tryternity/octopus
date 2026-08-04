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
/// SileroVad 实例各自 owned state + context，但共享底层 Session
/// （Session::run 是 &mut self，用 Arc<Mutex<Session>> 提供内部可变性 + 共享所有权）。
static VAD_SESSIONS: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<Session>>>>> = OnceLock::new();

fn vad_sessions() -> &'static Mutex<HashMap<PathBuf, Arc<Mutex<Session>>>> {
    VAD_SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// silero_vad_16k_op15.onnx——官方 16kHz 专用精简版（1.2MB，opset 15）。
/// 去掉 8kHz 分支（If 节点），比完整版 silero_vad.onnx（2.3MB）小 46%。
const VAD_BYTES: &[u8] = octopus_infra::resources::silero_vad_v6_onnx();
const VAD_CACHE_KEY: &str = "builtin://silero_vad_v6";

/// v6 LSTM 状态维度（config.json state_dim=128）。
const STATE_DIM: usize = 128;
/// v6 输入需拼 context（上一帧末尾样本）——16kHz = 64 样本。
/// 实际输入 shape = [1, samples + CONTEXT_SIZE]。漏拼 context 导致 prob 恒近零。
/// 参考官方 silero-vad/examples/rust-example/src/silero.rs。
const CONTEXT_SIZE: usize = 64;

/// Silero VAD v6 via ort (ONNX Runtime)
///
/// 参考官方 Rust 实现 silero-vad/examples/rust-example/src/silero.rs。
///
/// v6 关键差异（vs v4）：
/// - state 单 tensor `[2,1,128]`（v4 是 h/c 双 `[2,1,64]`）
/// - 输入拼 context（上一帧末尾 64 样本）→ `[1, 576]` = context(64) + samples(512)
/// - 窗口固定 512（16kHz）；漏拼 context 导致 prob 恒近零
pub struct SileroVad {
    session: Arc<Mutex<Session>>,
    state: Array3<f32>,     // [2, 1, 128]
    context: Array1<f32>,   // [CONTEXT_SIZE]——上一帧末尾样本，拼到当前帧前面
    sr: Array1<i64>,        // scalar: 16000
}

impl SileroVad {
    pub fn new(model_path: &Path) -> Result<Self> {
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
        Ok(Self::with_session(session))
    }

    /// 从编译期内嵌字节加载 VAD（`include_bytes!`），不落盘、不读磁盘文件。
    pub fn new_builtin() -> Result<Self> {
        let cache_key = PathBuf::from(VAD_CACHE_KEY);
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
        Ok(Self::with_session(session))
    }

    fn with_session(session: Arc<Mutex<Session>>) -> Self {
        Self {
            session,
            state: Array3::zeros((2, 1, STATE_DIM)),
            context: Array1::zeros(CONTEXT_SIZE),
            sr: Array1::from_vec(vec![16000i64]),
        }
    }

    /// Input: PCM samples (512 @ 16kHz). Output: speech probability [0.0, 1.0]
    ///
    /// 参考官方 silero-vad/examples/rust-example/src/silero.rs::calc_level——
    /// 输入拼 context（上一帧末尾 CONTEXT_SIZE 样本）→ [1, 576]，推理后更新 state + context。
    pub fn compute(&mut self, samples: &[f32]) -> Result<f32> {
        // 拼接 context + samples → [1, CONTEXT_SIZE + samples.len()]
        let mut buf = Vec::with_capacity(CONTEXT_SIZE + samples.len());
        buf.extend_from_slice(self.context.as_slice().unwrap_or(&[]));
        buf.extend_from_slice(samples);
        let input = ArrayView2::from_shape((1, buf.len()), &buf)?;

        let state_tensor = ort::value::TensorRef::from_array_view(self.state.view())?;
        let sr_tensor = ort::value::TensorRef::from_array_view(self.sr.view())?;
        let input_tensor = ort::value::TensorRef::from_array_view(input)?;

        let mut session = self.session.lock();
        let outputs = session.run(ort::inputs! {
            "input" => input_tensor,
            "sr" => sr_tensor,
            "state" => state_tensor
        })?;

        // Extract probability
        let (_shape, data) = outputs["output"].try_extract_tensor::<f32>()?;
        let prob = data[0];

        // Update state for next call
        let (_s_shape, s_data) = outputs["stateN"].try_extract_tensor::<f32>()?;
        if let Some(s_slice) = self.state.as_slice_mut() {
            s_slice.copy_from_slice(s_data);
        } else {
            self.state = Array3::from_shape_vec((2, 1, STATE_DIM), s_data.to_vec())?;
        }

        // Update context：取本帧输入末尾 CONTEXT_SIZE 样本（官方对齐）
        if samples.len() >= CONTEXT_SIZE {
            self.context = Array1::from_vec(samples[samples.len() - CONTEXT_SIZE..].to_vec());
        }

        Ok(prob)
    }

    /// Reset LSTM state + context (call between different audio segments)
    pub fn reset(&mut self) {
        self.state = Array3::zeros((2, 1, STATE_DIM));
        self.context = Array1::zeros(CONTEXT_SIZE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let samples = vec![0.0f32; 512];
        let prob = v.compute(&samples).expect("compute should succeed");
        assert!((0.0..=1.0).contains(&prob), "概率应在 [0,1]，实际 {}", prob);
    }

    // ── 内嵌 VAD 测试（include_bytes! + commit_from_memory）──

    #[test]
    fn new_builtin_loads_embedded_model() {
        let mut v = match SileroVad::new_builtin() {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[SKIP] ort commit_from_memory 失败: {}", e);
                return;
            }
        };
        v.reset();
    }

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
            "new_builtin() 两次应共享同一 Session Arc"
        );
    }

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
        let samples = vec![0.0f32; 512];
        let prob = v.compute(&samples).expect("compute should succeed");
        assert!(
            (0.0..=1.0).contains(&prob),
            "内嵌 VAD compute 概率应在 [0,1]，实际 {}",
            prob
        );
    }

    #[test]
    fn builtin_cache_key_exists() {
        let _ = match SileroVad::new_builtin() {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[SKIP] ort commit_from_memory 失败: {}", e);
                return;
            }
        };
        let cache = vad_sessions().lock();
        assert!(
            cache.contains_key(&PathBuf::from(VAD_CACHE_KEY)),
            "builtin:// cache key 应存在：{}",
            VAD_CACHE_KEY
        );
    }

    /// v6 context 拼接核心回归：模拟语音 prob 应显著高于静音。
    /// 防止 context 拼接缺失导致 prob 恒近零。
    #[test]
    fn v6_speech_prob_higher_than_silence() {
        let mut v = match SileroVad::new_builtin() {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[SKIP] ort commit_from_memory 失败: {}", e);
                return;
            }
        };

        let voice: Vec<f32> = (0..512)
            .map(|i| {
                let t = i as f32 / 16000.0;
                let fundamental = (std::f32::consts::TAU * 120.0 * t).sin() * 0.5;
                let harmonic2 = (std::f32::consts::TAU * 240.0 * t).sin() * 0.25;
                let harmonic4 = (std::f32::consts::TAU * 480.0 * t).sin() * 0.1;
                fundamental + harmonic2 + harmonic4
            })
            .collect();

        v.reset();
        let mut voice_prob = 0.0;
        for _ in 0..3 {
            voice_prob = v.compute(&voice).expect("compute");
        }

        v.reset();
        let silence = vec![0.0f32; 512];
        let silence_prob = v.compute(&silence).expect("compute");

        println!(
            "[v6] voice_prob={:.4} silence_prob={:.4}",
            voice_prob, silence_prob
        );
        assert!(
            voice_prob > silence_prob * 3.0,
            "v6 语音 prob ({:.4}) 应显著高于静音 ({:.4}) 的 3 倍",
            voice_prob,
            silence_prob
        );
    }
}
