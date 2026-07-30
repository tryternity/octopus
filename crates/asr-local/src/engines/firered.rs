use anyhow::{Context, Result};
use ort::session::Session;
use parking_lot::Mutex;

use crate::config;
use crate::fbank::{compute_fbank, POVEY_WINDOW};

/// FireRedASR2 CTC 引擎 —— FireRedASR2-AED 的 encoder + CTC branch 导出（attention decoder 弃用）。
///
/// 模型来自 `VidraAI/FireRedASR2-onnx`（k2-fsa 维护的 sherpa 格式）。单文件
/// `model.int8.onnx`(740M) + `tokens.txt`(vocab=8667)。CMVN 存于 ONNX metadata
/// （`cmvn_mean` / `cmvn_inv_stddev`），与 paraformer 同范式但 key 名不同、mean 为正值
/// （公式 `(fbank - mean) * inv_stddev`，**无** paraformer 的 `sqrt(enc_out)` scale——
/// CTC encoder 直接吃 80 维 fbank）。
///
/// 推理：80-bin fbank（复用 [`fbank::compute_fbank`]，无 LFR）→ CMVN
///       → `x[N,T,80]` + `x_lens[N]` → `log_probs[N,T,8667]` → greedy CTC（blank=0，
///       相邻去重）→ token 文本拼接（跳 `<...>` 特殊/方言 token，`▁`→空格）。
pub struct FireRedEngine {
    session: Mutex<Session>,
    vocab: Vec<String>,
    cmvn_mean: Vec<f32>,
    cmvn_inv_stddev: Vec<f32>,
}

/// CTC blank token id（tokens.txt 首行 `<blank> 0`）。
const BLANK_ID: i64 = 0;

impl FireRedEngine {
    pub fn new(entry: &config::ModelEntry) -> Result<Self> {
        let hf_path =
            config::resolve_model_dir(&entry.source).context("解析 FireRedASR2 模型目录失败")?;
        let model_path = hf_path.join("model.int8.onnx");
        if !model_path.exists() {
            anyhow::bail!("model.int8.onnx 未找到: {}", model_path.display());
        }
        let session = config::apply_session_acceleration(Session::builder()?)?
            .commit_from_file(&model_path)?;

        // CMVN 从 ONNX metadata 读（k2-fsa 导出格式：cmvn_mean / cmvn_inv_stddev，逗号分隔）。
        // 与 paraformer（neg_mean / inv_stddev）同手法，但 key 名不同、mean 为正值、无 enc_out scale。
        // 限缩在块内：metadata 借用 session，块结束 drop 释放借用，下面方能 move session 进 Mutex。
        let (cmvn_mean, cmvn_inv_stddev) = {
            let metadata = session.metadata()?;
            let mean = parse_cmvn_vec(
                &metadata
                    .custom("cmvn_mean")
                    .context("ONNX metadata 缺 cmvn_mean")?,
            )?;
            let inv_std = parse_cmvn_vec(
                &metadata
                    .custom("cmvn_inv_stddev")
                    .context("ONNX metadata 缺 cmvn_inv_stddev")?,
            )?;
            (mean, inv_std)
        };
        log::info!(
            "[firered] CMVN: {} means, {} inv_stddevs",
            cmvn_mean.len(),
            cmvn_inv_stddev.len()
        );

        // tokens.txt：每行 `token id`，vocab[id] = token（与 paraformer 同解析模式）。
        let tokens_path = hf_path.join("tokens.txt");
        let tokens_text = std::fs::read_to_string(&tokens_path)
            .with_context(|| format!("tokens.txt 未找到于 {}", tokens_path.display()))?;
        let mut vocab: Vec<String> = Vec::new();
        for line in tokens_text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Some((token, id_str)) = line.rsplit_once(' ') {
                if let Ok(id) = id_str.parse::<usize>() {
                    while vocab.len() <= id {
                        vocab.push(String::new());
                    }
                    vocab[id] = token.to_string();
                }
            }
        }

        Ok(Self {
            session: Mutex::new(session),
            vocab,
            cmvn_mean,
            cmvn_inv_stddev,
        })
    }
}

impl crate::engine::OfflineAsrEngine for FireRedEngine {
    fn transcribe(&self, samples: &[f32], _language: &str) -> Result<String> {
        // 80-bin fbank（含 *32768 预处理，无 LFR）。
        // FireRedASR 训练用 kaldi_native_fbank 默认（FireRedTeam/FireRedASR data/asr_feat.py
        // 仅覆盖 dither/num_bins/snip_edges）：preemph=0.97 + povey 窗 + DC offset。
        // 2026-07-09 确认对齐（此前 preemph=0.0+hamming 是未确认时的保守旧行为）。
        let scaled: Vec<f32> = samples.iter().map(|&s| s * 32768.0).collect();
        let mut features = compute_fbank(&scaled, &POVEY_WINDOW, 0.97)?;
        let (n_frames, feat_dim) = (features.nrows(), features.ncols());

        // CMVN：(fbank - mean) * inv_std（逐维；feat_dim=80 与 metadata 维度对齐）。
        for i in 0..n_frames {
            for j in 0..feat_dim {
                if j < self.cmvn_mean.len() && j < self.cmvn_inv_stddev.len() {
                    features[[i, j]] =
                        (features[[i, j]] - self.cmvn_mean[j]) * self.cmvn_inv_stddev[j];
                }
            }
        }

        // x[N,T,80] + x_lens[N] (int64) → log_probs[N,T,vocab]
        let x_vec = {
            let (v, _) = features.into_raw_vec_and_offset();
            v
        };
        let x = ndarray::Array3::from_shape_vec((1, n_frames, feat_dim), x_vec)?;
        let x_lens_arr = [n_frames as i64];
        let x_lens = ndarray::ArrayView1::from(&x_lens_arr);

        let mut session = self.session.lock();
        let outputs = session.run(ort::inputs! {
            "x" => ort::value::TensorRef::from_array_view(x.view())?,
            "x_lens" => ort::value::TensorRef::from_array_view(x_lens)?
        })?;

        // log_probs[1, T, vocab]（首个输出；第二个是 log_probs_len，解码不需要）。
        let (shape, logits) = outputs[0].try_extract_tensor::<f32>()?;
        let dim: Vec<usize> = shape.iter().map(|&d| d as usize).collect();
        if dim.len() != 3 {
            anyhow::bail!("Unexpected log_probs rank: {:?}", dim);
        }
        let (n_time, vocab_size) = (dim[1], dim[2]);

        // greedy CTC：blank_id=0，相邻去重。
        let mut deduped: Vec<i64> = Vec::new();
        let mut prev: i64 = -1;
        for t in 0..n_time {
            let offset = t * vocab_size;
            let frame = &logits[offset..offset + vocab_size];
            let best = frame
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
                .map(|(i, _)| i as i64)
                .unwrap_or(0);
            if best != prev && best != BLANK_ID {
                deduped.push(best);
            }
            prev = best;
        }

        // token 拼接：跳过 `<...>` 特殊/方言 token，`▁`（SentencePiece 词首）→ 空格。
        let mut text = String::new();
        for &tid in &deduped {
            let idx = tid as usize;
            if idx > 0 && idx < self.vocab.len() {
                let tok = &self.vocab[idx];
                if tok.starts_with('<') && tok.ends_with('>') {
                    continue;
                }
                text.push_str(tok);
            }
        }
        let text = text.replace('▁', " ");
        Ok(text.trim().to_string())
    }
}

/// 解析 CMVN metadata（逗号分隔 float 串）。
fn parse_cmvn_vec(s: &str) -> Result<Vec<f32>> {
    s.split(',')
        .map(|x| {
            x.trim()
                .parse::<f32>()
                .with_context(|| format!("CMVN 值解析失败: '{}'", x))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::OfflineAsrEngine;

    #[test]
    fn parse_cmvn_vec_splits_comma_floats() {
        let v = parse_cmvn_vec("10.5, 0.25,3.0").unwrap();
        assert_eq!(v, vec![10.5, 0.25, 3.0]);
    }

    #[test]
    fn parse_cmvn_vec_rejects_non_float() {
        assert!(parse_cmvn_vec("1.0,abc,2.0").is_err());
        assert!(parse_cmvn_vec("").is_err());
    }

    /// 真实模型 e2e：加载 DB 的 firered-asr2，识别 $OCTOPUS_TEST_WAV（若设）。
    /// 无环境变量则 skip（CI 无音频时不阻塞）；本地验证：OCTOPUS_TEST_WAV=/tmp/x.wav cargo test -- --ignored。
    #[test]
    #[ignore = "real-model: 需 DB 引擎 + OCTOPUS_TEST_WAV，cargo test -- --ignored 跑"]
    fn firered_real_model_transcribes() {
        let wav = match std::env::var("OCTOPUS_TEST_WAV") {
            Ok(p) if !p.is_empty() => p,
            _ => {
                eprintln!("[SKIP] 未设 OCTOPUS_TEST_WAV — 跳过 FireRedASR2 真模型 e2e");
                return;
            }
        };
        let cfg = crate::config::load_config().expect("load_config 失败");
        let entry = match crate::config::pick_entry(
            &cfg,
            crate::config::EngineCategory::FireRed,
            "firered-asr2",
        ) {
            Some(e) => e,
            None => {
                eprintln!("[SKIP] firered-asr2 不在 DB — 跳过");
                return;
            }
        };
        let engine = match FireRedEngine::new(entry) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("[SKIP] FireRedEngine::new 失败（HF 缓存未就绪?）: {e}");
                return;
            }
        };
        let samples = crate::audio::read_wav_16k(&wav).expect("读 wav 失败");
        let text = engine.transcribe(&samples, "zh").expect("transcribe 失败");
        println!("[FireRedASR2] {:?} => {:?}", wav, text);
        assert!(!text.is_empty(), "识别结果不应为空");
    }
}
