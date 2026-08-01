use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use pinyin::ToPinyin;

/// 方言模糊规则开关——由 `app_config.fuzzy_dialect`（逗号分隔 token：`f/h`/`hu/wu`/`n/l`/`r/l`）
/// 经 [`parse_dialect`] 解析而来。
///
/// **基础规则（平翘舌 zh/ch/sh→z/c/s + 前后鼻音 ing/eng/ang→in/en/an）始终开启**，
/// 不在此处控制——它们是跨方言的常见识别容错。
///
/// 六组方言混淆按需启用（归一化单向，索引与查询共用 [`normalize_fuzzy_pinyin`] → 双向对称命中）：
/// - `fh`（f/h 不分）：声母 f→h
/// - `nl`（n/l 不分）：声母 n→l
/// - `rl`（r/l 不分）：声母 r→l（n、r、l 在 nl+rl 同开时都归一到 l，互不冲突）
/// - `hw`（hu/wu 不分）：单字 hu→wu，其余 huX→wX（huang→wang、hua→wa）；
///   **不覆盖** hui↔wei（韵母 ui/ei 不同，拼音级无法统一）
/// - `yun_yong`（yun/yong）：整音节归一 yun→yong（孕/用）。解决「孕妇」→「用户」
///   （yun-fu vs yong-hu，配合 fh）等误识。
/// - `fei_hui`（fei/hui）：整音节归一 fei→hui（飞/回）。
///
/// yun_yong / fei_hui 是整音节（含声母+韵母），匹配精确——不像声母规则那样影响所有同声母字。
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct FuzzyRules {
    /// f/h 不分：声母 f→h
    pub fh: bool,
    /// n/l 不分：声母 n→l
    pub nl: bool,
    /// r/l 不分：声母 r→l
    pub rl: bool,
    /// hu/wu 不分：单字 hu→wu，其余 huX→wX
    pub hw: bool,
    /// yun/yong 整音节归一（孕/用）
    pub yun_yong: bool,
    /// fei/hui 整音节归一（飞/回）
    pub fei_hui: bool,
}

static FUZZY_RULES: OnceLock<parking_lot::RwLock<FuzzyRules>> = OnceLock::new();

fn fuzzy_store() -> &'static parking_lot::RwLock<FuzzyRules> {
    FUZZY_RULES.get_or_init(|| parking_lot::RwLock::new(FuzzyRules::default()))
}

/// 更新全局方言规则（启动装载 / set_config 调用）。
/// 变更后**必须** reload 热词索引——索引 key 由 [`normalize_fuzzy_pinyin`] 生成，
/// 规则变则 key 必变，旧索引与新查询规则不一致会漏命中。
pub fn set_fuzzy_rules(r: FuzzyRules) {
    *fuzzy_store().write() = r;
}

/// 解析 `fuzzy_dialect` 配置串（逗号分隔 token）→ [`FuzzyRules`]。
/// token：`f/h`→fh、`hu/wu`→hw、`n/l`→nl、`r/l`→rl、`yun/yong`→yun_yong、`fei/hui`→fei_hui；
/// 空白与未知 token 忽略（前向兼容）。
pub fn parse_dialect(s: &str) -> FuzzyRules {
    let mut r = FuzzyRules::default();
    for tok in s.split(',').map(|t| t.trim().to_lowercase()) {
        if tok.is_empty() {
            continue;
        }
        match tok.as_str() {
            "f/h" => r.fh = true,
            "hu/wu" => r.hw = true,
            "n/l" => r.nl = true,
            "r/l" => r.rl = true,
            "yun/yong" => r.yun_yong = true,
            "fei/hui" => r.fei_hui = true,
            _ => {} // 未知 token 忽略（前向兼容未来扩展）
        }
    }
    r
}

/// 归一化模糊拼音（读全局方言规则）。索引构建与查询共用此函数 → 双向对称命中。
/// 实际逻辑见 [`normalize_with_rules`]。
pub fn normalize_fuzzy_pinyin(py: &str) -> String {
    normalize_with_rules(py, &fuzzy_store().read())
}

/// 整音节归一规则表（精确匹配 `n == from`）。一个字只归一组（首个命中即替换）。
/// 添加新规则：往此表加一行 `(from, to, |r| r.xxx)`，FuzzyRules 加字段 + parse_dialect 加 token。
/// fei 在声母 f/h 前（fei 首字母 f，精确匹配优先于声母 starts_with）。
const SYLLABLE_RULES: &[(&str, &str, fn(&FuzzyRules) -> bool)] = &[
    ("fei", "hui", |r| r.fei_hui),
    ("yun", "yong", |r| r.yun_yong),
];

/// 声母归一规则表（首字母 `from`→`to`）。一个字只归一组（首个命中即替换）。
/// 添加新规则：往此表加一行 `(from, to, |r| r.xxx)`。
const INITIAL_RULES: &[(char, char, fn(&FuzzyRules) -> bool)] = &[
    ('n', 'l', |r| r.nl),
    ('f', 'h', |r| r.fh),
    ('r', 'l', |r| r.rl),
];

/// 归一化逻辑（纯函数，便于单测无全局污染）。
/// 顺序：基础规则（平翘舌 + 前后鼻音，始终开）→ 整音节归一表 → 声母归一表 → hu/wu（特殊）。
fn normalize_with_rules(py: &str, rules: &FuzzyRules) -> String {
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
    // 整音节归一（精确匹配，一个字只归一组）
    let mut syllable_matched = false;
    for &(from, to, enabled) in SYLLABLE_RULES {
        if enabled(rules) && n == from {
            n = to.to_string();
            syllable_matched = true;
            break;
        }
    }
    // 声母归一（首字母替换，一个字只归一组；整音节已命中则跳过）
    let mut initial_matched = syllable_matched;
    if !initial_matched {
        for &(from, to, enabled) in INITIAL_RULES {
            if enabled(rules) && n.starts_with(from) {
                n = format!("{}{}", to, &n[1..]);
                initial_matched = true;
                break;
            }
        }
    }
    // hu/wu（特殊：单字 hu→wu 整音节，其余 huX→wX 前缀；非单字符替换，不适合声母表）
    // 仅在前两组未命中时跑——防 fh 把 fu→hu 后被 hw 二次转 wu（一个字只归一组）
    if !initial_matched && rules.hw {
        if n == "hu" {
            n = "wu".to_string();
        } else if n.starts_with("hu") {
            n = format!("w{}", &n[2..]); // huang→(基础)huan→wan、hua→wa
        }
    }
    n
}

/// 单字 → 归一化模糊拼音；非汉字（无拼音）返回 None。
pub fn char_fuzzy_pinyin(c: char) -> Option<String> {
    c.to_pinyin().map(|p| normalize_fuzzy_pinyin(p.plain()))
}

/// 词 → 拼音首字母串（大写，非汉字跳过）。实现搬至 `octopus_infra::hotword_text`
/// （infra 为底层，db.rs 迁移/写 words_text 需复用，避免循环依赖）。
pub use octopus_infra::hotword_text::pinyin_initials;

/// 热词的内存索引：按「字数 → 归一化拼音 → 候选词列表」分组。
/// 纠错热路径按窗口字数与拼音 O(1) 查表。
pub struct HotwordIndex {
    by_len_py: HashMap<usize, HashMap<String, Vec<String>>>,
    active_words: HashSet<String>,
}

impl HotwordIndex {
    pub fn empty() -> Self {
        Self { by_len_py: HashMap::new(), active_words: HashSet::new() }
    }

    /// words 为 active 热词文本列表（来自 DB list_active_hotword_words）。
    /// 单字热词忽略（歧义太大）；含非汉字的热词忽略（拼音数 ≠ 字数）。
    pub fn from_words(words: &[String]) -> Self {
        let mut by_len_py: HashMap<usize, HashMap<String, Vec<String>>> = HashMap::new();
        let mut active_words = HashSet::new();
        for w in words {
            let chars: Vec<char> = w.chars().collect();
            let len = chars.len();
            if len < 2 { continue; }
            let py: Vec<String> = chars.iter().filter_map(|&c| char_fuzzy_pinyin(c)).collect();
            if py.len() != len { continue; } // 含非汉字 → 跳过
            let key = py.join("-");
            by_len_py.entry(len).or_default().entry(key).or_default().push(w.clone());
            active_words.insert(w.clone());
        }
        Self { by_len_py, active_words }
    }

    pub fn is_empty(&self) -> bool { self.active_words.is_empty() }

    pub fn max_len(&self) -> usize { *self.by_len_py.keys().max().unwrap_or(&0) }

    pub fn lookup(&self, len: usize, py: &str) -> Option<&Vec<String>> {
        self.by_len_py.get(&len)?.get(py)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 单测用纯函数包装（不触碰全局 FUZZY_RULES，避免并发测试互相污染）。
    fn norm(py: &str, r: FuzzyRules) -> String {
        normalize_with_rules(py, &r)
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
            "八爪鱼".to_string(),   // 八(ba) 爪(zhao→zao) 鱼(yu) → "ba-zao-yu", len 3
            "巴掌鱼".to_string(),   // 巴(ba) 掌(zhang→zan) 鱼(yu) → "ba-zan-yu", len 3
            "吴大锐".to_string(),   // 吴(wu) 大(da) 锐(rui) → "wu-da-rui", len 3
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

    // ── 方言规则 parse_dialect ──

    #[test]
    fn parse_dialect_maps_known_tokens() {
        let r = parse_dialect("f/h,hu/wu,n/l,r/l");
        assert!(r.fh && r.hw && r.nl && r.rl);
    }

    #[test]
    fn parse_dialect_single_and_empty() {
        let r = parse_dialect("f/h");
        assert!(r.fh && !r.nl && !r.rl && !r.hw);
        let r = parse_dialect("r/l");
        assert!(r.rl && !r.fh && !r.nl && !r.hw);
        let r = parse_dialect("");
        assert!(!r.fh && !r.nl && !r.rl && !r.hw);
    }

    #[test]
    fn parse_dialect_ignores_unknown_and_whitespace() {
        // 空白 trim + 未知 token 忽略
        let r = parse_dialect(" f/h , xx , n/l ");
        assert!(r.fh && r.nl && !r.hw);
    }

    // ── normalize_with_rules 方言组 ──

    #[test]
    fn normalize_fh_dialect() {
        let r = FuzzyRules { fh: true, ..Default::default() };
        // f→h：浮 fu→hu，护 hu→hu，双向归一相同
        assert_eq!(norm("fu", r), "hu");
        assert_eq!(norm("hu", r), "hu");
    }

    #[test]
    fn normalize_nl_dialect() {
        let r = FuzzyRules { nl: true, ..Default::default() };
        assert_eq!(norm("niu", r), "liu");
        assert_eq!(norm("liu", r), "liu");
    }

    #[test]
    fn normalize_rl_dialect() {
        let r = FuzzyRules { rl: true, ..Default::default() };
        // r→l：热 re→le，乐 le→le，双向归一相同（第一字可救）
        assert_eq!(norm("re", r), "le");
        assert_eq!(norm("le", r), "le");
        assert_eq!(norm("rou", r), "lou"); // 肉
        assert_eq!(norm("ren", r), "len"); // 人
    }

    // ── yun_yong / fei_hui 整音节归一（各自独立开关）──

    #[test]
    fn normalize_yun_yong_mapping() {
        let r = FuzzyRules { yun_yong: true, ..Default::default() };
        // yun→yong：孕→用（「孕妇」yong-hu 命中「用户」yong-hu）
        assert_eq!(norm("yun", r), "yong");
        assert_eq!(norm("yong", r), "yong"); // 目标端不变，双向对称
        // yun_yong 不影响 fei（fei 归 fei_hui 管）
        assert_eq!(norm("fei", r), "fei");
    }

    #[test]
    fn normalize_fei_hui_mapping() {
        let r = FuzzyRules { fei_hui: true, ..Default::default() };
        // fei→hui：飞→回
        assert_eq!(norm("fei", r), "hui");
        assert_eq!(norm("hui", r), "hui");
        // fei_hui 不影响 yun
        assert_eq!(norm("yun", r), "yun");
    }

    #[test]
    fn yun_yong_fei_hui_off_by_default() {
        // 默认关：yun/fei 不归一
        assert_eq!(norm("yun", FuzzyRules::default()), "yun");
        assert_eq!(norm("fei", FuzzyRules::default()), "fei");
    }

    /// 核心场景：yun_yong + fh 叠加——「孕妇」(yun-fu) 经 yun→yong + fu→hu = yong-hu = 「用户」(yong-hu)。
    /// yun_yong 整音节归一在声母组之前，可与 fh 同时作用。
    #[test]
    fn yun_yong_overlaps_with_fh() {
        let r = FuzzyRules { yun_yong: true, fh: true, ..Default::default() };
        // 单字验证：yun→yong（整音节归一），fu→hu（fh 声母组）
        assert_eq!(norm("yun", r), "yong");
        assert_eq!(norm("fu", r), "hu");
        // 整词等价：「孕妇」yong-hu = 「用户」yong-hu（find_candidates 经 lookup 命中）
        let idx = HotwordIndex::from_words(&["用户".to_string()]);
        // 模拟 find_candidates 的查询拼音：孕妇 = yun-fu → 归一 yong-hu
        let query_py = [norm("yun", r), norm("fu", r)].join("-");
        assert_eq!(query_py, "yong-hu");
        assert!(idx.lookup(2, &query_py).is_some(), "孕妇归一后应命中用户");
    }

    #[test]
    fn normalize_nl_rl_both() {
        // nl + rl 同开：n 与 r 首字母不同，互不干扰，都归一到 l
        let r = FuzzyRules { nl: true, rl: true, ..Default::default() };
        assert_eq!(norm("re", r), "le");
        assert_eq!(norm("niu", r), "liu");
        assert_eq!(norm("le", r), "le");
    }

    #[test]
    fn normalize_hw_dialect() {
        let r = FuzzyRules { hw: true, ..Default::default() };
        // 单字 hu→wu（胡/无）
        assert_eq!(norm("hu", r), "wu");
        assert_eq!(norm("wu", r), "wu");
        // 多字 huX→wX：huang 先基础 ang→an 得 huan，再 hu→w 得 wan；
        // wang 基础 ang→an 得 wan——两者归一相同（双向命中），归一结果不必是合法拼音。
        assert_eq!(norm("huang", r), "wan");
        assert_eq!(norm("wang", r), "wan");
        assert_eq!(norm("hua", r), "wa");
    }

    #[test]
    fn normalize_base_rules_always_on_dialect_off() {
        // 方言全关：基础规则（平翘舌 + 前后鼻音）仍生效
        let r = FuzzyRules::default();
        assert_eq!(norm("zhao", r), "zao"); // zh→z
        assert_eq!(norm("sheng", r), "sen"); // sh→s + eng→en
        // 默认方言关：f/n/hu 不归一
        assert_eq!(norm("fu", r), "fu");
        assert_eq!(norm("niu", r), "niu");
        assert_eq!(norm("hu", r), "hu");
        // huang 仅基础 ang→an（hu 不归一，方言关）→ huan
        assert_eq!(norm("huang", r), "huan");
    }

    #[test]
    fn normalize_fh_nl_hw_combine() {
        // 声母组同时开互不干扰（一个拼音只归一组，循环 break）；
        // 关键：fu 经 fh→hu 后**不**被 hw 二次转 wu（initial_matched flag 阻断 hw）。
        let r = FuzzyRules { fh: true, nl: true, hw: true, rl: true, yun_yong: false, fei_hui: false };
        assert_eq!(norm("fu", r), "hu"); // fh（不被 hw 二次捕获）
        assert_eq!(norm("niu", r), "liu"); // nl
        assert_eq!(norm("re", r), "le"); // rl
        assert_eq!(norm("huang", r), "wan"); // hw（基础 ang→an 后 hu→w）
    }
}
