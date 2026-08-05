use anyhow::{Context, Result};
use ort::session::Session;
use ort::value::TensorRef;
use parking_lot::Mutex;
use tokenizers::Tokenizer;

use crate::engine::TranslationEngine;

/// MarianMT decoder 上限 512（config model_max_length），留余量
const MAX_ENCODER_TOKENS: usize = 500;
const MAX_DECODER_LENGTH: usize = 500;
/// 禁止 3-gram 重复（标准 HF 默认值）
const NO_REPEAT_NGRAM_SIZE: usize = 3;
/// 重复惩罚因子（logit / penalty，>1 降低已出现 token 概率）
const REPETITION_PENALTY: f32 = 1.3;

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
            pad_id,
        })
    }

    /// 单 chunk 翻译核心逻辑
    fn translate_chunk(&self, text: &str) -> Result<String> {
        // MarianMT tokenizer：encode(text, true) 让 post_processor 自动补 </s>（eos），
        // 加 truncation 防超 encoder 上限
        let mut encoding = self.tokenizer.encode(text, true)
            .map_err(|e| anyhow::anyhow!("opus-mt tokenizer encode 失败: {}", e))?;
        // 兜底：超过 model_max_length 截断，防止 ONNX 位置越界
        if encoding.len() > MAX_ENCODER_TOKENS {
            log::warn!("opus-mt 输入 {} tokens 超上限 {}，截断", encoding.len(), MAX_ENCODER_TOKENS);
            encoding.truncate(MAX_ENCODER_TOKENS, 0, tokenizers::TruncationDirection::Right);
        }
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
            // 拷贝最后位置的 logits（需要修改副本做惩罚）
            let mut logits: Vec<f32> = logits_data[offset..offset + vocab_size].to_vec();

            // 1) Repetition penalty + 2) No-repeat-ngram —— 纯逻辑抽为函数，便于单测
            apply_penalties(&mut logits, &decoder_ids);

            let next_token = logits
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

        // 用实际 token 数判断是否需要分段
        let token_count = self.tokenizer.encode(text, true)
            .map(|enc| enc.len())
            .unwrap_or(MAX_ENCODER_TOKENS + 1);
        if token_count <= MAX_ENCODER_TOKENS {
            return self.translate_chunk(text);
        }

        log::info!("opus-mt 长文本分段：{} tokens", token_count);
        let sentences = crate::text_split::split_sentences(text);
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

        // 用空字符串拼接（保持段内连续），与 m2m100 一致
        // 上层 do_translate_streaming 已按 \n 切段逐段翻译，段内不需再加 \n
        Ok(results.join(""))
    }
}

#[async_trait::async_trait]
impl TranslationEngine for OpusMTEngine {
    fn name(&self) -> &str {
        "opus-mt"
    }

    async fn translate(&self, text: &str, _source_lang: &str, _target_lang: &str) -> Result<String> {
        // 规范化 CJK 邻接空格：opus-mt tokenizer (WhitespaceSplit + Metaspace) 对带空格
        // 中文会在句中产生独立 ▁ token，偏离训练分布（中文为连续字符）→ decoder 过早 EOS
        // → 译文截断为第一段（如「要看 猫…」只译出 "It depends."）。详见 normalize_cjk_spaces。
        let text = normalize_cjk_spaces(text);
        // Opus-MT 方向已由 load 时确定，source/target lang 不再需要
        self.split_and_translate(&text)
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
    let src = lang_prefix(source_lang);
    let tgt = lang_prefix(target_lang);
    let direction = format!("{}-{}", src, tgt);

    // DB source = "translate/opus-mt"，方向子目录 zh-en / en-zh
    let base = onnx_infra::resolve_model_dir("translate/opus-mt")?;
    let dir = base.join(&direction);
    if dir.is_dir() {
        return Ok((dir, src, tgt));
    }

    anyhow::bail!(
        "opus-mt 方向 '{}' 未找到模型目录。请在设置 > 模型管理 > 翻译模型 中下载，或手动放到 ~/.octopus/models/translate/opus-mt/{}/",
        direction, direction
    )
}

fn lang_prefix(lang: &str) -> String {
    lang.get(..2).unwrap_or(lang).to_lowercase()
}

/// 判断字符是否为 CJK 表意文字/假名/韩文（用于空格规范化）。
fn is_cjk_char(c: char) -> bool {
    matches!(c as u32,
        0x4e00..=0x9fff   // CJK 统一汉字
        | 0x3400..=0x4dbf // CJK 扩展 A
        | 0xf900..=0xfaff // CJK 兼容表意文字
        | 0x3040..=0x30ff // 日文假名（平/片假名）
        | 0xac00..=0xd7af // 韩文音节
    )
}

/// 规范化 CJK 邻接空格：移除「左侧或右侧为 CJK 字符」的 ASCII 半角空格。
///
/// 背景：opus-mt 的 tokenizer pre_tokenizer 是 `WhitespaceSplit + Metaspace`，
/// 对带空格的中文会在句中产生**独立的 `▁` token（id=7）**——而 opus-mt-zh-en
/// 训练数据中中文是连续字符（句中无 `▁`）。这种 OOD 输入会让 decoder 翻译完
/// 第一段（空格前）后过早输出 EOS，译文被截断（实测「要看 猫是主动咬…」→
/// "It depends."）。移除 CJK 邻接空格让 token 序列回到训练分布。
///
/// 语言无关、安全：纯英文输入无 CJK 字符，空格全保留；中英混合时仅移除
/// CJK 边界空格（如「使用 Python 编程」→「使用Python编程」），Latin 词内部
/// 空格保留。换行等其它空白不动（上层已按 \n 切段）。
fn normalize_cjk_spaces(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    for (i, &c) in chars.iter().enumerate() {
        if c == ' ' {
            let prev_cjk = i > 0 && is_cjk_char(chars[i - 1]);
            let next_cjk = i + 1 < chars.len() && is_cjk_char(chars[i + 1]);
            // 空格任一侧是 CJK → 移除（CJK 不靠空格分词）
            if prev_cjk || next_cjk {
                continue;
            }
        }
        out.push(c);
    }
    out
}

/// 对 decoder logits 施加 repetition penalty + no-repeat-ngram 惩罚（原地修改）。
/// 纯函数，不依赖 ONNX，便于单测。
/// - `logits`：当前步 logits 副本（vocab_size 长度）
/// - `decoder_ids`：含 decoder_start_id 的完整序列（index 0 = start_id）
fn apply_penalties(logits: &mut [f32], decoder_ids: &[i64]) {
    // 1) Repetition penalty：已生成 token（不含 decoder_start_id prompt）
    let generated: std::collections::HashSet<i64> =
        decoder_ids[1..].iter().copied().collect();
    for &tid in &generated {
        let idx = tid as usize;
        if idx < logits.len() {
            if logits[idx] > 0.0 {
                logits[idx] /= REPETITION_PENALTY;
            } else {
                logits[idx] *= REPETITION_PENALTY;
            }
        }
    }

    // 2) No-repeat-ngram-size：需历史中已存在完整 n-gram（len >= n）
    if decoder_ids.len() >= NO_REPEAT_NGRAM_SIZE {
        let n = NO_REPEAT_NGRAM_SIZE;
        let prefix_len = n - 1;
        let prefix = &decoder_ids[decoder_ids.len() - prefix_len..];
        for i in 0..decoder_ids.len().saturating_sub(prefix_len) {
            if decoder_ids[i..i + prefix_len] == *prefix {
                let banned_idx = decoder_ids[i + prefix_len] as usize;
                if banned_idx < logits.len() {
                    logits[banned_idx] = f32::NEG_INFINITY;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_cjk_spaces_removes_between_cjk() {
        // CJK 之间的空格移除（本 bug 的核心场景）
        assert_eq!(normalize_cjk_spaces("要看 猫是主动咬"), "要看猫是主动咬");
        assert_eq!(normalize_cjk_spaces("要 看 猫"), "要看猫");
    }

    #[test]
    fn normalize_cjk_spaces_preserves_latin() {
        // 纯英文空格全保留
        assert_eq!(normalize_cjk_spaces("hello world"), "hello world");
        assert_eq!(normalize_cjk_spaces("It depends."), "It depends.");
    }

    #[test]
    fn normalize_cjk_spaces_mixed() {
        // 中英混合：CJK 边界空格移除，Latin 词内部保留
        assert_eq!(normalize_cjk_spaces("使用 Python 编程"), "使用Python编程");
        assert_eq!(normalize_cjk_spaces("hello 世界"), "hello世界");
        assert_eq!(normalize_cjk_spaces("世界 hello"), "世界hello");
    }

    #[test]
    fn normalize_cjk_spaces_edges_and_empty() {
        // 首尾空格：邻接 CJK 移除
        assert_eq!(normalize_cjk_spaces(" 看"), "看");
        assert_eq!(normalize_cjk_spaces("看 "), "看");
        // 无空格 / 空串不变
        assert_eq!(normalize_cjk_spaces("要看猫"), "要看猫");
        assert_eq!(normalize_cjk_spaces(""), "");
    }

    #[test]
    fn test_apply_penalties_len1_no_crash() {
        // decoder_ids = [start_id]，len=1 → 不应 panic
        let mut logits = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        apply_penalties(&mut logits, &[100]);
        // len < NO_REPEAT_NGRAM_SIZE(3) → ngram 不触发，repetition 也无已生成 token
        assert_eq!(logits, vec![1.0, 2.0, 3.0, 4.0, 5.0]);
    }

    #[test]
    fn test_apply_penalties_len2_no_crash() {
        // decoder_ids = [start_id, token_a]，len=2 → 不应 panic
        let mut logits = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        apply_penalties(&mut logits, &[100, 1]);
        // len < 3 → ngram 不触发；repetition penalty 对 token 1 生效
        assert!(logits[1] < 2.0); // 2.0 / 1.3 ≈ 1.54
    }

    #[test]
    fn test_apply_penalties_len3_ngram_bans() {
        // decoder_ids = [start_id, 1, 2]，len=3 → ngram 触发
        // prefix_len=2，prefix = [1, 2]
        // 在 decoder_ids 中找 [1,2]：i=0 → [100,1]≠[1,2]；i=1 → [1,2]==[1,2] → ban decoder_ids[3]?
        // 不，decoder_ids.len()=3, saturating_sub(2)=1, 所以 i in 0..1 → i=0
        // decoder_ids[0..2] = [100, 1] ≠ [1, 2] → 不 ban
        let mut logits = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        apply_penalties(&mut logits, &[100, 1, 2]);
        // ngram 不 ban（前缀不匹配），repetition penalty 对 1 和 2 生效
        assert!(logits[1] < 2.0);
        assert!(logits[2] < 3.0);
    }

    #[test]
    fn test_apply_penalties_ngram_bans_repeated_pattern() {
        // 构造重复 n-gram：[start, 1, 2, 3, 1, 2]
        // 当前 prefix = [1, 2]（最后 2 个）
        // 历史中 i=0: [start,1]≠[1,2]; i=1: [1,2]==[1,2] → ban decoder_ids[3]=3
        // i=2: [2,3]≠[1,2]; i=3: [3,1]≠[1,2]
        let mut logits = vec![0.0; 10];
        logits[3] = 5.0; // token 3 有高 logit
        apply_penalties(&mut logits, &[100, 1, 2, 3, 1, 2]);
        // token 3 应被 ban（NEG_INFINITY）
        assert!(logits[3].is_infinite() && logits[3].is_sign_negative());
    }

    #[test]
    fn test_apply_penalties_repetition_penalty_only_positive() {
        // 正 logit → 除以 penalty（降低）
        let mut logits = vec![2.6];
        apply_penalties(&mut logits, &[100, 0]);
        assert!((logits[0] - 2.0).abs() < 0.01); // 2.6 / 1.3 = 2.0
    }

    #[test]
    fn test_apply_penalties_repetition_penalty_negative() {
        // 负 logit → 乘以 penalty（更负，也降低概率）
        let mut logits = vec![-1.0];
        apply_penalties(&mut logits, &[100, 0]);
        assert!((logits[0] - (-1.3)).abs() < 0.01); // -1.0 * 1.3 = -1.3
    }
}
