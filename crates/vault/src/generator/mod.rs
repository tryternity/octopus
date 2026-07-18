//! 密码生成器：Random / PassphraseEn / PassphraseZh / PIN。

pub mod pin;
pub mod random;
pub mod passphrase_en;
pub mod passphrase_zh;
pub mod eff_wordlist;
pub mod zh_wordlist_4096;

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "camelCase")]
pub enum GeneratorConfig {
    Random(RandomConfig),
    PassphraseEn(PassphraseEnConfig),
    PassphraseZh(PassphraseZhConfig),
    Pin(PinConfig),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RandomConfig {
    #[serde(default = "default_length_16")]
    pub length: u32,
    #[serde(default = "default_true")]
    pub uppercase: bool,
    #[serde(default = "default_true")]
    pub lowercase: bool,
    #[serde(default = "default_true")]
    pub numbers: bool,
    #[serde(default = "default_false")]
    pub symbols: bool,
    #[serde(default = "default_true")]
    pub avoid_ambiguous: bool,
}

impl Default for RandomConfig {
    fn default() -> Self {
        Self {
            length: 16,
            uppercase: true,
            lowercase: true,
            numbers: true,
            symbols: false,
            avoid_ambiguous: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PinConfig {
    #[serde(default = "default_length_6")]
    pub length: u32,
}

impl Default for PinConfig {
    fn default() -> Self {
        Self { length: 6 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PassphraseEnConfig {
    #[serde(default = "default_length_3")]
    pub word_count: u32,
    #[serde(default = "default_sep_dash")]
    pub separator: String,
    #[serde(default = "default_true")]
    pub capitalize: bool,
    #[serde(default = "default_true")]
    pub include_number: bool,
}

impl Default for PassphraseEnConfig {
    fn default() -> Self {
        Self {
            word_count: 3,
            separator: "-".into(),
            capitalize: true,
            include_number: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PassphraseZhConfig {
    #[serde(default = "default_length_4")]
    pub word_count: u32,
    #[serde(default = "default_sep_empty")]
    pub separator: String,
    #[serde(default = "default_true")]
    pub include_number: bool,
    #[serde(default = "default_false")]
    pub include_symbol: bool,
}

impl Default for PassphraseZhConfig {
    fn default() -> Self {
        Self {
            word_count: 4,
            separator: "".into(),
            include_number: true,
            include_symbol: false,
        }
    }
}

pub fn generate(cfg: &GeneratorConfig) -> Result<String> {
    match cfg {
        GeneratorConfig::Random(c) => random::generate(c),
        GeneratorConfig::PassphraseEn(c) => passphrase_en::generate(c),
        GeneratorConfig::PassphraseZh(c) => passphrase_zh::generate(c),
        GeneratorConfig::Pin(c) => pin::generate(c),
    }
}

fn default_length_16() -> u32 {
    16
}
fn default_length_6() -> u32 {
    6
}
fn default_length_3() -> u32 {
    3
}
fn default_length_4() -> u32 {
    4
}
fn default_true() -> bool {
    true
}
fn default_false() -> bool {
    false
}
fn default_sep_dash() -> String {
    "-".into()
}
fn default_sep_empty() -> String {
    "".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 防止前端/Rust serde 模式标签失配（详见 final-review C1）。
    /// 前端 PasswordGenerator.tsx 用 camelCase (`passphraseEn` / `passphraseZh`)，
    /// 因此 `rename_all = "camelCase"` 后变体应序列化为对应标签。
    #[test]
    fn test_generator_config_serde_modes() {
        // PassphraseZh
        let cfg = GeneratorConfig::PassphraseZh(PassphraseZhConfig::default());
        let json = serde_json::to_string(&cfg).unwrap();
        assert!(
            json.contains(r#""mode":"passphraseZh""#),
            "actual: {}",
            json
        );
        let parsed: GeneratorConfig = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, GeneratorConfig::PassphraseZh(_)));

        // PassphraseEn
        let cfg = GeneratorConfig::PassphraseEn(PassphraseEnConfig::default());
        let json = serde_json::to_string(&cfg).unwrap();
        assert!(
            json.contains(r#""mode":"passphraseEn""#),
            "actual: {}",
            json
        );
        let parsed: GeneratorConfig = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, GeneratorConfig::PassphraseEn(_)));

        // Random
        let cfg = GeneratorConfig::Random(RandomConfig::default());
        let json = serde_json::to_string(&cfg).unwrap();
        assert!(json.contains(r#""mode":"random""#), "actual: {}", json);
        let parsed: GeneratorConfig = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, GeneratorConfig::Random(_)));

        // Pin
        let cfg = GeneratorConfig::Pin(PinConfig::default());
        let json = serde_json::to_string(&cfg).unwrap();
        assert!(json.contains(r#""mode":"pin""#), "actual: {}", json);
        let parsed: GeneratorConfig = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, GeneratorConfig::Pin(_)));

        // 反序列化前端实际发出的 payload（camelCase 标签 + 扁平字段）
        let frontend_payload = r#"{"mode":"passphraseZh","word_count":4,"separator":"","include_number":true,"include_symbol":false}"#;
        let parsed: GeneratorConfig = serde_json::from_str(frontend_payload)
            .expect("前端 passphraseZh payload 必须可反序列化");
        assert!(matches!(parsed, GeneratorConfig::PassphraseZh(_)));

        let frontend_payload = r#"{"mode":"passphraseEn","word_count":3,"separator":"-","capitalize":true,"include_number":true}"#;
        let parsed: GeneratorConfig = serde_json::from_str(frontend_payload)
            .expect("前端 passphraseEn payload 必须可反序列化");
        assert!(matches!(parsed, GeneratorConfig::PassphraseEn(_)));
    }
}
