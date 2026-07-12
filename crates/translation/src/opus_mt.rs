use anyhow::{Context, Result};
use ort::session::Session;
use ort::value::TensorRef;
use parking_lot::Mutex;
use tokenizers::Tokenizer;

use crate::engine::TranslationEngine;

/// MarianMT decoder 上限 512（config model_max_length），留余量
const MAX_ENCODER_TOKENS: usize = 500;
const MAX_DECODER_LENGTH: usize = 500;

/// Opus-MT MarianMT 引擎。按翻译方向加载对应子目录（zh-en / en-zh）。
pub struct OpusMTEngine {
    encoder: Mutex<Session>,
    decoder: Mutex<Session>,
    tokenizer: Tokenizer,
    decoder_start_id: i64,
    eos_id: i64,
    #[allow(dead_code)]
    pad_id: i64,
}

impl OpusMTEngine {
    /// 按 source/target 方向加载模型。
    /// 目录结构：~/.octopus/models/translate/opus-mt/{zh-en,en-zh}/
    pub fn load(source_lang: &str, target_lang: &str) -> Result<Self> {
        let (dir, _src, _tgt) = resolve_opus_dir(source_lang, target_lang)?;

        let encoder_path = dir.join("onnx/encoder_model_int8.onnx");
        let decoder_path = dir.join("onnx/decoder_model_int8.onnx");
        let tokenizer_path = dir.join("tokenizer.json");

        for (name, path) in [
            ("encoder", &encoder_path),
            ("decoder", &decoder_path),
            ("tokenizer", &tokenizer_path),
        ] {
            if !path.exists() {
                anyhow::bail!("opus-mt 模型文件缺失: {} ({:?})", name, path);
            }
        }

        let encoder_real = std::fs::canonicalize(&encoder_path).unwrap_or(encoder_path);
        let decoder_real = std::fs::canonicalize(&decoder_path).unwrap_or(decoder_path);

        let encoder = {
            let mut b = Session::builder()?;
            b.commit_from_file(&encoder_real)
                .context("加载 opus-mt encoder ONNX 失败")?
        };
        let decoder = {
            let mut b = Session::builder()?;
            b.commit_from_file(&decoder_real)
                .context("加载 opus-mt decoder ONNX 失败")?
        };
        let tokenizer = load_opus_tokenizer(&tokenizer_path)?;

        // 从 generation_config.json 读取关键 token IDs
        let gen_path = dir.join("generation_config.json");
        let (decoder_start_id, eos_id, pad_id) = if gen_path.exists() {
            let gen: serde_json::Value = serde_json::from_reader(
                std::fs::File::open(&gen_path).context("读取 generation_config 失败")?,
            )?;
            (
                gen.get("decoder_start_token_id").and_then(|v| v.as_i64()).unwrap_or(65000),
                gen.get("eos_token_id").and_then(|v| v.as_i64()).unwrap_or(0),
                gen.get("pad_token_id").and_then(|v| v.as_i64()).unwrap_or(65000),
            )
        } else {
            (65000, 0, 65000)
        };

        log::info!("opus-mt 引擎加载完成: {:?} (decoder_start={}, eos={}, pad={})",
            dir, decoder_start_id, eos_id, pad_id);
        Ok(Self {
            encoder: Mutex::new(encoder),
            decoder: Mutex::new(decoder),
            tokenizer,
            decoder_start_id,
            eos_id,
            pad_id: pad_id,
        })
    }

    /// 单 chunk 翻译核心逻辑
    fn translate_chunk(&self, text: &str) -> Result<String> {
        // MarianMT tokenizer：直接 encode，不加语言标记
        let encoding = self.tokenizer.encode(text, false)
            .map_err(|e| anyhow::anyhow!("opus-mt tokenizer encode 失败: {}", e))?;
        let input_ids: Vec<i64> = encoding.get_ids().iter().map(|&id| id as i64).collect();
        let seq_len = input_ids.len();
        if seq_len == 0 {
            return Ok(String::new());
        }

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
        let mut decoder_ids: Vec<i64> = vec![self.decoder_start_id];
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

            if next_token == self.eos_id {
                break;
            }
            // 重复 token 检测
            if decoder_ids.len() >= 10 {
                let last8 = &decoder_ids[decoder_ids.len()-8..];
                if last8.iter().all(|&id| id == next_token) {
                    log::warn!("opus-mt 重复 token 检测触发，停止解码");
                    break;
                }
            }
            decoder_ids.push(next_token);
        }
        drop(decoder);

        // decode：跳过 decoder_start token
        let result_ids: Vec<u32> = decoder_ids[1..].iter().map(|&id| id as u32).collect();
        let decoded = self.tokenizer.decode(&result_ids, true)
            .map_err(|e| anyhow::anyhow!("opus-mt decode 失败: {}", e))?;
        Ok(decoded)
    }

    /// 长文本按句子切分，逐段翻译后拼接
    fn split_and_translate(&self, text: &str) -> Result<String> {
        if text.trim().is_empty() {
            return Ok(String::new());
        }

        // 简单 token 数估算（字符数 × 1.5 作为粗估）
        let estimated_tokens = text.chars().count() * 3 / 2;
        if estimated_tokens <= MAX_ENCODER_TOKENS {
            return self.translate_chunk(text);
        }

        log::info!("opus-mt 长文本分段：~{} tokens", estimated_tokens);
        let sentences = split_sentences(text);
        let mut results: Vec<String> = Vec::with_capacity(sentences.len());

        for (i, sent) in sentences.iter().enumerate() {
            if sent.trim().is_empty() {
                results.push(String::new());
                continue;
            }
            log::info!("opus-mt 翻译段 {}/{}", i + 1, sentences.len());
            let translated = self.translate_chunk(sent)?;
            results.push(translated);
        }

        Ok(results.join("\n"))
    }
}

impl TranslationEngine for OpusMTEngine {
    fn name(&self) -> &str {
        "opus-mt"
    }

    fn translate(&self, text: &str, _source_lang: &str, _target_lang: &str) -> Result<String> {
        // Opus-MT 方向已由 load 时确定，source/target lang 不再需要
        self.split_and_translate(text)
    }
}

/// 加载 opus-mt tokenizer.json。
/// 修复 Xenova 导出的 tokenizer.json 中 precompiled_charsmap=null 导致 tokenizers 0.21 panic。
fn load_opus_tokenizer(path: &std::path::Path) -> Result<Tokenizer> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("读取 tokenizer.json 失败: {:?}", path))?;
    // 含 precompiled_charsmap → 整个 normalizer 块移除（MarianMT 不需要 normalization）
    if raw.contains("\"precompiled_charsmap\"") {
        let mut json: serde_json::Value = serde_json::from_str(&raw)
            .map_err(|e| anyhow::anyhow!("解析 tokenizer.json 失败: {}", e))?;
        // 删除 normalizer 字段
        if let Some(obj) = json.as_object_mut() {
            obj.remove("normalizer");
        }
        let fixed = serde_json::to_string(&json)
            .map_err(|e| anyhow::anyhow!("序列化 tokenizer.json 失败: {}", e))?;
        Tokenizer::from_bytes(fixed.as_bytes())
            .map_err(|e| anyhow::anyhow!("加载修复后的 tokenizer.json 失败: {}", e))
    } else {
        Tokenizer::from_file(path)
            .map_err(|e| anyhow::anyhow!("加载 tokenizer.json 失败: {}", e))
    }
}

/// 按翻译方向解析 opus-mt 子目录。
/// 返回 (目录路径, source_lang, target_lang)。
fn resolve_opus_dir(source_lang: &str, target_lang: &str) -> Result<(std::path::PathBuf, String, String)> {
    // 构建 direction key：zh→en → "zh-en"
    let src = lang_prefix(source_lang);
    let tgt = lang_prefix(target_lang);
    let direction = format!("{}-{}", src, tgt);

    // ~//octopus/models/translate/opus-mt/{direction}/
    let home = std::env::var("HOME").context("HOME not set")?;
    let base = std::path::PathBuf::from(&home)
        .join(".octopus/models/translate/opus-mt")
        .join(&direction);

    if base.is_dir() {
        return Ok((base, src, tgt));
    }

    // 也尝试 HF cache 路径（Xenova/opus-mt-{direction}）
    let hf_repo = format!("Xenova/opus-mt-{}", direction);
    if let Ok(p) = onnx_infra::resolve_model_dir(&hf_repo) {
        return Ok((p, src, tgt));
    }

    anyhow::bail!(
        "opus-mt 方向 '{}' 未找到模型目录。请在设置 > 模型管理 > 翻译模型 中下载，或手动放到 ~/.octopus/models/translate/opus-mt/{}/",
        direction, direction
    )
}

fn lang_prefix(lang: &str) -> String {
    lang.get(..2).unwrap_or(lang).to_lowercase()
}

/// 按句子边界切分文本。支持 CJK 标点和 Latin 标点 + 换行。
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
