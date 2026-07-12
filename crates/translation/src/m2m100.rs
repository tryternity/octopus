use anyhow::{Context, Result};
use ort::session::Session;
use ort::value::TensorRef;
use parking_lot::Mutex;

use crate::engine::TranslationEngine;
use crate::tokenizer::{M2M100Tokenizer, EOS_ID, DECODER_START_TOKEN_ID, lang_code_to_id};

/// encoder 上限 1024，留余量给 lang token + eos + margin
const MAX_ENCODER_TOKENS: usize = 900;
/// decoder 单 chunk 最大生成长度
const MAX_DECODER_LENGTH: usize = 200;
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

    /// 单 chunk 翻译核心逻辑（不做分段）
    fn translate_chunk(&self, text: &str, source_lang: &str, target_lang: &str) -> Result<String> {
        let input_ids = self.tokenizer.encode(text, source_lang)?;
        let seq_len = input_ids.len();

        let input_ids_arr = ndarray::Array2::from_shape_vec((1, seq_len), input_ids)?;
        let attention_mask_arr =
            ndarray::Array2::from_shape_vec((1, seq_len), vec![1i64; seq_len])?;

        // Encoder
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

        // Decoder greedy loop
        let target_lang_id = lang_code_to_id(target_lang, self.tokenizer.tokenizer())
            .unwrap_or(128022) as i64;
        let mut decoder_ids: Vec<i64> = vec![DECODER_START_TOKEN_ID, target_lang_id];
        let mut decoder = self.decoder.lock();

        for _ in 0..MAX_DECODER_LENGTH {
            let dec_len = decoder_ids.len();
            let dec_arr =
                ndarray::Array2::from_shape_vec((1, dec_len), decoder_ids.clone())?;

            let dec_outputs = decoder.run(ort::inputs! {
                "input_ids" => TensorRef::from_array_view(dec_arr.view())?,
                "encoder_hidden_states" => TensorRef::from_array_view(enc_hidden.view())?,
                "encoder_attention_mask" => TensorRef::from_array_view(attention_mask_arr.view())?,
            })?;

            let (logits_shape, logits_data) = dec_outputs["logits"]
                .try_extract_tensor::<f32>()?;
            let vocab_size = logits_shape[2] as usize;
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
            if decoder_ids.len() >= 10 {
                // 连续 8 个相同 token 才触发（提高阈值）
                // 仅检查非标点 token：标点/空格重复是正常的（如 ...... 或连续空格）
                let last8 = &decoder_ids[decoder_ids.len()-8..];
                if last8.iter().all(|&id| id == next_token) {
                    log::warn!("重复 token 检测触发，停止解码");
                    break;
                }
            }
            decoder_ids.push(next_token);
        }
        drop(decoder);

        let result_ids: Vec<i64> = decoder_ids[2..].to_vec();
        self.tokenizer.decode(&result_ids)
    }

    /// 将长文本按句子切分，打包为不超过 MAX_ENCODER_TOKENS 的 chunk。
    fn split_into_chunks(&self, text: &str, source_lang: &str) -> Result<Vec<String>> {
        let full_ids = self.tokenizer.encode(text, source_lang)?;
        // 减去 lang token (1) + eos (1) = 2 个额外 token
        if full_ids.len() <= MAX_ENCODER_TOKENS {
            return Ok(vec![text.to_string()]);
        }

        log::info!("长文本分段：{} tokens，按句子切分", full_ids.len());

        // 按句子边界切分（CJK + Latin 标点 + 换行）
        let sentences = split_sentences(text);
        let mut chunks: Vec<String> = Vec::new();
        let mut current = String::new();
        let mut current_token_count = 2usize; // lang + eos 预留

        for sentence in sentences {
            let sent_ids = self.tokenizer.encode(&sentence, source_lang)?;
            // 句子本身的 token 数（减去 lang + eos）
            let sent_tokens = sent_ids.len().saturating_sub(2);

            if sent_tokens > MAX_ENCODER_TOKENS - 2 {
                // 单句就超限——按字符硬切
                if !current.is_empty() {
                    chunks.push(std::mem::take(&mut current));
                    current_token_count = 2;
                }
                let chars: Vec<char> = sentence.chars().collect();
                let mut start = 0;
                while start < chars.len() {
                    let mut end = (start + 200).min(chars.len());
                    // 尽量在标点或空格处切——向前搜索边界，找不到则硬切
                    while end > start + 100 && !is_boundary(chars[end - 1]) {
                        end -= 1;
                    }
                    if end == start { end = (start + 200).min(chars.len()); }
                    if end > start {
                        chunks.push(chars[start..end].iter().collect());
                    }
                    start = end;
                }
                continue;
            }

            if current_token_count + sent_tokens > MAX_ENCODER_TOKENS {
                if !current.is_empty() {
                    chunks.push(std::mem::take(&mut current));
                    current_token_count = 2;
                }
            }

            current.push_str(&sentence);
            current_token_count += sent_tokens;
        }

        if !current.is_empty() {
            chunks.push(current);
        }

        log::info!("分段完成：{} chunks", chunks.len());
        Ok(chunks)
    }
}

impl TranslationEngine for M2M100Engine {
    fn name(&self) -> &str {
        "m2m100-418M"
    }

    fn translate(&self, text: &str, source_lang: &str, target_lang: &str) -> Result<String> {
        if text.trim().is_empty() {
            return Ok(String::new());
        }
        let chunks = self.split_into_chunks(text, source_lang)?;

        if chunks.len() == 1 {
            return self.translate_chunk(&chunks[0], source_lang, target_lang);
        }

        // 多 chunk：逐段翻译，拼接结果
        let mut results: Vec<String> = Vec::with_capacity(chunks.len());
        for (i, chunk) in chunks.iter().enumerate() {
            log::info!("翻译 chunk {}/{} ({} chars)", i + 1, chunks.len(), chunk.len());
            let translated = self.translate_chunk(chunk, source_lang, target_lang)?;
            results.push(translated);
        }

        Ok(results.join("\n"))
    }
}

/// 按句子边界切分文本。支持 CJK 标点（。！？）和 Latin 标点（.!?）+ 换行。
fn split_sentences(text: &str) -> Vec<String> {
    let mut sentences: Vec<String> = Vec::new();
    let mut current = String::new();

    for ch in text.chars() {
        current.push(ch);
        if is_sentence_end(ch) {
            sentences.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        sentences.push(current);
    }

    if sentences.is_empty() {
        vec![text.to_string()]
    } else {
        sentences
    }
}

fn is_sentence_end(ch: char) -> bool {
    matches!(ch, '。' | '！' | '？' | '．' | '\n' | '.' | '!' | '?' | ';' | '；')
}

fn is_boundary(ch: char) -> bool {
    is_sentence_end(ch) || ch == ' ' || ch == ',' || ch == '，'
}
