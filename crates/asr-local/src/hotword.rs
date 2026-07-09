use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use pinyin::ToPinyin;

/// 方言模糊规则开关——由 `app_config.fuzzy_dialect`（逗号分隔 token：`f/h`/`hu/wu`/`n/l`）
/// 经 [`parse_dialect`] 解析而来。
///
/// **基础规则（平翘舌 zh/ch/sh→z/c/s + 前后鼻音 ing/eng/ang→in/en/an）始终开启**，
/// 不在此处控制——它们是跨方言的常见识别容错。
///
/// 三组方言混淆按需启用（归一化单向，索引与查询共用 [`normalize_fuzzy_pinyin`] → 双向对称命中）：
/// - `fh`（f/h 不分，福建）：声母 f→h
/// - `nl`（n/l 不分，湖南）：声母 n→l
/// - `hw`（hu/wu 不分，江浙）：单字 hu→wu，其余 huX→wX（huang→wang、hua→wa）；
///   **不覆盖** hui↔wei（韵母 ui/ei 不同，拼音级无法统一）
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct FuzzyRules {
    /// f/h 不分（福建）：声母 f→h
    pub fh: bool,
    /// n/l 不分（湖南）：声母 n→l
    pub nl: bool,
    /// hu/wu 不分（江浙）：单字 hu→wu，其余 huX→wX
    pub hw: bool,
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
/// token：`f/h`→fh、`hu/wu`→hw、`n/l`→nl；空白与未知 token 忽略（前向兼容）。
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

/// 归一化逻辑（纯函数，便于单测无全局污染）。
/// 顺序：基础规则（始终）→ 可选方言 nl → fh → hw。
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
    // 可选方言组（fuzzy_dialect 控制）。基础规则不改首字母（zh→z 去尾 h、ing/eng/ang 改尾），
    // 故方言组仍可基于「基础后」的首字母 n/f/hu 互斥判断——else if 防止 fh 把 fu→hu 后被 hw
    // 二次捕获（一个字只归一组）。
    if rules.nl && n.starts_with('n') {
        n = format!("l{}", &n[1..]);
    } else if rules.fh && n.starts_with('f') {
        n = format!("h{}", &n[1..]);
    } else if rules.hw {
        // 单字 hu→wu（"胡/无"）；须先精确判 hu 再 starts_with，否则 hu 走第二分支变 "w"（非法拼音）。
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
        let r = parse_dialect("f/h,hu/wu,n/l");
        assert!(r.fh && r.hw && r.nl);
    }

    #[test]
    fn parse_dialect_single_and_empty() {
        let r = parse_dialect("f/h");
        assert!(r.fh && !r.nl && !r.hw);
        let r = parse_dialect("");
        assert!(!r.fh && !r.nl && !r.hw);
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
        // 三组同时开互不干扰（else if 互斥，一个拼音只归一组）；
        // 关键：fu 经 fh→hu 后**不**被 hw 二次转 wu（else if 链终止）。
        let r = FuzzyRules { fh: true, nl: true, hw: true };
        assert_eq!(norm("fu", r), "hu"); // fh（不被 hw 二次捕获）
        assert_eq!(norm("niu", r), "liu"); // nl
        assert_eq!(norm("huang", r), "wan"); // hw（基础 ang→an 后 hu→w）
    }
}
