use anyhow::{Context, Result};
use ort::session::Session;
use ort::value::TensorRef;
use parking_lot::Mutex;

use crate::engine::TranslationEngine;
use crate::tokenizer::{M2M100Tokenizer, EOS_ID, DECODER_START_TOKEN_ID, lang_code_to_id};

const MAX_LENGTH: usize = 200;
const M2M100_REPO: &str = "lazycodepersona/m2m100_418m";

pub struct M2M100Engine {
    encoder: Mutex<Session>,
    decoder: Mutex<Session>,
    tokenizer: M2M100Tokenizer,
}

impl M2M100Engine {
    pub fn load() -> Result<Self> {
        let model_dir = onnx_infra::resolve_model_dir(M2M100_REPO)
            .context("m2m100 模型未找到，请在设置 > 模型管理 > 翻译模型 中下载")?;

        let encoder_path = model_dir.join("onnx/encoder_model_quantized.onnx");
        let decoder_path = model_dir.join("onnx/decoder_model_quantized.onnx");
        let tokenizer_path = model_dir.join("tokenizer.json");

        for (name, path) in [
            ("encoder", &encoder_path),
            ("decoder", &decoder_path),
            ("tokenizer", &tokenizer_path),
        ] {
            if !path.exists() {
                anyhow::bail!("模型文件缺失: {} ({:?})", name, path);
            }
        }

        // HF cache 使用符号链接——读取真实文件路径
        let encoder_real = std::fs::canonicalize(&encoder_path).unwrap_or(encoder_path);
        let decoder_real = std::fs::canonicalize(&decoder_path).unwrap_or(decoder_path);

        let encoder = {
            let mut b = Session::builder()?;
            b.commit_from_file(&encoder_real)
                .context("加载 encoder ONNX 失败")?
        };
        let decoder = {
            let mut b = Session::builder()?;
            b.commit_from_file(&decoder_real)
                .context("加载 decoder ONNX 失败")?
        };
        let tokenizer = M2M100Tokenizer::load(&tokenizer_path)?;

        log::info!("m2m100 引擎加载完成: {:?}", model_dir);
        Ok(Self {
            encoder: Mutex::new(encoder),
            decoder: Mutex::new(decoder),
            tokenizer,
        })
    }
}

impl TranslationEngine for M2M100Engine {
    fn name(&self) -> &str {
        "m2m100-418M"
    }

    fn translate(&self, text: &str, source_lang: &str, target_lang: &str) -> Result<String> {
        // 1. Tokenize
        let input_ids = self.tokenizer.encode(text, source_lang)?;
        let seq_len = input_ids.len();

        // 构建 encoder 输入张量 [1, seq_len]
        let input_ids_arr = ndarray::Array2::from_shape_vec((1, seq_len), input_ids)?;
        let attention_mask_arr =
            ndarray::Array2::from_shape_vec((1, seq_len), vec![1i64; seq_len])?;

        // 2. Encoder forward — 提取 hidden states 后立即释放锁
        let enc_hidden = {
            let mut encoder = self.encoder.lock();
            let enc_outputs = encoder.run(ort::inputs! {
                "input_ids" => TensorRef::from_array_view(input_ids_arr.view())?,
                "attention_mask" => TensorRef::from_array_view(attention_mask_arr.view())?,
            })?;
            let (enc_shape, enc_data) = enc_outputs["last_hidden_state"]
                .try_extract_tensor::<f32>()?;
            let d: Vec<usize> = enc_shape.iter().map(|&v| v as usize).collect();
            ndarray::Array3::from_shape_vec((d[0], d[1], d[2]), enc_data.to_vec())?
        };

        // 3. Decoder greedy loop
        // m2m100 decoder 初始输入：[eos, target_lang_id]
        let target_lang_id = lang_code_to_id(target_lang, self.tokenizer.tokenizer())
            .unwrap_or(128022) as i64;
        let mut decoder_ids: Vec<i64> = vec![DECODER_START_TOKEN_ID, target_lang_id];
        let mut decoder = self.decoder.lock();

        for _ in 0..MAX_LENGTH {
            let dec_len = decoder_ids.len();
            let dec_arr =
                ndarray::Array2::from_shape_vec((1, dec_len), decoder_ids.clone())?;

            let dec_outputs = decoder.run(ort::inputs! {
                "input_ids" => TensorRef::from_array_view(dec_arr.view())?,
                "encoder_hidden_states" => TensorRef::from_array_view(enc_hidden.view())?,
                "encoder_attention_mask" => TensorRef::from_array_view(attention_mask_arr.view())?,
            })?;

            // logits: [1, dec_len, vocab_size]
            let (logits_shape, logits_data) = dec_outputs["logits"]
                .try_extract_tensor::<f32>()?;
            let vocab_size = logits_shape[2] as usize;
            // 最后一个位置的 logits（batch=0, position=dec_len-1）
            let offset = (dec_len - 1) * vocab_size;
            let last_logits = &logits_data[offset..offset + vocab_size];

            let next_token = last_logits
                .iter()
                .enumerate()
                .max_by(|(_, a), (_, b)| {
                    a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)
                })
                .map(|(i, _)| i as i64)
                .unwrap_or(0);

            if next_token == EOS_ID {
                break;
            }
            // 重复 token 检测：连续 5 个相同 token 则停止
            if decoder_ids.len() >= 6 {
                let last5 = &decoder_ids[decoder_ids.len()-5..];
                if last5.iter().all(|&id| id == next_token) {
                    log::warn!("重复 token 检测触发，停止解码");
                    break;
                }
            }
            decoder_ids.push(next_token);
        }
        drop(decoder);

        // 4. Detokenize（跳过 start token + target lang token）
        let result_ids: Vec<i64> = decoder_ids[2..].to_vec();
        let text = self.tokenizer.decode(&result_ids)?;
        Ok(text)
    }
}
