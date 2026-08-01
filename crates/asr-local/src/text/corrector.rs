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
    // active 热词原文缓存——方言规则（FuzzyRules）变更时用它重建 hotwords 索引
    //（索引 key 由 normalize_fuzzy_pinyin 生成，规则变 key 必变，见 reload_fuzzy_dialect）。
    active_words: parking_lot::RwLock<Vec<String>>,
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
    fn find_candidates(&self, query_word: &str) -> Vec<String> {
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
        let mut candidates: Vec<String> = idx
            .lookup(char_len, &query_py)
            .cloned()
            .unwrap_or_default();
        if !candidates.contains(&query_word.to_string()) {
            candidates.push(query_word.to_string());
        }
        candidates
    }

    pub fn correct(&self, text: &str) -> String {
        self.correct_greedy(text)
    }

    /// 贪心纠错：从左到右单次扫描，每处取最优候选词**原地替换**后继续前进。
    ///
    /// **性能**：候选词评分用局部上下文（±15 字窗口）而非全句，
    /// 避免 N 字句 × K 候选 × O(N²) jieba 分词的 O(N³K) 爆炸；
    /// 单次扫描替代旧 `correct_depth` 的递归回头（最多 5 轮全句扫描）。
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
                let candidates = self.find_candidates(&window_word);
                // 热词命中（候选含 ≠ 原词的热词）→ 直接替换。
                // spec 意图：热词是用户显式指定，低频专名语料分数本就低，
                // 不靠 unigram/bigram gain 否决（旧 gain 机制反向坑了热词）。
                // 多热词同音取首个（lookup 顺序 = from_words 插入序）。
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

/// 用 active 热词文本列表重建 corrector 的热词索引。
/// 启动时（DB 初始化后）与每次热词增删/确认后调用。
/// 同时缓存原文到 `active_words`，供 [`reload_fuzzy_dialect`] 重建索引用。
/// corrector 未初始化时先 force init（空索引），再写入——确保首调也能落地。
pub fn reload_hotwords(words: Vec<String>) {
    // 锁外预建索引（拼音转换 CPU 密集），避免持有 hotwords 写锁期间阻塞 ASR 读热路径。
    let new_index = HotwordIndex::from_words(&words);
    let apply = |c: &LightCorrector| {
        *c.active_words.write() = words;
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

    /// 辅助：给单例 corrector 装载热词后返回它。调用方须先持 `serial()` guard。
    fn with_hotwords(words: &[&str]) -> &'static LightCorrector {
        let v: Vec<String> = words.iter().map(|s| s.to_string()).collect();
        reload_hotwords(v);
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
}
