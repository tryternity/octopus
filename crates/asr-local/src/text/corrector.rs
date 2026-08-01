// 有界热词纠错：热词命中即替换（correct_greedy），不依赖 unigram/bigram 评分。
// unigram_scores 保留供 is_common_word（miner 过滤常用词）；bigram 评分机制已移除。
use std::collections::HashMap;
use std::io::Read;
use flate2::read::GzDecoder;
use jieba_rs::Jieba;
use pinyin::ToPinyin;

use crate::hotword::{normalize_fuzzy_pinyin, HotwordIndex};

const UNIGRAM_GZ: &[u8] = include_bytes!("corrector_data/unigram.txt.gz");

pub struct LightCorrector {
    jieba: Jieba,
    // Unigram log probabilities: word -> log(prob)（is_common_word 用，miner 过滤常用词）
    unigram_scores: HashMap<String, f64>,
    // 热词索引——纠错候选的唯一来源。热路径读锁，reload 整体替换。
    // 空索引 → find_candidates 短路返回单候选 → 零纠错（消灭全词典自由联想的过纠根因）。
    hotwords: parking_lot::RwLock<HotwordIndex>,
    // active 热词缓存（word, pinyin, hit_count）——方言规则变更时用它重建 hotwords 索引
    //（索引 key 由 normalize_fuzzy_pinyin 生成，规则变 key 必变，见 reload_fuzzy_dialect）。
    active_words: parking_lot::RwLock<Vec<(String, String, i64)>>,
    // 字级 bigram 频次表（用户历史 voice 语料）——多命中上下文打分用。
    // scheduler CPU 空闲时 reload_bigrams 整体替换；空表 → bigram_score=0，回退 hit_count 排序。
    bigrams: parking_lot::RwLock<std::collections::HashMap<(char, char), usize>>,
    // 本次 correct 命中的热词收集（correct_greedy push，pipeline 经 drain_hits 取走后 bump DB）。
    // corrector 保持纯内存不碰 DB；命中计数持久化交调用层，避免单测污染真实 DB。
    pending_hits: parking_lot::Mutex<Vec<String>>,
}

/// 词级归一化模糊拼音（每个字归一化后用 `-` 连接）。
/// 内部调 `crate::hotword::normalize_fuzzy_pinyin`（与 HotwordIndex 同一规则）。
fn get_fuzzy_pinyin(word: &str) -> String {
    let mut parts = Vec::new();
    for p in word.to_pinyin().flatten() {
        parts.push(normalize_fuzzy_pinyin(p.plain()));
    }
    if parts.is_empty() {
        "".to_string()
    } else {
        parts.join("-")
    }
}

impl Default for LightCorrector {
    fn default() -> Self {
        Self::new()
    }
}

impl LightCorrector {
    pub fn new() -> Self {
        let mut unigram_scores: HashMap<String, f64> = HashMap::new();

        // Decompress and parse unigrams（is_common_word 用；热词索引改由 reload 注入）
        let mut unigram_decoder = GzDecoder::new(UNIGRAM_GZ);
        let mut unigram_str = String::new();
        if let Err(e) = unigram_decoder.read_to_string(&mut unigram_str) {
            log::error!("Failed to decompress embedded unigrams: {}", e);
        } else {
            let mut unigrams = Vec::new();
            let mut total_unigram_freq = 0.0;
            for line in unigram_str.lines() {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() == 2 {
                    let word = parts[0].to_string();
                    if let Ok(freq) = parts[1].parse::<f64>() {
                        total_unigram_freq += freq;
                        unigrams.push((word, freq));
                    }
                }
            }

            if total_unigram_freq > 0.0 {
                let log_total = total_unigram_freq.ln();
                for (word, freq) in &unigrams {
                    unigram_scores.insert(word.clone(), freq.ln() - log_total);
                }
            }
        }

        Self {
            jieba: Jieba::new(),
            unigram_scores,
            hotwords: parking_lot::RwLock::new(HotwordIndex::empty()),
            active_words: parking_lot::RwLock::new(Vec::new()),
            bigrams: parking_lot::RwLock::new(std::collections::HashMap::new()),
            pending_hits: parking_lot::Mutex::new(Vec::new()),
        }
    }

    /// 暴露内部 jieba 分词器供 miner 复用（避免每次新建 Jieba 的词典加载开销）。
    /// `jieba_rs::Jieba::cut` 是 `&self` 只读，线程安全可跨线程共享。
    pub fn jieba(&self) -> &Jieba {
        &self.jieba
    }

    /// 候选词**唯一来自 HotwordIndex**（active 热词）。
    /// 空热词或无命中 → 仅返回原词（单候选，correct_greedy 视为无操作）。
    /// 多命中时按 `bigram_score * W_CONTEXT + hit_count * W_HIT` 降序排序
    /// （上下文频次主导，hit_count 辅助），平局按 word 字典序（确定性）。
    /// `prev_char`/`next_char` = 窗口前后字符（'\0' 表示无边界），用于 bigram 上下文打分。
    fn find_candidates(&self, query_word: &str, prev_char: char, next_char: char) -> Vec<String> {
        let char_len = query_word.chars().count();
        if char_len < 2 {
            return vec![query_word.to_string()];
        }
        let idx = self.hotwords.read();
        if idx.is_empty() {
            // 无热词 → 无候选 → 零纠错（消灭全词典自由联想的过纠根因）
            return vec![query_word.to_string()];
        }
        let query_py = get_fuzzy_pinyin(query_word);
        if query_py.is_empty() {
            return vec![query_word.to_string()];
        }
        let mut candidates: Vec<(String, i64)> = idx
            .lookup(char_len, &query_py)
            .cloned()
            .unwrap_or_default();
        // bigram 上下文分 + hit_count 组合排序（降序 + 平局字典序）
        let bg = self.bigrams.read();
        candidates.sort_by(|a, b| {
            let sa = bigram_score(&bg, &a.0, prev_char, next_char) * W_CONTEXT
                + a.1 as f64 * W_HIT;
            let sb = bigram_score(&bg, &b.0, prev_char, next_char) * W_CONTEXT
                + b.1 as f64 * W_HIT;
            sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
        // 原词追加末尾（不参与排序——correct_greedy find 跳过它）
        if !candidates.iter().any(|(w, _)| w == query_word) {
            candidates.push((query_word.to_string(), 0));
        }
        candidates.into_iter().map(|(w, _)| w).collect()
    }

    pub fn correct(&self, text: &str) -> String {
        self.correct_greedy(text)
    }

    /// 贪心纠错：从左到右单次扫描，每处取最优候选词**原地替换**后继续前进。
    ///
    /// **性能**：单次扫描 + 字级 bigram 上下文打分（O(1) 查表），替代旧 `correct_depth`
    /// 的递归回头（最多 5 轮全句 jieba 分词 O(N³K) 爆炸）。
    fn correct_greedy(&self, text: &str) -> String {
        if text.trim().is_empty() {
            return text.to_string();
        }
        let mut chars: Vec<char> = text.chars().collect();
        let n = chars.len();
        // 窗口上限随热词 max_len 扩展（覆盖 >3 字热词）；空热词 max_len=0→max(3)=3，
        // 但 find_candidates 短路返回单候选，循环不替换，行为等价旧版「无操作」。
        let max_sz = { self.hotwords.read().max_len().max(3) };
        let mut i = 0;
        while i < n {
            let mut replaced_sz = 0;
            for sz in (2..=max_sz).rev() {
                if i + sz > n {
                    continue;
                }
                let window_word: String = chars[i..i + sz].iter().collect();
                let prev_char = if i > 0 { chars[i - 1] } else { '\0' };
                let next_char = if i + sz < n { chars[i + sz] } else { '\0' };
                let candidates = self.find_candidates(&window_word, prev_char, next_char);
                // 热词命中（候选含 ≠ 原词的热词）→ 直接替换。
                // spec 意图：热词是用户显式指定，命中即替换不否决。
                // 多热词同音时 find_candidates 已按 bigram 上下文 + hit_count 排序，
                // 这里取排序后第一个非原词候选。
                if let Some(hw) = candidates.iter().find(|c| *c != &window_word) {
                    log::info!(
                        "[ASR Correct] Hotword replace '{}' → '{}'",
                        window_word, hw
                    );
                    self.pending_hits.lock().push(hw.clone());
                    let hw_chars: Vec<char> = hw.chars().collect();
                    chars[i..(sz + i)].copy_from_slice(&hw_chars[..sz]);
                    replaced_sz = sz;
                    break; // 跳出 sz 循环，i 前进续扫
                }
            }
            // 替换后步进整个窗口（跳过已纠正的字，防重叠二次纠错）；
            // 未替换则 +1 滑窗。
            i += if replaced_sz > 0 { replaced_sz } else { 1 };
        }
        chars.iter().collect()
    }
}

pub static CORRECTOR: std::sync::OnceLock<LightCorrector> = std::sync::OnceLock::new();

pub fn get_corrector() -> &'static LightCorrector {
    CORRECTOR.get_or_init(LightCorrector::new)
}

/// 词是否在语言模型常用词表内（miner 过滤常用词候选用）。
/// unigram_scores 含通用语料高频词；专名/低频词不在表内 → 返回 false（保留为候选）。
/// 首调触发 corrector 初始化（加载 unigram/bigram 数据，一次性）。
pub fn is_common_word(word: &str) -> bool {
    get_corrector().unigram_scores.contains_key(word)
}

/// 用 active 热词列表重建 corrector 的热词索引。
/// entries = [(word, pinyin, hit_count)]，pinyin 是 DB 原始拼音，hit_count 用于多命中排序。
/// 启动时（DB 初始化后）与每次热词增删后调用。
/// 同时缓存到 `active_words`，供 [`reload_fuzzy_dialect`] 重建索引用。
/// corrector 未初始化时先 force init（空索引），再写入——确保首调也能落地。
pub fn reload_hotwords(entries: Vec<(String, String, i64)>) {
    // 锁外预建索引（拼音归一化 CPU 密集），避免持有 hotwords 写锁期间阻塞 ASR 读热路径。
    let new_index = HotwordIndex::from_words(&entries);
    let apply = |c: &LightCorrector| {
        *c.active_words.write() = entries;
        *c.hotwords.write() = new_index;
    };
    if let Some(c) = CORRECTOR.get() {
        apply(c);
    } else {
        // corrector 尚未初始化——先 force init（空索引），再写入
        let _ = get_corrector();
        if let Some(c) = CORRECTOR.get() {
            apply(c);
        }
    }
}

/// 从 DB 重新加载方言模糊规则（enabled 的规则）并用当前 active 热词重建索引。
/// set_config（规则变更后）与启动装载调用。2026-08-01 改无参——规则从 DB 读，不再接收字符串。
/// **必须重建索引**：`HotwordIndex::from_words` 用 [`crate::hotword::normalize_fuzzy_pinyin`]
/// 生成索引 key，规则变则 key 变，旧索引与新查询规则不一致会漏命中。用缓存的 `active_words` 原文重建。
pub fn reload_fuzzy_dialect() {
    // 从 DB 读 enabled 规则（按 match_type + sort_order 排序），更新全局缓存
    match crate::db::list_enabled_fuzzy_dialect_rules() {
        Ok(rules) => crate::hotword::set_fuzzy_rules_cache(rules),
        Err(e) => log::warn!("[corrector] 读方言规则失败，用空规则: {}", e),
    }
    // 确保单例已初始化（active_words 存在）；未初始化时 force init 空 words。
    let _ = get_corrector();
    if let Some(c) = CORRECTOR.get() {
        let words = c.active_words.read().clone();
        *c.hotwords.write() = HotwordIndex::from_words(&words);
    }
}

/// 上下文打分权重（bigram 主导，hit_count 辅助）。
const W_CONTEXT: f64 = 1.0;
const W_HIT: f64 = 0.3;

/// 从 DB 重新加载字级 bigram 频次表（用户历史 voice 语料）。
/// scheduler CPU 空闲时调。失败告警不阻断（bigrams 空 → bigram_score=0，回退 hit_count 排序）。
pub fn reload_bigrams() {
    match crate::miner::build_char_bigram_index() {
        Ok(index) => {
            let _ = get_corrector();
            if let Some(c) = CORRECTOR.get() {
                *c.bigrams.write() = index;
                log::info!("[corrector] bigram 索引已加载（{} 条字对）", c.bigrams.read().len());
            }
        }
        Err(e) => log::warn!("[corrector] 加载 bigram 索引失败，用空表: {}", e),
    }
}

/// 候选词在当前上下文的 bigram 打分（前缀 + 后缀字对频次）。
/// `prev_char` = 窗口前一字符（'\0' 表示文本开头无前缀），`next_char` = 窗口后一字符（'\0' 表示文末）。
fn bigram_score(
    bigrams: &std::collections::HashMap<(char, char), usize>,
    word: &str,
    prev_char: char,
    next_char: char,
) -> f64 {
    let mut score = 0.0;
    let chars: Vec<char> = word.chars().collect();
    let first = chars.first().copied();
    let last = chars.last().copied();
    // 前缀 bigram：(prev_char, word 首字)
    if prev_char != '\0' {
        if let Some(fc) = first {
            score += *bigrams.get(&(prev_char, fc)).unwrap_or(&0) as f64;
        }
    }
    // 后缀 bigram：(word 末字, next_char)
    if next_char != '\0' {
        if let Some(lc) = last {
            score += *bigrams.get(&(lc, next_char)).unwrap_or(&0) as f64;
        }
    }
    score
}

/// 取出并清空本次 corrector 命中的热词列表（`correct_greedy` push，pipeline 纠错后调）。
/// pipeline 批量 bump DB 命中计数；corrector 自身不碰 DB（分层，避免单测污染真实 DB）。
/// 单例未初始化返回空。注意：跨 correct 调用累积，未 drain 则残留。
pub fn drain_hits() -> Vec<String> {
    CORRECTOR
        .get()
        .map(|c| std::mem::take(&mut *c.pending_hits.lock()))
        .unwrap_or_default()
}

/// corrector 全局单例的测试串行锁（跨模块共享）。
///
/// corrector 是进程级单例，`reload_hotwords` 改全局热词索引。任何测试只要 touch
/// corrector（含 streaming_runner / engines / pipeline 的测试），都必须先持此锁，
/// 避免并发测试互相覆盖热词表。本模块 tests 内的 `serial()` 也复用此锁。
#[cfg(test)]
pub(crate) static CORRECTOR_TEST_LOCK: std::sync::OnceLock<std::sync::Mutex<()>> =
    std::sync::OnceLock::new();

/// 持此 guard 的测试段串行执行（跨模块统一锁）。调用方 `let _g = test_serial();`。
#[cfg(test)]
pub(crate) fn test_serial() -> std::sync::MutexGuard<'static, ()> {
    CORRECTOR_TEST_LOCK
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 持有此 guard 的测试段串行执行：correct 为只读，但 reload 写全局，
    /// 故整段 reload+correct+assert 必须在锁内（见各测试首行 `let _g = serial();`）。
    /// 复用跨模块共享的 `CORRECTOR_TEST_LOCK`（streaming_runner / pipeline 等测试共用）。
    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::text::corrector::test_serial()
    }

    /// 辅助：给单例 corrector 装载热词（hit_count=0）后返回它。调用方须先持 `serial()` guard。
    fn with_hotwords(words: &[&str]) -> &'static LightCorrector {
        let entries: Vec<(String, String, i64)> = words
            .iter()
            .map(|s| (s.to_string(), crate::hotword::word_raw_pinyin(s), 0))
            .collect();
        reload_hotwords(entries);
        get_corrector()
    }

    /// 辅助：给单例 corrector 装载热词（带 hit_count）后返回它。
    fn with_hotwords_scored(words: &[(&str, i64)]) -> &'static LightCorrector {
        let entries: Vec<(String, String, i64)> = words
            .iter()
            .map(|(s, hc)| (s.to_string(), crate::hotword::word_raw_pinyin(s), *hc))
            .collect();
        reload_hotwords(entries);
        get_corrector()
    }

    /// 辅助：设置方言规则（直接注入缓存，不经 DB）。调用方须先持 `serial()` guard。
    /// 传空 slice = 清空所有方言规则（仅基础规则）。
    fn set_rules(rules: &[(&str, &str, &str, &str)]) {
        let v: Vec<octopus_infra::db::FuzzyDialectRule> = rules
            .iter()
            .map(|(token, from, to, mt)| octopus_infra::db::FuzzyDialectRule {
                token: token.to_string(),
                label: token.to_string(),
                from_py: from.to_string(),
                to_py: to.to_string(),
                match_type: mt.to_string(),
                enabled: true,
                sort_order: 1,
            })
            .collect();
        crate::hotword::set_fuzzy_rules_cache(v);
    }

    #[test]
    fn test_hotword_homophone_replace() {
        let _g = serial();
        let c = with_hotwords(&["已经"]);
        // 模型把「已经」误识为同音的「以经」→ 热词命中替换
        assert_eq!(c.correct("我们以经坐上飞机了"), "我们已经坐上飞机了");
    }

    #[test]
    fn test_hotword_leci_to_reci() {
        // 用户报告：「乐词」应被纠正为「热词」（le-ci → re-ci，l/r 混淆）
        // 需先开 rl 模糊规则
        let _g = serial();
        set_rules(&[("r/l", "r", "l", "initial")]);
        let c = with_hotwords(&["热词"]);
        let result = c.correct("乐词");
        assert_eq!(result, "热词", "乐词应被热词纠正为热词（l/r 混淆）");
    }

    #[test]
    fn test_hotword_fuzzy_accent() {
        let _g = serial();
        let c = with_hotwords(&["卫生"]);
        // 平翘舌/前后鼻音误读：微生(wei-sheng)→卫生(wei-sheng)，模糊归一后命中
        assert_eq!(c.correct("打扫微生"), "打扫卫生");
    }

    #[test]
    fn test_no_hotword_is_noop() {
        let _g = serial();
        // 空热词 → 原样返回，零纠错（过纠根因消失的铁证）
        let c = with_hotwords(&[]);
        assert_eq!(c.correct("我们以经坐上飞机了"), "我们以经坐上飞机了");
    }

    #[test]
    fn test_overcorrection_regression() {
        let _g = serial();
        // 历史过纠案例：模型正确的「开始语音识别」在旧 corrector 被改成「开始于饮食别」。
        // 有界版即使挂了热词，未命中窗口也必须原样返回。
        let c = with_hotwords(&["八爪鱼"]);
        assert_eq!(c.correct("开始语音识别"), "开始语音识别");
    }

    #[test]
    fn test_unaffected_text() {
        let _g = serial();
        let c = with_hotwords(&["八爪鱼"]);
        let input = "你好，世界！Hello World.";
        assert_eq!(c.correct(input), input);
    }

    #[test]
    fn test_longer_hotword_window() {
        let _g = serial();
        // 3 字热词（旧 correct_greedy 窗口只到 3；重构后按 max_len 覆盖）
        let c = with_hotwords(&["八爪鱼"]);
        // 同音误识「扒爪鱼」(ba-zhua-yu) → 命中
        assert_eq!(c.correct("我在养扒爪鱼"), "我在养八爪鱼");
    }

    #[test]
    fn test_lowfreq_proper_noun_replace() {
        let _g = serial();
        // 低频专名（语料无分）：热词「浮窗」命中同音「福川」即替换——
        // 复刻用户 e2e（sensevoice 把「浮窗」识成「福川」）。
        // 旧 gain 机制会因专名语料分数低 + change_penalty 否决；命中即替换修复之。
        let c = with_hotwords(&["浮窗"]);
        assert_eq!(c.correct("开福川"), "开浮窗");
    }

    // ── 方言模糊规则（fuzzy_dialect）── 每个测试首行 reload_fuzzy_dialect 设自身状态，
    //    避免被前序测试的全局 FUZZY_RULES 残留影响（TEST_LOCK 串行，但全局跨段持久）。
    //    热词均用双字（单字 len<2 被 from_words 跳过）。

    #[test]
    fn test_dialect_fh_corrects() {
        let _g = serial();
        set_rules(&[("f/h", "f", "h", "initial")]);
        // 复刻用户 e2e：热词「浮窗」，识别成「护窗」(hu) → fh 规则 fu/hu 归一 → 命中替换。
        let c = with_hotwords(&["浮窗"]);
        assert_eq!(c.correct("开护窗"), "开浮窗");
    }

    #[test]
    fn test_dialect_nl_corrects() {
        let _g = serial();
        set_rules(&[("n/l", "n", "l", "initial")]);
        // 牛总 niu-zong → nl n→l → liu-zong；热词「刘总」liu-zong。命中。
        let c = with_hotwords(&["刘总"]);
        assert_eq!(c.correct("牛总"), "刘总");
    }

    #[test]
    fn test_dialect_hw_corrects() {
        let _g = serial();
        set_rules(&[("hu/wu", "hu", "w", "special_hu")]);
        // 小王 xiao-wang → 基础 wang→wan → xiao-wan；
        // 小黄 xiao-huang → 基础 huang→huan → hw hu→w → xiao-wan。归一相同，命中。
        let c = with_hotwords(&["小黄"]);
        assert_eq!(c.correct("小王"), "小黄");
    }

    #[test]
    fn test_dialect_default_off_no_correct() {
        let _g = serial();
        set_rules(&[]); // 无方言规则
        // 默认方言全关：护窗 hu 不归一到 fu，不纠正（防回归）。
        let c = with_hotwords(&["浮窗"]);
        assert_eq!(c.correct("开护窗"), "开护窗");
    }

    #[test]
    fn test_drain_hits_collects_matches() {
        let _g = serial();
        set_rules(&[]); // 无方言规则
        let _ = drain_hits(); // 清空前序测试残留（pending_hits 跨 correct 累积）
        let c = with_hotwords(&["浮窗"]);
        assert_eq!(c.correct("开福川"), "开浮窗");
        // 命中「浮窗」被收集到 pending_hits（内存，未碰 DB）
        assert_eq!(drain_hits(), vec!["浮窗".to_string()]);
        // drain 后清空
        assert!(drain_hits().is_empty());
    }

    // ── P2 多命中 hit_count 排序（2026-08-01）──
    // 同音多热词命中时，hit_count 高的优先（用户验证过的更可信）；
    // hit_count 相同按 word 字典序（确定性，避免 HashSet 迭代序不确定）。

    #[test]
    fn multi_hit_picks_higher_hit_count() {
        let _g = serial();
        set_rules(&[]);
        // 「浮窗」(hit=5) 和「福川」(hit=1) 拼音归一相同。输入「福创」——两者都是候选。
        // hit_count 5 > 1 → 浮窗胜出。
        let c = with_hotwords_scored(&[("浮窗", 5), ("福川", 1)]);
        assert_eq!(c.correct("福创"), "浮窗", "hit_count 5 > 1，浮窗应胜出");
    }

    #[test]
    fn multi_hit_zero_count_picks_alpha_order() {
        let _g = serial();
        set_rules(&[]);
        // 两个都 0 hit_count → 字典序确定性（不取决于 HashSet 迭代序）
        let c = with_hotwords_scored(&[("浮窗", 0), ("福川", 0)]);
        let r1 = c.correct("福创");
        let c2 = with_hotwords_scored(&[("浮窗", 0), ("福川", 0)]);
        let r2 = c2.correct("福创");
        assert_eq!(r1, r2, "零 hit_count 时应确定性选一个（{} vs {}）", r1, r2);
    }

    #[test]
    fn multi_hit_deterministic_across_reloads() {
        let _g = serial();
        set_rules(&[]);
        // 「浮窗」(fu-chuang, hit=3) 和「福川」(fu-chuan, hit=7) 拼音归一相同。
        // 输入「福创」(fu-chuang→归一同) —— 两个热词都是候选（都不是原词）。
        // hit_count 7 > 3 → 福川胜出。多次 reload 结果应一致（确定性）。
        let r1 = {
            let c = with_hotwords_scored(&[("浮窗", 3), ("福川", 7)]);
            c.correct("福创")
        };
        let r2 = {
            let c = with_hotwords_scored(&[("浮窗", 3), ("福川", 7)]);
            c.correct("福创")
        };
        assert_eq!(r1, r2, "多次 reload 结果应一致（确定性）：{} vs {}", r1, r2);
        assert_eq!(r1, "福川", "hit_count 7 > 3，福川应胜出（实际 {}）", r1);
    }

    // ── bigram 上下文打分（2026-08-01）──
    // 上下文频次主导排序，hit_count 辅助。bigram 命中的候选优先于纯 hit_count 高的。

    /// 辅助：注入 bigram 索引到 corrector 单例。
    fn set_bigrams(texts: &[&str]) {
        let owned: Vec<String> = texts.iter().map(|s| s.to_string()).collect();
        let idx = crate::miner::build_char_bigram_index_from(&owned);
        let c = get_corrector();
        *c.bigrams.write() = idx;
    }

    #[test]
    fn bigram_context_overrides_lower_hit_count() {
        let _g = serial();
        set_rules(&[]);
        // 历史语料里「打开浮窗」常见 → (开,浮)(浮,窗) bigram 频次高
        set_bigrams(&["打开浮窗", "打开浮窗", "打开浮窗"]);
        // 「浮窗」hit=1 但 bigram 命中高；「福川」hit=10 但 bigram 不命中
        // 上下文主导：浮窗 bigram_score ≈ 3+3=6 * 1.0 = 6，福川 hit 10 * 0.3 = 3 → 浮窗胜
        let c = with_hotwords_scored(&[("浮窗", 1), ("福川", 10)]);
        assert_eq!(c.correct("开福创"), "开浮窗",
            "bigram 上下文命中应优先于 hit_count（浮窗 bigram≈6 > 福川 hit 3）");
    }

    #[test]
    fn no_bigram_falls_back_to_hit_count() {
        let _g = serial();
        set_rules(&[]);
        // 无 bigram 索引（空）→ 回退 hit_count 排序
        set_bigrams(&[]);
        let c = with_hotwords_scored(&[("浮窗", 1), ("福川", 10)]);
        assert_eq!(c.correct("开福创"), "开福川",
            "无 bigram 时回退 hit_count 排序（福川 hit 10 > 浮窗 hit 1）");
    }

    #[test]
    fn bigram_tie_keeps_deterministic() {
        let _g = serial();
        set_rules(&[]);
        // 两词 bigram 都不命中 + hit_count 相同 → 字典序确定性
        set_bigrams(&[]);
        let r1 = {
            let c = with_hotwords_scored(&[("浮窗", 0), ("福川", 0)]);
            c.correct("开福创")
        };
        let r2 = {
            let c = with_hotwords_scored(&[("浮窗", 0), ("福川", 0)]);
            c.correct("开福创")
        };
        assert_eq!(r1, r2, "bigram + hit_count 都平局时字典序确定性（{} vs {}）", r1, r2);
    }
}
