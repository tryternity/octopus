//! 简繁体字形转换（单字级"愚能"转换）。
//!
//! 基于「开放词典网」(kaifangcidian.com) 繁简对照表，CC-BY 3.0 授权（见 `data/NOTICE`）。
//! 仅转字形、不转地域用词（如「電腦→电脑」而非「计算机」），适合 ASR 输出归一化。
//!
//! 数据编译期嵌入（`include_str!`），零运行时文件依赖；由 `config.output_simplified`
//! 控制方向：`true`→输出简体（繁→简），`false`→输出繁体（简→繁）。
//! 简→繁一对多时取数据中的首选（数据已消歧，如「发→發」）。
//!
//! 动机：Qwen3-ASR 在 `language=auto` 下不强制简体，输出会混入繁体；sherpa #3509 显示
//! `language` 参数不可靠，故在 ASR 输出后做字形归一化（保持 auto 多语言优势）。

use crate::config::load_app_config_cached;
use once_cell::sync::OnceCell;
use std::collections::HashMap;

/// 繁→简单字对照表（编译期嵌入）。
const T2S_DATA: &str = include_str!("../data/t2s.txt");
/// 简→繁单字对照表（编译期嵌入）。
const S2T_DATA: &str = include_str!("../data/s2t.txt");

fn t2s_map() -> &'static HashMap<char, char> {
    static MAP: OnceCell<HashMap<char, char>> = OnceCell::new();
    MAP.get_or_init(|| parse_map(T2S_DATA))
}

fn s2t_map() -> &'static HashMap<char, char> {
    static MAP: OnceCell<HashMap<char, char>> = OnceCell::new();
    MAP.get_or_init(|| parse_map(S2T_DATA))
}

/// 解析 `键\t值` 单字对照表为 `HashMap`。
/// 仅收录单字行（跳过词组条目）；重复 key 取首个（简→繁数据已消歧）。
fn parse_map(data: &str) -> HashMap<char, char> {
    let mut m = HashMap::new();
    for line in data.lines() {
        let mut parts = line.split('\t');
        if let (Some(k), Some(v)) = (parts.next(), parts.next()) {
            // 仅收单字行（key 与 value 各 1 个 char）
            if k.chars().count() == 1 && v.chars().count() == 1 {
                let kc = k.chars().next().unwrap();
                let vc = v.chars().next().unwrap();
                m.entry(kc).or_insert(vc);
            }
        }
    }
    m
}

/// 繁→简（单字级）。未命中的字符（已是简体/非中文）原样保留。
pub fn to_simplified(text: &str) -> String {
    let map = t2s_map();
    text.chars().map(|c| *map.get(&c).unwrap_or(&c)).collect()
}

/// 简→繁（单字级）。未命中的字符（已是繁体/非中文）原样保留。
pub fn to_traditional(text: &str) -> String {
    let map = s2t_map();
    text.chars().map(|c| *map.get(&c).unwrap_or(&c)).collect()
}

/// 按 `config.output_simplified` 归一化 ASR 输出：`true`→简体，`false`→繁体。
/// 在 ASR 输出边界（offline `transcribe_with_vad` / streaming `finish`）调用。
pub fn normalize_variant(text: &str) -> String {
    if load_app_config_cached().output_simplified {
        to_simplified(text)
    } else {
        to_traditional(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn t2s_first_entry() {
        // t2s.txt 首行：丟→丢
        assert_eq!(to_simplified("丟"), "丢");
    }

    #[test]
    fn s2t_first_entry() {
        // s2t.txt 首行：专→專
        assert_eq!(to_traditional("专"), "專");
    }

    #[test]
    fn t2s_common_phrase() {
        // 常见繁体短语 → 简
        assert_eq!(to_simplified("語言識別"), "语言识别");
        assert_eq!(to_simplified("電腦"), "电脑");
    }

    #[test]
    fn s2t_common_phrase() {
        assert_eq!(to_traditional("语言识别"), "語言識別");
    }

    #[test]
    fn preserves_length_and_non_cjk() {
        let inp = "Hello 語言 123";
        let out = to_simplified(inp);
        assert_eq!(out.chars().count(), inp.chars().count());
        assert!(out.contains("Hello") && out.contains("123"));
    }

    #[test]
    fn missing_char_unchanged() {
        // 已是简体/无繁体源 → 不变
        assert_eq!(to_simplified("你好"), "你好");
    }

    #[test]
    fn roundtrip_simplified_via_traditional() {
        // 简→繁→简 往返：常用字应稳定（一对多可能偶发偏差，此处取稳定字）
        let s = "语言识别电脑";
        assert_eq!(to_simplified(&to_traditional(s)), s);
    }
}
