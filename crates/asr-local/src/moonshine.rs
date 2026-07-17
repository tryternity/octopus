use std::collections::HashMap;
use parking_lot::Mutex;

use anyhow::{Context, Result};
use ort::session::Session;
use ort::value::TensorRef;

use crate::config;

/// Moonshine ASR 引擎 — 纯 ONNX 体系，4 session 流水线。
///
/// 模型来自 `csukuangfj/sherpa-onnx-moonshine-{base,tiny}-en-int8`（v1 格式）。
/// 推理流程：preprocess → encode → uncached_decode（首 token，初始化 KV cache）
///           → cached_decode 循环（后续 token，复用 KV cache）→ EOS 停止。
///
/// Decode 循环逻辑参考 sherpa-onnx `offline-moonshine-greedy-search-decoder.cc`：
/// BOS(1) → uncached_decode → logits + N 个 KV cache（层数×2，base 模型=32）
/// 循环: argmax → EOS(2) 则停 → cached_decode(token, cache) → logits + 新 cache
pub struct MoonshineEngine {
    preprocess_session: Mutex<Session>,
    encode_session: Mutex<Session>,
    uncached_decode_session: Mutex<Session>,
    cached_decode_session: Mutex<Session>,
    vocab: Vec<String>,
}

impl MoonshineEngine {
    pub fn new(entry: &config::ModelEntry) -> Result<Self> {
        let model_dir = config::resolve_model_dir(&entry.source)
            .context("解析 Moonshine 模型目录失败")?;

        let preprocess_path = model_dir.join("preprocess.onnx");
        let encode_path = model_dir.join("encode.int8.onnx");
        let uncached_path = model_dir.join("uncached_decode.int8.onnx");
        let cached_path = model_dir.join("cached_decode.int8.onnx");

        for (name, p) in [
            ("preprocess", &preprocess_path),
            ("encode", &encode_path),
            ("uncached_decode", &uncached_path),
            ("cached_decode", &cached_path),
        ] {
            if !p.exists() {
                anyhow::bail!("Moonshine {} 未找到: {}", name, p.display());
            }
        }

        let make_session = |path: &std::path::Path| -> Result<Session> {
            Ok(config::apply_session_acceleration(Session::builder()?)?
                .commit_from_file(path)?)
        };

        let vocab = load_tokens(&model_dir.join("tokens.txt"))?;
        // Moonshine 设计 vocab=32768（byte-level BPE，tiny/base 一致）。不强制校验——
        // 未来微调/变体词表可能变化；argmax 的 vocab_size 取自 logits 维度（shape[2]）
        // 而非此硬编码，且 decode 对越界 id 有 `id < vocab.len()` 保护，能自适应。

        Ok(Self {
            preprocess_session: Mutex::new(make_session(&preprocess_path)?),
            encode_session: Mutex::new(make_session(&encode_path)?),
            uncached_decode_session: Mutex::new(make_session(&uncached_path)?),
            cached_decode_session: Mutex::new(make_session(&cached_path)?),
            vocab,
        })
    }

    /// 运行 preprocess：audio (1, N) → features (1, T, 416)。
    /// 返回 owned `Value`（不拷贝到 CPU）+ features_len(T)，后续 run_encode 以 &Value 传回。
    fn run_preprocess(&self, samples: &[f32]) -> Result<(ort::value::Value, usize)> {
        let audio = ndarray::ArrayView2::from_shape((1, samples.len()), samples)?;
        let mut session = self.preprocess_session.lock();
        let outputs = session.run(ort::inputs! {
            "args_0" => TensorRef::from_array_view(audio)?
        })?;
        // 消费 SessionOutputs 取 features 为 owned Value，不经 CPU to_vec。
        let out: Vec<ort::value::Value> =
            outputs.into_iter().map(|(_, v)| v).collect();
        let features = out.into_iter().next().context("preprocess 输出为空")?;
        // features shape (1, T, 416)，取 T 作为 features_len。
        let features_len = {
            let (shape, _data) = features.try_extract_tensor::<f32>()?;
            anyhow::ensure!(shape.len() >= 2, "preprocess 输出维度异常: {:?}", shape);
            shape[1] as usize
        };
        Ok((features, features_len))
    }

    /// 运行 encode：features (1, T, 416) + len → encoder_out (1, T, 416)。
    /// features 与返回值均为 owned `Value`（不拷贝到 CPU）——preprocess → encode → decode 全链路零拷贝。
    fn run_encode(
        &self,
        features: &ort::value::Value,
        features_len: usize,
    ) -> Result<ort::value::Value> {
        let len_arr = [features_len as i32];
        let len_view = ndarray::ArrayView1::from(&len_arr);
        let mut session = self.encode_session.lock();
        let outputs = session.run(ort::inputs! {
            "args_0" => features,
            "args_1" => TensorRef::from_array_view(len_view)?
        })?;
        // 消费 SessionOutputs 取 encoder_out 为 owned Value（[0]），不经 CPU to_vec。
        // 先 collect 成 Vec<Value> 脱离 session 借用——SessionOutputs<'_> 持 session 引用，
        // 而 owned Value 内部 Arc、'static，collect 后即可跨 session 生命周期返回。
        let out: Vec<ort::value::Value> =
            outputs.into_iter().map(|(_, v)| v).collect();
        out.into_iter().next().context("encode 输出为空")
    }

    /// Greedy decode 循环。参考 sherpa-onnx `offline-moonshine-greedy-search-decoder.cc`。
    fn greedy_decode(
        &self,
        encoder_out: &ort::value::Value,
        features_len: i32,
    ) -> Result<Vec<i64>> {
        const BOS: i32 = 1;
        const EOS: i64 = 2;
        // 与 sherpa-onNX 一致：encoder_frames * 384 / 16000 * 6
        // +20 安全余量：极短音频（如 1s 指令）的 BPE 切分可能超出 6 token/秒，
        // 无余量会导致末尾字被强行截断。
        let max_len = (features_len as f32 * 384.0 / 16000.0 * 6.0) as usize + 20;

        // ── 首 token（BOS）: uncached_decode ──
        let token = [BOS];
        let token_view = ndarray::ArrayView2::from_shape((1, 1), &token)?;
        let seq_len = [1i32];
        let seq_len_view = ndarray::ArrayView1::from(&seq_len);

        let mut uncached_session = self.uncached_decode_session.lock();
        let uncached_out = uncached_session.run(ort::inputs! {
            "args_0" => TensorRef::from_array_view(token_view)?,
            "args_1" => encoder_out,
            "args_2" => TensorRef::from_array_view(seq_len_view)?
        })?;

        // logits (index 0) + N KV caches (index 1.., N=层数×2，base 模型=32)
        let num_caches = uncached_out.len().saturating_sub(1);
        // vocab_size 固定（= logits 末维），argmax 仅限此范围。
        let vocab_size = {
            let (shape, _data) = uncached_out[0].try_extract_tensor::<f32>()?;
            anyhow::ensure!(shape.len() >= 3, "uncached_decode logits 维度异常: {:?}", shape);
            shape[2] as usize
        };

        // logits + KV cache 复用：消费 uncached 输出为 owned Value 列表（[0]=logits，[1..]=N cache），
        // 后续步骤直接以 ValueView（O(1) Arc 引用计数）传回 ONNX Runtime，张量全程留在 ORT 内部
        // ——消除原先每步 `to_vec()` 深拷贝 logits（vocab×4B=128KB/步）与 N 个随 seq_len 增长 cache
        // （base=32，长音频每步可达 MB 级）的开销。参考 sherpa-onnx greedy-search-decoder 直接复用 OrtValue。
        let mut state_values: Vec<ort::value::Value> =
            uncached_out.into_iter().map(|(_, v)| v).collect();

        // ── 后续 tokens: cached_decode 循环 ──
        let mut result_tokens: Vec<i64> = Vec::new();
        let mut seq_len_val: i32 = 1;
        let mut cached_session = self.cached_decode_session.lock();

        // 预算 cache 输入键名（"args_3".."args_{N+2}"），循环内以 Cow::Borrowed 复用，
        // 避免每步 format! + 堆分配 N 个 String（base=32，长音频循环数百次）。
        let cache_keys: Vec<String> = (3..num_caches + 3).map(|i| format!("args_{i}")).collect();

        for _ in 0..max_len {
            // argmax 直接在当前步 logits（state_values[0]）的零拷贝 &[f32] 上做，不 to_vec()。
            // 块作用域释放借用，之后才能 view/消费 state_values。
            let next_token = {
                let (_, logits) = state_values[0].try_extract_tensor::<f32>()?;
                logits[..vocab_size]
                    .iter()
                    .enumerate()
                    .max_by(|(_, a), (_, b)| {
                        a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .map(|(i, _)| i as i64)
                    .unwrap_or(EOS)
            };

            if next_token == EOS {
                break;
            }
            result_tokens.push(next_token);
            seq_len_val += 1;

            let token = [next_token as i32];
            let token_view = ndarray::ArrayView2::from_shape((1, 1), &token)?;
            let seq_len = [seq_len_val];
            let seq_len_view = ndarray::ArrayView1::from(&seq_len);

            let mut inputs = ort::inputs! {
                "args_0" => TensorRef::from_array_view(token_view)?,
                "args_1" => encoder_out,
                "args_2" => TensorRef::from_array_view(seq_len_view)?
            };
            // KV cache 直接以 ValueView 传回（Arc 引用计数，O(1)），不经 CPU 深拷贝；
            // 键名复用预算的 cache_keys（Cow::Borrowed，零分配）。
            for i in 0..num_caches {
                inputs.push((
                    std::borrow::Cow::Borrowed(cache_keys[i].as_str()),
                    state_values[i + 1].view().into(),
                ));
            }

            // 消费 cached_out 替换 state_values（新 logits=[0]，新 cache=[1..]）。
            let cached_out = cached_session.run(inputs)?;
            state_values = cached_out.into_iter().map(|(_, v)| v).collect();
        }

        Ok(result_tokens)
    }
}

impl crate::engine::OfflineAsrEngine for MoonshineEngine {
    // moonshine 是 en-only：corrector 跳过由 transcribe_with_vad 基于 language=en 自动处理
    //（desktop=config.language、CLI=--language、server=请求，对 en-only 模型即 en），
    // 无需在此覆盖 skip_corrector()（后者仅用于 qwen3 等「自带纠错」的非语言原因）。

    fn transcribe(&self, samples: &[f32], _language: &str) -> Result<String> {
        if samples.is_empty() {
            return Ok(String::new());
        }
        let (features, features_len) = self.run_preprocess(samples)?;
        if features_len == 0 {
            return Ok(String::new());
        }
        let encoder_out = self.run_encode(&features, features_len)?;
        let token_ids = self.greedy_decode(&encoder_out, features_len as i32)?;
        Ok(decode_moonshine_tokens(&token_ids, &self.vocab))
    }
}

/// 加载 tokens.txt：每行 "token_text\ttoken_id"，按 id 索引构建 vocab。
fn load_tokens(path: &std::path::Path) -> Result<Vec<String>> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("读取 tokens.txt 失败: {}", path.display()))?;
    let mut vocab: HashMap<i64, String> = HashMap::new();
    for line in content.lines() {
        let parts: Vec<&str> = line.rsplitn(2, '\t').collect();
        if parts.len() == 2 {
            let token_id: i64 = parts[0]
                .parse()
                .with_context(|| format!("tokens.txt 无效 token id: {}", parts[0]))?;
            // 防御：负 id 经 `as usize` 会 wrapping 成巨大下标，致 result[] 越界 panic。
            // 标准 tokens.txt 无负 id，此处仅兜底损坏/自造词表，给出清晰加载错误而非崩溃。
            anyhow::ensure!(
                token_id >= 0,
                "tokens.txt 出现负 token id {}: {}",
                token_id,
                path.display()
            );
            vocab.insert(token_id, parts[1].to_string());
        }
    }
    let max_id = vocab.keys().copied().max().unwrap_or(-1);
    let mut result = vec![String::new(); (max_id + 1) as usize];
    for (id, text) in vocab {
        result[id as usize] = text;
    }
    Ok(result)
}

/// Moonshine byte-level BPE 解码：直接拼接 vocab[token_id]，再将 SentencePiece
/// 空格标记 ▁ (U+2581) 替换为空格。
fn decode_moonshine_tokens(token_ids: &[i64], vocab: &[String]) -> String {
    let mut text = String::new();
    for &id in token_ids {
        let id = id as usize;
        if id >= vocab.len() {
            continue;
        }
        let token = &vocab[id];
        // 跳过特殊控制 token（<unk>/<s>/</s>/<pad> 等）：以 '<' 开头且以 '>' 结尾。
        // greedy_decode 已过滤 BOS/EOS 不进结果序列；此处兜底 <unk> 及其他特殊标记，
        // 避免噪声/空白音频时模型误输出特殊 token 被当文本拼接。byte-level BPE 普通
        // token 不会呈完整 "<...>" 形式（'<' 可单独出现但不被 '>' 包裹），判断安全。
        if token.starts_with('<') && token.ends_with('>') {
            continue;
        }
        text.push_str(token);
    }
    text.replace('\u{2581}', " ").trim_start().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::OfflineAsrEngine;

    /// 验证 tokens.txt 解析：vocab 大小 32768，特殊 token 在正确位置。
    #[test]
    fn test_load_tokens() {
        let cfg = config::load_config().expect("load_config 失败");
        let entry = match config::pick_entry(&cfg, config::EngineCategory::Moonshine, "moonshine-base-en") {
            Some(e) => e,
            None => {
                eprintln!("[SKIP] moonshine-base-en 不在 DB 中 — 跳过 load_tokens 测试");
                return;
            }
        };
        let model_dir = config::resolve_model_dir(&entry.source).expect("resolve_model_dir 失败");
        let vocab = load_tokens(&model_dir.join("tokens.txt")).expect("load_tokens 失败");
        assert_eq!(vocab.len(), 32768, "vocab 大小应为 32768");
        assert_eq!(vocab[0], "<unk>", "token 0 应为 <unk>");
        assert_eq!(vocab[1], "<s>", "token 1 应为 <s> (BOS)");
        assert_eq!(vocab[2], "</s>", "token 2 应为 </s> (EOS)");
    }

    /// 真实模型端到端测试：加载 Moonshine base 模型，识别 test_wavs 中的 wav 文件。
    #[test]
    fn test_moonshine_base_real_model() {
        let cfg = config::load_config().expect("load_config 失败");
        let entry = match config::pick_entry(&cfg, config::EngineCategory::Moonshine, "moonshine-base-en") {
            Some(e) => e,
            None => {
                eprintln!("[SKIP] moonshine-base-en 不在 DB 中 — 跳过真实模型测试");
                return;
            }
        };
        let engine = match MoonshineEngine::new(entry) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("[SKIP] MoonshineEngine::new 失败（可能 HF 缓存未就绪）: {e}");
                return;
            }
        };

        let model_dir = config::resolve_model_dir(&entry.source).expect("resolve_model_dir 失败");
        let test_wav_dir = model_dir.join("test_wavs");
        if !test_wav_dir.exists() {
            eprintln!("[SKIP] 无 test_wavs 目录");
            return;
        }

        let mut any_tested = false;
        let entries: Vec<_> = std::fs::read_dir(&test_wav_dir)
            .expect("read_dir 失败")
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "wav"))
            .collect();

        for path in entries {
            let path_str = path.to_str().expect("路径转 str 失败");
            let samples = match crate::audio::read_wav_16k(path_str) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("[WARN] 读取 {:?} 失败: {e}", path.file_name());
                    continue;
                }
            };
            let text = engine.transcribe(&samples, "en").expect("transcribe 失败");
            println!("[Moonshine] {:?} => {:?}", path.file_name().unwrap(), text);
            assert!(!text.is_empty(), "识别结果不应为空: {:?}", path.file_name());
            any_tested = true;
        }
        assert!(any_tested, "应至少测试一个 wav 文件");
    }
}
