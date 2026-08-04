//! 编译期内联资源统一入口。
//!
//! 2026-08-04 集中化：DB schema / 字典 / 模型 / prompt 从各 crate 散落的
//! `include_bytes!`/`include_str!` 集中到 `infra/resources/`，消除跨 crate `../../`
//! 脆弱路径。
//!
//! crate 专有资源（desktop icon/i18n/tauri.conf、pty shell 脚本）保留原位，
//! 调用方用 `env!("CARGO_MANIFEST_DIR")` 消除 `../../`——本模块不为其提供 API。

// ── SQL ──────────────────────────────────────────────────────────

/// SQLite schema（含表结构 + 短种子；长 seed 走 seeds.rs 运行时加载）。
pub const fn db_schema_sql() -> &'static str {
    include_str!("../resources/sql/schema.sql")
}

// ── 字典 / 词表 ──────────────────────────────────────────────────

/// Mozilla Public Suffix List（vault 域名匹配用）。
/// 季度级同步：curl -o crates/infra/resources/dicts/public_suffix_list.dat \
///   https://publicsuffix.org/list/public_suffix_list.dat
pub const fn public_suffix_list() -> &'static [u8] {
    include_bytes!("../resources/dicts/public_suffix_list.dat")
}

/// OCR 常用词表（ocr 引擎识别后纠错用）。
pub const fn ocr_words_common() -> &'static str {
    include_str!("../resources/dicts/words_common.txt")
}

/// 简繁转换：简体→繁体映射表。
pub const fn hans_t2s() -> &'static str {
    include_str!("../resources/dicts/t2s.txt")
}

/// 简繁转换：繁体→简体映射表。
pub const fn hans_s2t() -> &'static str {
    include_str!("../resources/dicts/s2t.txt")
}

/// ASR 文本纠错 unigram（gzip 压缩，运行时解压）。
pub const fn corrector_unigram_gz() -> &'static [u8] {
    include_bytes!("../resources/dicts/unigram.txt.gz")
}

// ── 模型 ─────────────────────────────────────────────────────────

/// Silero VAD v6 ONNX 模型（语音端点检测）。
/// 用户可在 ~/.octopus/models/vad.onnx 放自定义版本覆盖（见 asr vad.rs）。
pub const fn silero_vad_v6_onnx() -> &'static [u8] {
    include_bytes!("../resources/models/vad/silero_vad_v6.onnx")
}

// ── Prompt ───────────────────────────────────────────────────────

/// 热词挖掘 LLM prompt（从用户编辑文本提取热词候选）。
pub const fn hotword_mine_prompt() -> &'static str {
    include_str!("../resources/prompts/hotword_mine.md")
}
