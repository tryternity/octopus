use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use octopus_infra::db::FuzzyDialectRule;
use pinyin::ToPinyin;

/// 全局方言规则缓存（仅 enabled 规则，从 DB 读出）。
/// `reload_fuzzy_dialect` 时替换；`normalize_fuzzy_pinyin` 读它。
static FUZZY_RULES_CACHE: OnceLock<parking_lot::RwLock<Vec<FuzzyDialectRule>>> = OnceLock::new();

fn fuzzy_cache() -> &'static parking_lot::RwLock<Vec<FuzzyDialectRule>> {
    FUZZY_RULES_CACHE.get_or_init(|| parking_lot::RwLock::new(Vec::new()))
}

/// 更新全局方言规则缓存（reload_fuzzy_dialect 调用，传入 DB 读出的 enabled 规则）。
/// 变更后**必须** reload 热词索引——索引 key 由 [`normalize_fuzzy_pinyin`] 生成，
/// 规则变则 key 必变，旧索引与新查询规则不一致会漏命中。
pub fn set_fuzzy_rules_cache(rules: Vec<FuzzyDialectRule>) {
    *fuzzy_cache().write() = rules;
}

/// 归一化模糊拼音（读全局方言规则缓存）。索引构建与查询共用此函数 → 双向对称命中。
pub fn normalize_fuzzy_pinyin(py: &str) -> String {
    normalize_with_rules(py, &fuzzy_cache().read())
}

/// 归一化逻辑（纯函数，便于单测无全局污染）。
///
/// 顺序：基础规则（平翘舌 zh/ch/sh→z/c/s + 前后鼻音 ing/eng/ang→in/en/an，始终开）
/// → syllable 组（整音节精确匹配 `== from_py`）→ initial 组（声母前缀 `starts_with(from_py)`）
/// → special_hu 组（hu→wu + huX→wX，硬编码）。一个字只归一组（flag 互斥）。
///
/// rules 须按 DB 的 (match_type, sort_order) 排序——syllable 在 initial 前避免 fei 被 fh 抢。
fn normalize_with_rules(py: &str, rules: &[FuzzyDialectRule]) -> String {
    let mut n = py.to_lowercase();
    // 基础规则（始终开）：平翘舌
    if n.starts_with("zh") {
        n = n.replacen("zh", "z", 1);
    } else if n.starts_with("ch") {
        n = n.replacen("ch", "c", 1);
    } else if n.starts_with("sh") {
        n = n.replacen("sh", "s", 1);
    }
    // 基础规则（始终开）：前后鼻音
    if n.ends_with("ing") {
        n = n[..n.len() - 3].to_string() + "in";
    } else if n.ends_with("eng") {
        n = n[..n.len() - 3].to_string() + "en";
    } else if n.ends_with("ang") {
        n = n[..n.len() - 3].to_string() + "an";
    }
    // 方言规则：按 match_type 分组遍历（rules 已按 match_type+sort_order 排序）。
    // 一个字只归一组（syllable 命中则跳过 initial/special_hu）。
    let mut matched = false;
    // syllable 组（整音节精确）
    for r in rules {
        if r.match_type != "syllable" { continue; }
        if n == r.from_py {
            n = r.to_py.clone();
            matched = true;
            break;
        }
    }
    // initial 组（声母前缀）
    if !matched {
        for r in rules {
            if r.match_type != "initial" { continue; }
            if n.starts_with(&r.from_py) {
                n = format!("{}{}", r.to_py, &n[r.from_py.len()..]);
                matched = true;
                break;
            }
        }
    }
    // special_hu 组（hu→wu + huX→wX，硬编码——非单字符替换）
    if !matched {
        for r in rules {
            if r.match_type != "special_hu" { continue; }
            if n == "hu" {
                n = "wu".to_string();
                break;
            } else if n.starts_with("hu") {
                n = format!("w{}", &n[2..]);
                break;
            }
        }
    }
    n
}

/// 单字 → 归一化模糊拼音；非汉字（无拼音）返回 None。
pub fn char_fuzzy_pinyin(c: char) -> Option<String> {
    c.to_pinyin().map(|p| normalize_fuzzy_pinyin(p.plain()))
}

/// 单字 → 原始拼音（不经归一化）；非汉字返回 None。测试 + DB 写入用。
pub fn char_raw_pinyin(c: char) -> Option<String> {
    c.to_pinyin().map(|p| p.plain().to_string())
}

/// 词 → 原始拼音空格分隔（每字 char_raw_pinyin，非汉字跳过）。
/// 与 infra `word_plain_pinyins` 等价（asr-local 测试用，避免跨 crate 依赖）。
pub fn word_raw_pinyin(word: &str) -> String {
    word.chars().filter_map(char_raw_pinyin).collect::<Vec<_>>().join(" ")
}

/// 词 → 拼音首字母串（大写，非汉字跳过）。实现搬至 `octopus_infra::hotword_text`
/// （infra 为底层，db.rs 迁移/写 words_text 需复用，避免循环依赖）。
pub use octopus_infra::hotword_text::pinyin_initials;

/// 热词的内存索引：按「字数 → 归一化拼音 → 候选词列表」分组。
/// 纠错热路径按窗口字数与拼音 O(1) 查表。
/// 候选词携带 hit_count（correct 多命中时按 hit_count 降序排序，确定性）。
pub struct HotwordIndex {
    by_len_py: HashMap<usize, HashMap<String, Vec<(String, i64)>>>,
    active_words: HashSet<String>,
}

impl HotwordIndex {
    pub fn empty() -> Self {
        Self { by_len_py: HashMap::new(), active_words: HashSet::new() }
    }

    /// entries = [(word, raw_pinyin, hit_count)]，raw_pinyin 是 DB 存的原始拼音
    /// （空格分隔 "ba zhao yu"，不经归一化）。from_words 跳过 to_pinyin 现算，
    /// 只做 normalize_fuzzy_pinyin 生成 key（方言规则运行时生效）。
    /// 单字热词忽略（歧义太大）；含非汉字的热词忽略（拼音数 ≠ 字数）。
    pub fn from_words(entries: &[(String, String, i64)]) -> Self {
        let mut by_len_py: HashMap<usize, HashMap<String, Vec<(String, i64)>>> = HashMap::new();
        let mut active_words = HashSet::new();
        for (w, raw_pinyin, hit_count) in entries {
            let chars: Vec<char> = w.chars().collect();
            let len = chars.len();
            if len < 2 { continue; }
            // DB 原始拼音 split → 逐字 normalize_fuzzy_pinyin（跳过 to_pinyin 查表）
            let py: Vec<String> = raw_pinyin
                .split_whitespace()
                .map(|p| normalize_fuzzy_pinyin(p))
                .collect();
            if py.len() != len { continue; } // 含非汉字 → 跳过
            let key = py.join("-");
            by_len_py.entry(len).or_default().entry(key).or_default().push((w.clone(), *hit_count));
            active_words.insert(w.clone());
        }
        Self { by_len_py, active_words }
    }

    pub fn is_empty(&self) -> bool { self.active_words.is_empty() }

    pub fn max_len(&self) -> usize { *self.by_len_py.keys().max().unwrap_or(&0) }

    pub fn lookup(&self, len: usize, py: &str) -> Option<&Vec<(String, i64)>> {
        self.by_len_py.get(&len)?.get(py)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一条方言规则（测试 helper）。
    fn rule(token: &str, from: &str, to: &str, match_type: &str, sort_order: i64) -> FuzzyDialectRule {
        FuzzyDialectRule {
            token: token.to_string(),
            label: token.to_string(),
            from_py: from.to_string(),
            to_py: to.to_string(),
            match_type: match_type.to_string(),
            enabled: true,
            sort_order,
        }
    }

    /// 单测用纯函数（不触碰全局缓存，避免并发测试互相污染）。
    fn norm(py: &str, rules: &[FuzzyDialectRule]) -> String {
        normalize_with_rules(py, rules)
    }

    #[test]
    fn empty_index_is_empty() {
        let idx = HotwordIndex::from_words(&[]);
        assert!(idx.is_empty());
        assert_eq!(idx.max_len(), 0);
        assert!(idx.lookup(3, "ba-zhua-yu").is_none());
    }

    #[test]
    fn groups_by_length_and_pinyin() {
        let idx = HotwordIndex::from_words(&[
            ("八爪鱼".to_string(), word_raw_pinyin("八爪鱼"), 0),   // → "ba-zao-yu", len 3
            ("巴掌鱼".to_string(), word_raw_pinyin("巴掌鱼"), 0),   // → "ba-zan-yu", len 3
            ("吴大锐".to_string(), word_raw_pinyin("吴大锐"), 0),   // → "wu-da-rui", len 3
        ]);
        assert!(!idx.is_empty());
        assert_eq!(idx.max_len(), 3);
        // 归一化拼音 lookup：爪 zhao→zao（平翘舌归一）
        assert!(idx.lookup(3, "ba-zao-yu").is_some());
        // 模糊：掌 zhang→zan 归一后能查到「巴掌鱼」
        assert!(idx.lookup(3, "ba-zan-yu").is_some());
        // 不存在的拼音
        assert!(idx.lookup(3, "xxx-yyy-zzz").is_none());
    }

    #[test]
    fn fuzzy_pinyin_normalizes_accents() {
        // 卫 wei（无归一），生 sheng→sen（sh→s, eng→en）
        assert_eq!(char_fuzzy_pinyin('卫'), Some("wei".to_string()));
        assert_eq!(char_fuzzy_pinyin('生'), Some("sen".to_string()));
        assert_eq!(char_fuzzy_pinyin('A'), None); // 非汉字
    }

    // ── normalize_with_rules 方言组（DB 驱动规则注入）──

    #[test]
    fn normalize_fh_dialect() {
        let rules = [rule("f/h", "f", "h", "initial", 2)];
        // f→h：浮 fu→hu，护 hu→hu，双向归一相同
        assert_eq!(norm("fu", &rules), "hu");
        assert_eq!(norm("hu", &rules), "hu");
    }

    #[test]
    fn normalize_nl_dialect() {
        let rules = [rule("n/l", "n", "l", "initial", 1)];
        assert_eq!(norm("niu", &rules), "liu");
        assert_eq!(norm("liu", &rules), "liu");
    }

    #[test]
    fn normalize_rl_dialect() {
        let rules = [rule("r/l", "r", "l", "initial", 3)];
        // r→l：热 re→le，乐 le→le，双向归一相同（第一字可救）
        assert_eq!(norm("re", &rules), "le");
        assert_eq!(norm("le", &rules), "le");
        assert_eq!(norm("rou", &rules), "lou"); // 肉
        assert_eq!(norm("ren", &rules), "len"); // 人
    }

    // ── yun/yong + fei/hui 整音节归一（各自独立）──

    #[test]
    fn normalize_yun_yong_mapping() {
        let rules = [rule("yun/yong", "yun", "yong", "syllable", 2)];
        // yun→yong：孕→用（「孕妇」yong-hu 命中「用户」yong-hu）
        assert_eq!(norm("yun", &rules), "yong");
        assert_eq!(norm("yong", &rules), "yong"); // 目标端不变，双向对称
        // yun/yong 不影响 fei
        assert_eq!(norm("fei", &rules), "fei");
    }

    #[test]
    fn normalize_fei_hui_mapping() {
        let rules = [rule("fei/hui", "fei", "hui", "syllable", 1)];
        // fei→hui：飞→回
        assert_eq!(norm("fei", &rules), "hui");
        assert_eq!(norm("hui", &rules), "hui");
        // fei/hui 不影响 yun
        assert_eq!(norm("yun", &rules), "yun");
    }

    #[test]
    fn normalize_si_ci_collapse() {
        // si→ci：四/词收口（平翘舌 + 齿音）。
        // 关键：基础规则先把 shi→si、chi→ci，故这一条 si→ci 让 si/shi/chi/ci 四者全收口到 ci。
        let rules = [rule("si/ci", "si", "ci", "syllable", 3)];
        assert_eq!(norm("si", &rules), "ci");   // 四 → 词
        assert_eq!(norm("shi", &rules), "ci");  // 时：基础 shi→si，再 si→ci
        assert_eq!(norm("ci", &rules), "ci");   // 词：目标端不变
        assert_eq!(norm("chi", &rules), "ci");  // 吃：基础 chi→ci
        // 不应误伤其他 si/ci 开头的音节（syllable 是精确匹配，se/ce/san/can 不受影响）
        assert_eq!(norm("se", &rules), "se");
        assert_eq!(norm("ce", &rules), "ce");
        assert_eq!(norm("san", &rules), "san");
    }

    #[test]
    fn no_rules_no_dialect_normalization() {
        // 空规则：yun/fei 不归一（仅基础规则）
        assert_eq!(norm("yun", &[]), "yun");
        assert_eq!(norm("fei", &[]), "fei");
    }

    /// 核心场景：yun/yong + f/h 叠加——「孕妇」(yun-fu) 经 yun→yong + fu→hu = yong-hu = 「用户」(yong-hu)。
    /// syllable 组（yun→yong）与 initial 组（fu→hu）可同时作用在不同字上（按字独立归一）。
    /// 注意：HotwordIndex::from_words 读全局 normalize 缓存，须持 corrector 串行锁避免并发污染。
    #[test]
    fn yun_yong_overlaps_with_fh() {
        let _g = crate::text::corrector::test_serial(); // 串行（HotwordIndex 读全局）
        // 设全局缓存为这两条规则（与 norm 的 rules 一致）
        crate::hotword::set_fuzzy_rules_cache(vec![
            rule("yun/yong", "yun", "yong", "syllable", 2),
            rule("f/h", "f", "h", "initial", 2),
        ]);
        // 模拟 DB 读出的顺序（ORDER BY match_type，仅为展示确定性，不影响归一正确性）
        let rules = [
            rule("yun/yong", "yun", "yong", "syllable", 2),
            rule("f/h", "f", "h", "initial", 2),
        ];
        // 单字验证：yun→yong（syllable 组），fu→hu（initial 组）
        assert_eq!(norm("yun", &rules), "yong");
        assert_eq!(norm("fu", &rules), "hu");
        // 整词等价：「孕妇」yong-hu = 「用户」yong-hu（HotwordIndex 用全局缓存归一）
        let idx = HotwordIndex::from_words(&[("用户".to_string(), word_raw_pinyin("用户"), 0)]);
        let query_py = [norm("yun", &rules), norm("fu", &rules)].join("-");
        assert_eq!(query_py, "yong-hu");
        assert!(idx.lookup(2, &query_py).is_some(), "孕妇归一后应命中用户");
    }

    #[test]
    fn normalize_nl_rl_both() {
        // nl + rl 同开：n 与 r 首字母不同，互不干扰，都归一到 l
        let rules = [rule("n/l", "n", "l", "initial", 1), rule("r/l", "r", "l", "initial", 3)];
        assert_eq!(norm("re", &rules), "le");
        assert_eq!(norm("niu", &rules), "liu");
        assert_eq!(norm("le", &rules), "le");
    }

    #[test]
    fn normalize_hw_dialect() {
        let rules = [rule("hu/wu", "hu", "w", "special_hu", 1)];
        // 单字 hu→wu（胡/无）
        assert_eq!(norm("hu", &rules), "wu");
        assert_eq!(norm("wu", &rules), "wu");
        // 多字 huX→wX：huang 先基础 ang→an 得 huan，再 hu→w 得 wan；
        // wang 基础 ang→an 得 wan——两者归一相同（双向命中）。
        assert_eq!(norm("huang", &rules), "wan");
        assert_eq!(norm("wang", &rules), "wan");
        assert_eq!(norm("hua", &rules), "wa");
    }

    #[test]
    fn normalize_base_rules_always_on_dialect_off() {
        // 无方言规则：基础规则（平翘舌 + 前后鼻音）仍生效
        assert_eq!(norm("zhao", &[]), "zao"); // zh→z
        assert_eq!(norm("sheng", &[]), "sen"); // sh→s + eng→en
        // 无方言规则：f/n/hu 不归一
        assert_eq!(norm("fu", &[]), "fu");
        assert_eq!(norm("niu", &[]), "niu");
        assert_eq!(norm("hu", &[]), "hu");
        // huang 仅基础 ang→an → huan
        assert_eq!(norm("huang", &[]), "huan");
    }

    #[test]
    fn normalize_fh_nl_hw_combine() {
        // 声母 + special_hu 同时开：一个拼音只归一组（matched flag 阻断后续）；
        // 关键：fu 经 fh→hu 后**不**被 hw 二次转 wu（matched flag 阻断）。
        let rules = [
            rule("n/l", "n", "l", "initial", 1),
            rule("f/h", "f", "h", "initial", 2),
            rule("r/l", "r", "l", "initial", 3),
            rule("hu/wu", "hu", "w", "special_hu", 1),
        ];
        assert_eq!(norm("fu", &rules), "hu"); // fh（不被 hw 二次捕获）
        assert_eq!(norm("niu", &rules), "liu"); // nl
        assert_eq!(norm("re", &rules), "le"); // rl
        assert_eq!(norm("huang", &rules), "wan"); // hw（基础 ang→an 后 hu→w）
    }

    /// 回归：syllable 组（整音节精确）优先于 initial 组（声母前缀），无论 rules 数组顺序。
    /// 关键场景：fei/hui（syllable）+ f/h（initial）同开——fei 必须走 fei/hui 归一到 hui，
    /// 不能被 f/h 抢成 hei。实现按 match_type 分组遍历（syllable → initial → special_hu）
    /// + matched flag 互斥，与 rules 传入顺序无关（DB 排序只为输出确定性，不影响正确性）。
    #[test]
    fn syllable_beats_initial_regardless_of_array_order() {
        // ① syllable 在 initial 前（DB ORDER BY match_type 的自然序）
        let rules_sorted = [
            rule("fei/hui", "fei", "hui", "syllable", 1),
            rule("f/h", "f", "h", "initial", 2),
        ];
        assert_eq!(norm("fei", &rules_sorted), "hui"); // 走 fei/hui，不被 f/h 抢成 hei
        assert_eq!(norm("fu", &rules_sorted), "hu"); // 非 syllable 命中 → initial 生效
        assert_eq!(norm("hui", &rules_sorted), "hui"); // 目标端不变

        // ② initial 在 syllable 前（打乱顺序）——分组遍历保证结果不变
        let rules_shuffled = [
            rule("f/h", "f", "h", "initial", 2),
            rule("fei/hui", "fei", "hui", "syllable", 1),
        ];
        assert_eq!(norm("fei", &rules_shuffled), "hui", "数组顺序不影响：syllable 分组必先于 initial");
        assert_eq!(norm("fu", &rules_shuffled), "hu");
        assert_eq!(norm("hui", &rules_shuffled), "hui");
    }

    /// 回归：special_hu 不抢先于 syllable 命中的整音节。
    /// hu（单字）若同时配了某 syllable 规则，syllable 先命中则 special_hu 不再作用。
    #[test]
    fn syllable_beats_special_hu() {
        let rules = [
            rule("fei/hui", "fei", "hui", "syllable", 1),
            rule("hu/wu", "hu", "w", "special_hu", 1),
        ];
        // fei → syllable 命中 hui，不走 special_hu（fei 也不匹配 hu 前缀）
        assert_eq!(norm("fei", &rules), "hui");
        // hu 不匹配 fei（syllable 未命中）→ special_hu 生效 → wu
        assert_eq!(norm("hu", &rules), "wu");
    }
}
