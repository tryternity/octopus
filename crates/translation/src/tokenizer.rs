use anyhow::{Context, Result};
use std::path::Path;
use tokenizers::Tokenizer;

pub struct M2M100Tokenizer {
    tok: Tokenizer,
}

pub const EOS_ID: i64 = 2;
pub const DECODER_START_TOKEN_ID: i64 = 2;

/// m2m100 tokenizer.json 中未知语言的 fallback token id（仅用于错误诊断，不应进入正常翻译流程）
pub const FALLBACK_LANG_ID: u32 = 128022;

/// 语言标记 token IDs（from tokenizer.json, m2m100 standard layout）
pub fn lang_code_to_id(lang: &str, tok: &Tokenizer) -> Option<u32> {
    let prefix = lang.get(..2).unwrap_or(lang).to_lowercase();
    let token = match prefix.as_str() {
        "zh" => "__zh__",
        "en" => "__en__",
        "ja" => "__ja__",
        "ko" => "__ko__",
        "fr" => "__fr__",
        "de" => "__de__",
        "es" => "__es__",
        "ru" => "__ru__",
        "it" => "__it__",
        "pt" => "__pt__",
        "ar" => "__ar__",
        "th" => "__th__",
        "vi" => "__vi__",
        "id" => "__id__",
        "tr" => "__tr__",
        "nl" => "__nl__",
        "pl" => "__pl__",
        "uk" => "__uk__",
        "hi" => "__hi__",
        _ => return None,
    };
    tok.token_to_id(token)
}

impl M2M100Tokenizer {
    pub fn load(tokenizer_path: &Path) -> Result<Self> {
        let tok = Tokenizer::from_file(tokenizer_path)
            .map_err(|e| anyhow::anyhow!("加载 tokenizer.json 失败: {}", e))?;
        Ok(Self { tok })
    }

    /// 编码：text → token ids。
    /// m2m100 格式：[source_lang_id] + text_tokens + [eos]
    /// tokenizer.json 的特殊 token 添加行为不一致，手动构建序列。
    pub fn encode(&self, text: &str, source_lang: &str) -> Result<Vec<i64>> {
        let encoding = self.tok.encode(text, false)
            .map_err(|e| anyhow::anyhow!("tokenizer encode 失败: {}", e))?;
        let text_ids: Vec<i64> = encoding.get_ids().iter().map(|&id| id as i64).collect();
        let lang_id = lang_code_to_id(source_lang, &self.tok)
            .with_context(|| format!("不支持的语言代码: {}", source_lang))? as i64;
        // [source_lang_id] + text_tokens + [eos]
        let mut result = vec![lang_id];
        result.extend(text_ids);
        result.push(EOS_ID);
        Ok(result)
    }

    /// 解码：token ids → text。过滤特殊 token 和语言标记。
    pub fn decode(&self, ids: &[i64]) -> Result<String> {
        let u32_ids: Vec<u32> = ids.iter().map(|&id| id as u32).collect();
        let text = self.tok.decode(&u32_ids, true)
            .map_err(|e| anyhow::anyhow!("tokenizer decode 失败: {}", e))?;
        Ok(text.trim().to_string())
    }

    pub fn tokenizer(&self) -> &Tokenizer {
        &self.tok
    }
}
