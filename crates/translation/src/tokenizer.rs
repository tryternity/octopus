use anyhow::{Context, Result};
use std::path::Path;
use sentencepiece_rs::SentencePieceProcessor;

pub struct M2M100Tokenizer {
    sp: SentencePieceProcessor,
}

pub const BOS_ID: i64 = 0;
pub const PAD_ID: i64 = 1;
pub const EOS_ID: i64 = 2;
pub const UNK_ID: i64 = 3;
pub const DECODER_START_TOKEN_ID: i64 = 2;

fn lang_code_to_token(lang: &str) -> Option<&'static str> {
    match lang {
        "zh" => Some("__zh__"),
        "en" => Some("__en__"),
        "ja" => Some("__ja__"),
        "ko" => Some("__ko__"),
        "fr" => Some("__fr__"),
        "de" => Some("__de__"),
        "es" => Some("__es__"),
        "ru" => Some("__ru__"),
        "it" => Some("__it__"),
        "pt" => Some("__pt__"),
        "ar" => Some("__ar__"),
        "th" => Some("__th__"),
        "vi" => Some("__vi__"),
        "id" => Some("__id__"),
        "tr" => Some("__tr__"),
        "nl" => Some("__nl__"),
        "pl" => Some("__pl__"),
        "uk" => Some("__uk__"),
        "hi" => Some("__hi__"),
        _ => None,
    }
}

impl M2M100Tokenizer {
    pub fn load(model_path: &Path) -> Result<Self> {
        let sp = SentencePieceProcessor::open(model_path)
            .context("加载 SentencePiece 模型失败")?;
        Ok(Self { sp })
    }

    pub fn encode(&self, text: &str, source_lang: &str) -> Result<Vec<i64>> {
        let lang_token = lang_code_to_token(source_lang).unwrap_or("__en__");
        let lang_id = self.sp.model().try_piece_to_id(lang_token).unwrap_or(UNK_ID as usize);
        let ids = self.sp.encode_to_ids(text).context("SentencePiece encode 失败")?;
        let mut result: Vec<i64> = vec![lang_id as i64];
        result.extend(ids.iter().map(|&id| id as i64));
        result.push(EOS_ID);
        Ok(result)
    }

    pub fn decode(&self, ids: &[i64]) -> Result<String> {
        let vocab_size = self.sp.model().vocab_size();
        let filtered: Vec<usize> = ids
            .iter()
            .filter(|&&id| id > UNK_ID && (id as usize) < vocab_size)
            .map(|&id| id as usize)
            .collect();
        let text = self.sp.decode_ids(&filtered)
            .context("SentencePiece decode_ids 失败")?;
        Ok(text.trim().to_string())
    }
}
