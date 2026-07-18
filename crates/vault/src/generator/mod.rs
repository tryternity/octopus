//! 密码生成器：Random / PassphraseEn / PassphraseZh / PIN。

pub mod pin;
pub mod random;
pub mod passphrase_en;
pub mod passphrase_zh;
pub mod eff_wordlist;
pub mod zh_wordlist_4096;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
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

pub fn generate(cfg: &GeneratorConfig) -> String {
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
