//! ASR 数字 ITN（Inverse Text Normalization）：中文数字→阿拉伯数字。
//!
//! 解决 Zipformer/Moonshine/Whisper 等无内置 ITN 引擎输出
//! 「二零二六年七月二十六日」而非「2026年7月26日」的问题。
//!
//! **单数字保护 + 黑名单**（2026-07-27）：
//! - `chinese2digits` crate 对所有含数字字符的文本激进提取，导致「统一→统1」。
//! - 改为只转连续 2+ 数字字符的片段——单个数字字符一律保留。
//!   「七月」「三月」「五天」这种单数字+量词是地道中文，不转更自然。
//! - 黑名单词（万一/千万/百万/十分...）即使含 2+ 数字字符也不转。
//!
//! 详见 spec 2026-07-27-asr-itn-design.md。

use chinese2digits::take_number_from_string;

/// 中文数字字符集（含幺，电信场景用）
const CN_DIGITS: &str = "零一二三四五六七八九十百千万亿兆点幺";

/// 黑名单——含 2+ 数字字符但不是数字的常用词/成语。
/// 只在**词边界**（前后非数字字符）匹配时保护——
/// 「二百五」独立出现不转，但「二百五十六」该转。
const BLACKLIST: &[&str] = &[
    "万一", "千万", "百万", "一点",
    "三十六计", "三百六十行", "二十四史", "二百五",
    "七十二变", "八十一难", "九九归一",
    "三七二十一", "八九不离十", "三五成群", "略知一二", "乱七八糟", "千百年",
    // 节日/纪念日——单独出现不转，连数字时转（「五四运动」不转，「五四三二一」转）
    "五四", "六一", "五一", "八一", "十一",
];

/// 中文数字→阿拉伯数字。始终应用，无开关。
///
/// 规则：
/// 1. 先用占位符保护黑名单词（万一/千万/百万/十分等不转）
/// 2. 扫描文本，对每个连续数字字符片段：
///    - 2+ 连续数字 → 调 chinese2digits 转换
///    - 单个数字字符 → 一律保留（「七月」「统一」都不转）
/// 3. 还原黑名单占位符
pub fn normalize(text: &str) -> String {
    // 1. 保护黑名单词（只在词边界匹配——前后非数字字符时）
    //
    // 优化（#10）：旧实现每匹配一个黑名单词就重 collect 全文 chars（O(N)）+ 线性扫描
    // （O(N)），K 处匹配 → O(N·K)。改为单次遍历：用 str::find 迭代收集所有匹配的
    // 字节区间，校验边界后统一从后向前 replace_range 替换（避免索引偏移 + 重复 collect）。
    let is_digit = |c: char| CN_DIGITS.contains(c);
    let mut protected = text.to_string();
    let mut placeholders: Vec<(String, String)> = Vec::new();

    for (idx, word) in BLACKLIST.iter().enumerate() {
        let word_byte_len = word.len();
        if word_byte_len == 0 {
            continue;
        }
        // 收集所有匹配的字节区间（含边界校验）。
        let mut ranges: Vec<(usize, usize)> = Vec::new();
        let mut search_from = 0usize;
        while search_from <= protected.len() {
            // str::find 返回字节位置（保证是 UTF-8 边界，因 word 是 &str 子串）
            match protected[search_from..].find(word) {
                None => break,
                Some(rel_start) => {
                    let byte_start = search_from + rel_start;
                    let byte_end = byte_start + word_byte_len;
                    // 边界校验：前一字符与后一字符不得为数字（与原实现语义一致）
                    let prev_ok = byte_start == 0
                        || !is_digit(
                            protected[..byte_start].chars().next_back().unwrap_or('\0'),
                        );
                    let next_ok = byte_end >= protected.len()
                        || !is_digit(protected[byte_end..].chars().next().unwrap_or('\0'));
                    if prev_ok && next_ok {
                        ranges.push((byte_start, byte_end));
                    }
                    // 无论是否采用，都越过本次匹配继续找（避免重叠死循环）
                    search_from = byte_end;
                }
            }
        }
        // 从后向前替换（字节索引不变，避免偏移）
        for &(start, _end) in ranges.iter().rev() {
            // 占位符含 NUL（\u{0000}，单字节），不与中文/数字冲突；idx 标识黑名单词序号，
            // start 标识本次匹配的字节位置（同 idx 多匹配时位置唯一）。
            let ph = format!("\u{0000}B{idx}P{start}\u{0000}");
            protected.replace_range(start..start + word_byte_len, &ph);
            placeholders.push((ph, word.to_string()));
        }
    }

    // 2. 扫描：只转连续 2+ 数字字符的片段
    let is_digit = |c: char| CN_DIGITS.contains(c);
    let chars: Vec<char> = protected.chars().collect();
    let mut result = String::new();
    let mut i = 0;

    while i < chars.len() {
        if is_digit(chars[i]) {
            let start = i;
            while i < chars.len() && is_digit(chars[i]) {
                i += 1;
            }
            let seq: String = chars[start..i].iter().collect();

            if seq.chars().count() >= 2 {
                // 2+ 连续数字 → 转换
                let converted = take_number_from_string(&seq, false, true);
                result.push_str(&converted.replaced_text);
            } else {
                // 单个数字字符 → 保留（「七月」「统一」都不转）
                result.push_str(&seq);
            }
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }

    // 3. 还原黑名单
    for (ph, word) in &placeholders {
        result = result.replace(ph, word);
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn year_and_date() {
        // 二零二六（4 连续）→ 2026；七月（单数字）→ 保留七月；二十六（2 连续）→ 26
        assert_eq!(normalize("二零二六年七月二十六日"), "2026年七月26日");
    }

    #[test]
    fn decimal() {
        assert_eq!(normalize("三点五"), "3.5");
    }

    #[test]
    fn count() {
        assert_eq!(normalize("十五个"), "15个");
    }

    #[test]
    fn single_digit_preserved() {
        // 单个数字字符一律保留——「七月」「三个」「五天」是地道中文
        assert_eq!(normalize("三个苹果"), "三个苹果");
        assert_eq!(normalize("七月"), "七月");
        assert_eq!(normalize("五天"), "五天");
    }

    #[test]
    fn no_chinese_number_noop() {
        assert_eq!(normalize("今天天气不错"), "今天天气不错");
    }

    #[test]
    fn already_arabic_noop() {
        assert_eq!(normalize("2026年7月26日"), "2026年7月26日");
    }

    #[test]
    fn english_noop() {
        assert_eq!(normalize("hello world"), "hello world");
    }

    // ── 单数字保护（杜绝「统一→统1」）──

    #[test]
    fn single_digit_in_word_not_converted() {
        assert_eq!(normalize("统一"), "统一");
        assert_eq!(normalize("一些"), "一些");
        assert_eq!(normalize("唯一"), "唯一");
        assert_eq!(normalize("同一"), "同一");
        assert_eq!(normalize("一起"), "一起");
        assert_eq!(normalize("一直"), "一直");
    }

    // ── 黑名单保护（含 2+ 数字字符但不是数字的词）──

    #[test]
    fn blacklist_words_not_converted() {
        assert_eq!(normalize("万一"), "万一");
        assert_eq!(normalize("千万"), "千万");
        assert_eq!(normalize("百万"), "百万");
        assert_eq!(normalize("三十六计"), "三十六计");
        assert_eq!(normalize("三百六十行"), "三百六十行");
        assert_eq!(normalize("二十四史"), "二十四史");
        assert_eq!(normalize("七十二变"), "七十二变");
        assert_eq!(normalize("八十一难"), "八十一难");
    }

    #[test]
    fn blacklist_boundary_check() {
        // 「二百五」独立出现不转，但跟其他数字连用时该转
        assert_eq!(normalize("二百五"), "二百五");       // 独立 → 保护
        assert_eq!(normalize("二百五十六"), "256");      // 连数字 → 转
        assert_eq!(normalize("他是个二百五"), "他是个二百五"); // 句中独立 → 保护
        // 节日/纪念日同规则
        assert_eq!(normalize("五四运动"), "五四运动");   // 独立 → 保护
        assert_eq!(normalize("六一儿童节"), "六一儿童节");
        assert_eq!(normalize("五一假期"), "五一假期");
        assert_eq!(normalize("八一建军节"), "八一建军节");
        assert_eq!(normalize("十一国庆"), "十一国庆");
    }

    /// 回归 #10：同一黑名单词在文本中出现多次时，全部应被保护（旧实现每匹配一处
    /// 重 collect 全文，可能漏掉或索引错乱）。单次扫描收集所有匹配后统一替换。
    #[test]
    fn blacklist_multiple_matches_all_protected() {
        // 「万一」出现 3 次（前后非数字字符隔开）→ 全部保护，中间 2+ 连续数字正常转。
        assert_eq!(
            normalize("万一来不及，万一出错，万一有二百五十六块"),
            "万一来不及，万一出错，万一有256块"
        );
        // 多个不同黑名单词（前后非数字）混排 → 全部保护
        assert_eq!(normalize("万一咋办，千万小心"), "万一咋办，千万小心");
        // 长文本多次匹配（前后非数字隔开，验证不退化 / 不丢匹配）。
        // 注：「万一」直接相邻时彼此互为数字边界，全部不保护（与原实现一致），
        // 故用非数字分隔。
        let long = (0..50).map(|_| "万一，").collect::<String>();
        let expected = long.clone();
        assert_eq!(normalize(&long), expected, "50 个「万一，」应全部保护");
    }

    // ── 混合文本 ──

    #[test]
    fn mixed_text_with_numbers_and_words() {
        assert_eq!(normalize("唯一的十五个苹果"), "唯一的15个苹果");
        assert_eq!(normalize("统一的三百六十五天"), "统一的365天");
        assert_eq!(normalize("万一丢了十五个"), "万一丢了15个");
        assert_eq!(normalize("七月二十一日"), "七月21日");
        // 成语在句中 + 真正数字混合
        assert_eq!(normalize("三十六计走为上"), "三十六计走为上");
        assert_eq!(normalize("花了二百五买了十五个"), "花了二百五买了15个");
    }


}

