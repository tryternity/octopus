use anyhow::{Context, Result};
use ort::session::Session;
use ort::value::TensorRef;
use std::path::Path;
use std::sync::Mutex;

use crate::config;
use crate::fbank::compute_fbank_features;

/// 原版 SenseVoice-Small 引擎（FunASR 原生 4 输入 ONNX 导出，非 sherpa 简化版）。
///
/// 模型如 `WisemeAI/sensevoice-small-quant`：单文件 `model.onnx` + `tokens.json`
/// （string 数组，index=id）+ **`am.mvn`**（kaldi `<AddShift>`+`<Rescale>`，各 560 维，
/// **运行时必须外部应用**——FunASR `WavFrontend` 标准 fbank→LFR→CMVN，CMVN 不进 ONNX 图，
/// `config.yaml` 的 `cmvn_file: null` 是训练残留字段，导出后由 frontend 用 am.mvn 做）。
/// ONNX I/O：
/// - 输入：`speech[N,T,560]`（fbank+LFR m=7/n=6，**已 CMVN**）+ `speech_lengths[N]` int32
///        + `language[N]` int32（0=auto）+ `textnorm[N]` int32（1=不插标点 itn）
/// - 输出：`ctc_logits[N,T,vocab]` + `encoder_out_lens[N]`
///
/// 推理：80-bin fbank + LFR（复用 [`fbank::compute_fbank_features`]，出 560 维）
///       → **CMVN** `(feat+addshift)*rescale`（am.mvn）→ 喂 4 输入 → greedy CTC（blank=0）
///       → tokens.json 文本拼接（跳 `<...>` 特殊 token）。
///
/// 与 [`crate::sensevoice::SenseVoiceEngine`]（sherpa nano 单输入 `x` 版，CMVN 烤进 ONNX）
/// I/O / 词表 / tokens 格式（base64 vs json）均不兼容，故独立引擎 + 独立 category `sensevoice-orig`。
///
/// **CMVN 是必须的**：缺失时真实麦克风语音出现系统性近音字错误（如"开始语音识别"→
/// "开始于饮食别"），合成 TTS 音频因落在模型鲁棒区仍能侥幸通过，故早期合成-wav e2e 未暴露。
pub struct SenseVoiceOrigEngine {
    session: Mutex<Session>,
    vocab: Vec<String>,
    /// am.mvn `<AddShift>` 560 维（= -mean），LFR 后应用。
    cmvn_addshift: Vec<f32>,
    /// am.mvn `<Rescale>` 560 维（= 1/std），LFR 后应用。
    cmvn_rescale: Vec<f32>,
}

/// CTC blank token id（tokens.json[0]=`<unk>`，但 greedy 实测 blank=0 出正确中文）。
const BLANK_ID: i64 = 0;
/// SenseVoice `language` 输入：0=auto（多语自动检测）。
const LANG_AUTO: i32 = 0;
/// SenseVoice `textnorm` 输入：1=不插标点（标点交 octopus pipeline corrector 后处理补，
/// 与其他本地引擎一致；python 实测 textnorm 0/1 对纯中文输出无差异）。
const TEXTNORM_NO_ITN: i32 = 1;

impl SenseVoiceOrigEngine {
    pub fn new(entry: &config::ModelEntry) -> Result<Self> {
        let hf_path =
            config::resolve_model_dir(&entry.source).context("解析 SenseVoice 原版模型目录失败")?;
        let model_path = hf_path.join("model.onnx");
        if !model_path.exists() {
            anyhow::bail!("model.onnx 未找到: {}", model_path.display());
        }
        let session = config::apply_session_acceleration(Session::builder()?)?
            .commit_from_file(&model_path)?;

        // tokens.json：JSON string 数组，index=id，vocab[id]=token。
        let tokens_path = hf_path.join("tokens.json");
        let tokens_text = std::fs::read_to_string(&tokens_path)
            .with_context(|| format!("tokens.json 未找到于 {}", tokens_path.display()))?;
        let vocab: Vec<String> =
            serde_json::from_str(&tokens_text).context("tokens.json 解析失败（应为 string 数组）")?;

        // am.mvn：kaldi nnet 格式，含 <AddShift> + <Rescale> 两块（各 560 维，LFR 后应用）。
        // 缺失 am.mvn 则 CMVN 无法应用 → 真实语音系统性近音错误，故必须加载。
        let mvn_path = hf_path.join("am.mvn");
        let (cmvn_addshift, cmvn_rescale) = parse_kaldi_am_mvn(&mvn_path)
            .with_context(|| format!("am.mvn 解析失败于 {}", mvn_path.display()))?;
        if cmvn_addshift.len() != 560 || cmvn_rescale.len() != 560 {
            anyhow::bail!(
                "am.mvn 维度异常：addshift={} rescale={}（期望各 560）",
                cmvn_addshift.len(),
                cmvn_rescale.len()
            );
        }
        log::info!(
            "[sensevoice-orig] vocab={} tokens, CMVN addshift/rescale 各 560 维已加载",
            vocab.len()
        );

        Ok(Self {
            session: Mutex::new(session),
            vocab,
            cmvn_addshift,
            cmvn_rescale,
        })
    }
}

impl crate::engine::OfflineAsrEngine for SenseVoiceOrigEngine {
    /// 跳过通用中文 corrector：原版 SenseVoice 是高质量模型，自带语言建模，输出已最优，
    /// corrector 基于 n-gram 频率的过纠反而有害。实测模型正确输出"开始语音识别"，
    /// corrector 却把"始语→始于"(gain 1.98)、"音识→饮食"(gain 1.91) 误纠成"开始于饮食别"，
    /// 故跳过（与 qwen3 同机制；[`crate::pipeline::transcribe_batch`] 消费此标志）。
    fn skip_corrector(&self) -> bool {
        true
    }

    fn transcribe(&self, samples: &[f32], _language: &str) -> Result<String> {
        // 80-bin fbank + LFR(m=7/n=6) → [T,560]（复用 sensevoice）。
        let mut features = compute_fbank_features(samples)?;
        let (n_frames, feat_dim) = (features.nrows(), features.ncols());
        // 维度对齐（am.mvn 是 560 = LFR 输出维度）。
        if feat_dim != self.cmvn_addshift.len() {
            anyhow::bail!(
                "LFR feat_dim={} 与 am.mvn 维度={} 不一致",
                feat_dim,
                self.cmvn_addshift.len()
            );
        }
        // CMVN：(feat + addshift) * rescale，等价 (feat - mean) / std。逐帧逐维应用。
        for i in 0..n_frames {
            for j in 0..feat_dim {
                let v = features[[i, j]];
                features[[i, j]] = (v + self.cmvn_addshift[j]) * self.cmvn_rescale[j];
            }
        }

        let x_vec = {
            let (v, _) = features.into_raw_vec_and_offset();
            v
        };
        let speech = ndarray::Array3::from_shape_vec((1, n_frames, feat_dim), x_vec)?;
        let speech_lengths = ndarray::Array1::from_vec(vec![n_frames as i32]);
        let language = ndarray::Array1::from_vec(vec![LANG_AUTO]);
        let textnorm = ndarray::Array1::from_vec(vec![TEXTNORM_NO_ITN]);

        let mut session = self.session.lock().unwrap();
        let outputs = session.run(ort::inputs! {
            "speech" => TensorRef::from_array_view(speech.view())?,
            "speech_lengths" => TensorRef::from_array_view(speech_lengths.view())?,
            "language" => TensorRef::from_array_view(language.view())?,
            "textnorm" => TensorRef::from_array_view(textnorm.view())?,
        })?;

        // ctc_logits[1, T, vocab]（首个输出；第二个是 encoder_out_lens，解码不需要）。
        let (shape, logits) = outputs[0].try_extract_tensor::<f32>()?;
        let dim: Vec<usize> = shape.iter().map(|&d| d as usize).collect();
        if dim.len() != 3 {
            anyhow::bail!("Unexpected ctc_logits rank: {:?}", dim);
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

        // token 拼接：跳过 `<...>` 特殊 token，`▁`（SentencePiece 词首）→ 空格。
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

/// 解析 kaldi nnet am.mvn：提取 `<AddShift>` 与 `<Rescale>` 两块的数值向量。
///
/// 格式（每块）：`<AddShift> 560 560 \n <LearnRateCoef> 0 [ v0 v1 ... v559 ]`
/// 返回 (addshift, rescale)，各为 LFR 输出维度（560）。
fn parse_kaldi_am_mvn(path: &Path) -> Result<(Vec<f32>, Vec<f32>)> {
    let txt = std::fs::read_to_string(path)
        .with_context(|| format!("读取 am.mvn 失败: {}", path.display()))?;
    let addshift = parse_mvn_block(&txt, "AddShift")?;
    let rescale = parse_mvn_block(&txt, "Rescale")?;
    Ok((addshift, rescale))
}

/// 从 am.mvn 文本中提取 `<{tag}>` 后首个 `[ ... ]` 块的数值。
fn parse_mvn_block(txt: &str, tag: &str) -> Result<Vec<f32>> {
    let needle = format!("<{}>", tag);
    let start = txt
        .find(&needle)
        .ok_or_else(|| anyhow::anyhow!("am.mvn 缺少 <{}> 块", tag))?;
    let lb = txt[start..]
        .find('[')
        .ok_or_else(|| anyhow::anyhow!("am.mvn <{}> 后缺少 [", tag))?
        + start;
    let rb = txt[lb..]
        .find(']')
        .ok_or_else(|| anyhow::anyhow!("am.mvn <{}> 后缺少 ]", tag))?
        + lb;
    txt[lb + 1..rb]
        .split_whitespace()
        .map(|s| s.parse::<f32>().context("am.mvn 数值解析失败"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::OfflineAsrEngine;

    /// kaldi am.mvn `<{tag}>` 块解析往返。
    #[test]
    fn parse_mvn_block_extracts_addshift_rescale() {
        let sample = "<Nnet>\n<Splice> 560 560\n[ 0 ]\n\
            <AddShift> 560 560\n<LearnRateCoef> 0 [ -1.5 -2.5 -3.5 ]\n\
            <Rescale> 560 560\n<LearnRateCoef> 0 [ 0.1 0.2 0.3 ]\n";
        assert_eq!(
            parse_mvn_block(sample, "AddShift").unwrap(),
            vec![-1.5, -2.5, -3.5]
        );
        assert_eq!(
            parse_mvn_block(sample, "Rescale").unwrap(),
            vec![0.1, 0.2, 0.3]
        );
    }

    /// 缺失 tag 应报错（而非静默返回空）。
    #[test]
    fn parse_mvn_block_missing_tag_errors() {
        let txt = "<OtherTag> 1 1\n[ 0 ]\n";
        assert!(parse_mvn_block(txt, "AddShift").is_err());
    }

    /// 真实模型 e2e：加载 DB 的 sensevoice-orig-small，识别 $OCTOPUS_TEST_WAV（若设）。
    /// 无环境变量则 skip。本地验证：OCTOPUS_TEST_WAV=/tmp/x.wav cargo test sensevoice_orig。
    ///
    /// 注意：若用 TTS 合成 wav（如 macOS `say`），CMVN 修复前后均能识别（合成音频落在
    /// 模型鲁棒区）；CMVN 修复主要改善真实麦克风录音——质量验证需用真实录音人工核对。
    #[test]
    fn sensevoice_orig_real_model_transcribes() {
        let wav = match std::env::var("OCTOPUS_TEST_WAV") {
            Ok(p) if !p.is_empty() => p,
            _ => {
                eprintln!("[SKIP] 未设 OCTOPUS_TEST_WAV — 跳过 SenseVoice 原版 e2e");
                return;
            }
        };
        let cfg = crate::config::load_config().expect("load_config 失败");
        let entry = match crate::config::pick_entry(
            &cfg,
            crate::config::EngineCategory::SenseVoiceOrig,
            "sensevoice-orig-small",
        ) {
            Some(e) => e,
            None => {
                eprintln!("[SKIP] sensevoice-orig-small 不在 DB — 跳过");
                return;
            }
        };
        let engine = match SenseVoiceOrigEngine::new(entry) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("[SKIP] SenseVoiceOrigEngine::new 失败（HF 缓存未就绪?）: {e}");
                return;
            }
        };
        let samples = crate::audio::read_wav_16k(&wav).expect("读 wav 失败");
        let text = engine.transcribe(&samples, "zh").expect("transcribe 失败");
        println!("[SenseVoice-Orig] {:?} => {:?}", wav, text);
        assert!(!text.is_empty(), "识别结果不应为空");
    }
}
