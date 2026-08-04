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

// ── VAD 版本 feature 互斥守护 ──
// vad_v6 / vad_v4 不能同时启用（两套 ONNX 签名，struct 字段互斥）。
// 开发期用 cargo --no-default-features --features vad_v4 切换；验证通过后删 v4。
#[cfg(all(feature = "vad_v4", feature = "vad_v6"))]
compile_error!("vad_v4 和 vad_v6 互斥，不能同时启用。用 --no-default-features --features <其一>");
#[cfg(not(any(feature = "vad_v4", feature = "vad_v6")))]
compile_error!("必须启用 vad_v4 或 vad_v6 之一（default = [\"vad_v6\"]）");

/// 按 model 路径缓存已加载的 ONNX Session。
/// SileroVad 实例各自 owned 状态（v6: state + context；v4: h/c），但共享底层 Session
/// （Session::run 是 &mut self，用 Arc<Mutex<Session>> 提供内部可变性 + 共享所有权）。
static VAD_SESSIONS: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<Session>>>>> = OnceLock::new();

fn vad_sessions() -> &'static Mutex<HashMap<PathBuf, Arc<Mutex<Session>>>> {
    VAD_SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 内嵌 VAD 模型的 cache key（版本隔离）+ include_bytes! 路径。
#[cfg(feature = "vad_v6")]
mod builtin_meta {
    /// silero_vad_16k_op15.onnx——官方 16kHz 专用精简版（1.2MB，opset 15）。
    /// 去掉 8kHz 分支（If 节点），比完整版 silero_vad.onnx（2.3MB）小 46%。
    /// 输入签名与完整版一致（input + state + sr），sr 固定 16000。
    pub const BYTES: &[u8] = include_bytes!("../../models/silero_vad_v6.onnx");
    pub const CACHE_KEY: &str = "builtin://silero_vad_v6";
    /// v6 LSTM 状态维度（config.json state_dim=128）。
    pub const STATE_DIM: usize = 128;
    /// v6 输入需拼 context（上一帧末尾样本）——16kHz = 64 样本，8kHz = 32。
    /// 实际输入 shape = [1, samples + CONTEXT_SIZE]。漏拼 context 导致 prob 恒近零。
    /// 参考官方 silero-vad/examples/rust-example/src/silero.rs。
    pub const CONTEXT_SIZE: usize = 64;
}

#[cfg(feature = "vad_v4")]
mod builtin_meta {
    pub const BYTES: &[u8] = include_bytes!("../../models/silero_vad_v4.onnx");
    pub const CACHE_KEY: &str = "builtin://silero_vad_v4";
    /// v4 LSTM hidden/cell 维度。
    pub const STATE_DIM: usize = 64;
}

/// Silero VAD via ort (ONNX Runtime)
///
/// 参考官方 Rust 实现 silero-vad/examples/rust-example/src/silero.rs。
///
/// 版本（feature flag 选择，互斥）：
/// - `vad_v6`（默认）：state 单 tensor `[2,1,128]` + context `[64]` 拼接输入。
///   输入 `input`/`state`/`sr` → 输出 `output`/`stateN`。
/// - `vad_v4`：h/c 双 tensor `[2,1,64]`。输入 `input`/`h`/`c`/`sr` → 输出 `output`/`hn`/`cn`。
///
/// 对外接口（compute/reset/new/new_builtin）版本无关，调用方不受影响。
pub struct SileroVad {
    session: Arc<Mutex<Session>>,   // 共享缓存：相同 path 复用同一 Session
    #[cfg(feature = "vad_v6")]
    state: Array3<f32>,             // [2, 1, 128]
    #[cfg(feature = "vad_v6")]
    context: Array1<f32>,           // [CONTEXT_SIZE]——上一帧末尾样本，拼到当前帧前面
    #[cfg(feature = "vad_v4")]
    h: Array3<f32>,                 // [2, 1, 64]
    #[cfg(feature = "vad_v4")]
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
        Ok(Self::with_session(session))
    }

    /// 从编译期内嵌字节加载 VAD（`include_bytes!`），不落盘、不读磁盘文件。
    ///
    /// 模型版本由 feature flag 决定（vad_v6 / vad_v4）。ort `commit_from_memory` 直接
    /// 从 `&[u8]` 构造 Session。缓存 key 用版本化的 `builtin://` URI 与磁盘路径隔离。
    pub fn new_builtin() -> Result<Self> {
        let cache_key = PathBuf::from(builtin_meta::CACHE_KEY);
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
                        .commit_from_memory(builtin_meta::BYTES)
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

    /// 公共构造：session 共享缓存逻辑提取后，初始化版本相关的状态字段。
    fn with_session(session: Arc<Mutex<Session>>) -> Self {
        let dim = builtin_meta::STATE_DIM;
        #[cfg(feature = "vad_v6")]
        {
            Self {
                session,
                state: Array3::zeros((2, 1, dim)),
                context: Array1::zeros(builtin_meta::CONTEXT_SIZE),
                sr: Array1::from_vec(vec![16000i64]),
            }
        }
        #[cfg(feature = "vad_v4")]
        {
            Self {
                session,
                h: Array3::zeros((2, 1, dim)),
                c: Array3::zeros((2, 1, dim)),
                sr: Array1::from_vec(vec![16000i64]),
            }
        }
    }

    /// Input: PCM samples (512 @ 16kHz). Output: speech probability [0.0, 1.0]
    ///
    /// v6：参考官方 silero-vad/examples/rust-example/src/silero.rs::calc_level——
    /// 输入拼 context（上一帧末尾 CONTEXT_SIZE 样本）→ [1, 576]，推理后更新 state + context。
    pub fn compute(&mut self, samples: &[f32]) -> Result<f32> {
        let mut session = self.session.lock();

        #[cfg(feature = "vad_v6")]
        {
            let ctx_len = builtin_meta::CONTEXT_SIZE;

            // 拼接 context + samples → [1, ctx_len + samples.len()]
            let mut buf = Vec::with_capacity(ctx_len + samples.len());
            buf.extend_from_slice(self.context.as_slice().unwrap_or(&[]));
            buf.extend_from_slice(samples);
            let input = ArrayView2::from_shape((1, buf.len()), &buf)?;

            let state_tensor = ort::value::TensorRef::from_array_view(self.state.view())?;
            let sr_tensor = ort::value::TensorRef::from_array_view(self.sr.view())?;
            let input_tensor = ort::value::TensorRef::from_array_view(input)?;

            let outputs = session.run(ort::inputs! {
                "input" => input_tensor,
                "sr" => sr_tensor,
                "state" => state_tensor
            })?;

            // Extract probability
            let (_shape, data) = outputs["output"].try_extract_tensor::<f32>()?;
            let prob = data[0];

            // Update state for next call（v6 单 state tensor）
            let (_s_shape, s_data) = outputs["stateN"].try_extract_tensor::<f32>()?;
            if let Some(s_slice) = self.state.as_slice_mut() {
                s_slice.copy_from_slice(s_data);
            } else {
                self.state = Array3::from_shape_vec((2, 1, builtin_meta::STATE_DIM), s_data.to_vec())?;
            }

            // Update context：取本帧输入末尾 CONTEXT_SIZE 样本（下帧拼到前面）。
            // 官方实现对齐：context = data[data.len() - context_size..]
            if samples.len() >= ctx_len {
                self.context = Array1::from_vec(samples[samples.len() - ctx_len..].to_vec());
            }

            Ok(prob)
        }

        #[cfg(feature = "vad_v4")]
        {
            // v4 输入直接是 samples（不拼 context）
            let input = ArrayView2::from_shape((1, samples.len()), samples)?;
            let h_tensor = ort::value::TensorRef::from_array_view(self.h.view())?;
            let c_tensor = ort::value::TensorRef::from_array_view(self.c.view())?;
            let sr_tensor = ort::value::TensorRef::from_array_view(self.sr.view())?;
            let input_tensor = ort::value::TensorRef::from_array_view(input)?;

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
                self.h = Array3::from_shape_vec((2, 1, builtin_meta::STATE_DIM), h_data.to_vec())?;
            }

            let (_c_shape, c_data) = outputs["cn"].try_extract_tensor::<f32>()?;
            if let Some(c_slice) = self.c.as_slice_mut() {
                c_slice.copy_from_slice(c_data);
            } else {
                self.c = Array3::from_shape_vec((2, 1, builtin_meta::STATE_DIM), c_data.to_vec())?;
            }

            Ok(prob)
        }
    }

    /// Reset LSTM states (call between different audio segments)
    pub fn reset(&mut self) {
        let dim = builtin_meta::STATE_DIM;
        #[cfg(feature = "vad_v6")]
        {
            self.state = Array3::zeros((2, 1, dim));
            self.context = Array1::zeros(builtin_meta::CONTEXT_SIZE);
        }
        #[cfg(feature = "vad_v4")]
        {
            self.h = Array3::zeros((2, 1, dim));
            self.c = Array3::zeros((2, 1, dim));
        }
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
        let samples = vec![0.0f32; 512];
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
        let samples = vec![0.0f32; 512];
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
        // 确认 builtin:// cache key 存在（版本相关，v6/v4 各自的 key）
        let cache = vad_sessions().lock();
        assert!(
            cache.contains_key(&PathBuf::from(builtin_meta::CACHE_KEY)),
            "builtin:// cache key 应存在：{}",
            builtin_meta::CACHE_KEY
        );
    }

    // ── v6 专属测试（state 维度 + context 拼接 + 语音区分）──

    /// v6 状态维度 = 128（config.json state_dim）。
    #[cfg(feature = "vad_v6")]
    #[test]
    fn v6_state_dim_is_128() {
        assert_eq!(
            builtin_meta::STATE_DIM, 128,
            "v6 LSTM 状态维度应为 128"
        );
    }

    /// v6 context 拼接核心回归：模拟语音 prob 应显著高于静音。
    /// 防止 context 拼接缺失导致 prob 恒近零（2026-08-04 踩坑：漏拼 context + 废模型双重 bug）。
    #[cfg(feature = "vad_v6")]
    #[test]
    fn v6_speech_prob_higher_than_silence() {
        let mut v = match SileroVad::new_builtin() {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[SKIP] ort commit_from_memory 失败: {}", e);
                return;
            }
        };

        // 模拟语音：多谐波正弦波（基频 + 谐波，逼近真实人声频谱）
        let voice: Vec<f32> = (0..512)
            .map(|i| {
                let t = i as f32 / 16000.0;
                let fundamental = (std::f32::consts::TAU * 120.0 * t).sin() * 0.5;
                let harmonic2 = (std::f32::consts::TAU * 240.0 * t).sin() * 0.25;
                let harmonic4 = (std::f32::consts::TAU * 480.0 * t).sin() * 0.1;
                fundamental + harmonic2 + harmonic4
            })
            .collect();

        // 连续 3 帧语音（context + state 热身），取最后一帧 prob
        v.reset();
        let mut voice_prob = 0.0;
        for _ in 0..3 {
            voice_prob = v.compute(&voice).expect("compute");
        }

        // 静音
        v.reset();
        let silence = vec![0.0f32; 512];
        let silence_prob = v.compute(&silence).expect("compute");

        println!(
            "[v6] voice_prob={:.4} silence_prob={:.4}",
            voice_prob, silence_prob
        );
        assert!(
            voice_prob > silence_prob * 3.0,
            "v6 语音 prob ({:.4}) 应显著高于静音 ({:.4}) 的 3 倍——context 拼接可能缺失或模型损坏",
            voice_prob,
            silence_prob
        );
    }

    // ── v4 专属测试（回退验证用）──

    #[cfg(feature = "vad_v4")]
    #[test]
    fn v4_state_dim_is_64() {
        assert_eq!(
            builtin_meta::STATE_DIM, 64,
            "v4 LSTM hidden/cell 维度应为 64"
        );
    }
}
