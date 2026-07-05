use std::collections::HashMap;
use std::io::Read;
use flate2::read::GzDecoder;
use jieba_rs::Jieba;
use pinyin::ToPinyin;

const UNIGRAM_GZ: &[u8] = include_bytes!("corrector_data/unigram.txt.gz");
const BIGRAM_GZ: &[u8] = include_bytes!("corrector_data/bigram.txt.gz");

pub struct LightCorrector {
    jieba: Jieba,
    // Maps fuzzy pinyin to a list of candidate words
    // E.g., "yi-jing" -> ["已经", "一镜", "以经", ...]
    fuzzy_pinyin_to_words: HashMap<String, Vec<String>>,
    // Unigram log probabilities: word -> log(prob)
    unigram_scores: HashMap<String, f64>,
    // Bigram log probabilities: w1 -> { w2 -> log(prob) }
    bigram_scores: HashMap<String, HashMap<String, f64>>,
}

fn normalize_fuzzy_pinyin(py: &str) -> String {
    let mut normalized = py.to_lowercase();
    if normalized.starts_with("zh") {
        normalized = normalized.replacen("zh", "z", 1);
    } else if normalized.starts_with("ch") {
        normalized = normalized.replacen("ch", "c", 1);
    } else if normalized.starts_with("sh") {
        normalized = normalized.replacen("sh", "s", 1);
    }
    
    if normalized.starts_with('n') {
        normalized = format!("l{}", &normalized[1..]);
    }

    if normalized.ends_with("ing") {
        normalized = normalized[..normalized.len() - 3].to_string() + "in";
    } else if normalized.ends_with("eng") {
        normalized = normalized[..normalized.len() - 3].to_string() + "en";
    } else if normalized.ends_with("ang") {
        normalized = normalized[..normalized.len() - 3].to_string() + "an";
    }
    
    normalized
}

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
        let mut bigram_scores: HashMap<String, HashMap<String, f64>> = HashMap::new();
        let mut fuzzy_pinyin_to_words: HashMap<String, Vec<String>> = HashMap::new();
        let mut raw_unigram_freqs: HashMap<String, f64> = HashMap::new();

        // 1. Decompress and parse unigrams
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
                        unigrams.push((word.clone(), freq));
                        raw_unigram_freqs.insert(word, freq);
                    }
                }
            }

            if total_unigram_freq > 0.0 {
                let log_total = total_unigram_freq.ln();
                for (word, freq) in &unigrams {
                    unigram_scores.insert(word.clone(), freq.ln() - log_total);
                }
            }

            // Build fuzzy pinyin mapping
            for (word, _) in &unigrams {
                let fuzzy_py = get_fuzzy_pinyin(word);
                if !fuzzy_py.is_empty() {
                    fuzzy_pinyin_to_words.entry(fuzzy_py).or_default().push(word.clone());
                }
            }
        }

        // 2. Decompress and parse bigrams
        let mut bigram_decoder = GzDecoder::new(BIGRAM_GZ);
        let mut bigram_str = String::new();
        if let Err(e) = bigram_decoder.read_to_string(&mut bigram_str) {
            log::error!("Failed to decompress embedded bigrams: {}", e);
        } else {
            let mut raw_bigrams = Vec::new();
            let mut w1_total_freq = HashMap::new();

            for line in bigram_str.lines() {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() == 2 {
                    let pair = parts[0];
                    if let Ok(freq) = parts[1].parse::<f64>() {
                        if let Some(colon_pos) = pair.find(':') {
                            let w1 = pair[..colon_pos].to_string();
                            let w2 = pair[colon_pos + 1..].to_string();
                            *w1_total_freq.entry(w1.clone()).or_insert(0.0) += freq;
                            raw_bigrams.push((w1, w2, freq));
                        }
                    }
                }
            }

            for (w1, w2, freq) in raw_bigrams {
                let denom = if let Some(&uni_freq) = raw_unigram_freqs.get(&w1) {
                    uni_freq
                } else {
                    w1_total_freq.get(&w1).copied().unwrap_or(freq)
                };
                let denom = if denom < freq { freq } else { denom };
                let log_prob = freq.ln() - denom.ln();
                bigram_scores.entry(w1).or_default().insert(w2, log_prob);
            }
        }

        Self {
            jieba: Jieba::new(),
            fuzzy_pinyin_to_words,
            unigram_scores,
            bigram_scores,
        }
    }

    fn is_jieba_valid_word(&self, word: &str) -> bool {
        let cuts = self.jieba.cut(word, false);
        cuts.len() == 1 && cuts[0] == word
    }

    fn get_word_score(&self, word: &str) -> f64 {
        if let Some(&score) = self.unigram_scores.get(word) {
            score
        } else if self.is_jieba_valid_word(word) {
            -12.0
        } else {
            -18.0
        }
    }

    fn find_candidates(&self, query_word: &str) -> Vec<String> {
        let char_len = query_word.chars().count();
        if char_len < 2 {
            return vec![query_word.to_string()];
        }

        let query_py = get_fuzzy_pinyin(query_word);
        if query_py.is_empty() {
            return vec![query_word.to_string()];
        }

        let mut candidates = Vec::new();
        if let Some(words) = self.fuzzy_pinyin_to_words.get(&query_py) {
            for w in words {
                if w.chars().count() == char_len {
                    candidates.push(w.clone());
                }
            }
        }

        if !candidates.contains(&query_word.to_string()) {
            candidates.push(query_word.to_string());
        }

        candidates
    }

    fn score_sentence(&self, text: &str) -> f64 {
        let words: Vec<&str> = self.jieba.cut(text, false);
        if words.is_empty() {
            return -99.0;
        }

        let alpha = 1.0;
        let beta = 1.5;
        let mut total_score = 0.0;

        for i in 0..words.len() {
            let w = words[i];
            let score_uni = self.get_word_score(w);

            let score_prev = if i > 0 {
                let w_prev = words[i - 1];
                if let Some(next_map) = self.bigram_scores.get(w_prev) {
                    if let Some(&log_prob) = next_map.get(w) {
                        log_prob
                    } else {
                        self.get_word_score(w) - 2.0
                    }
                } else {
                    self.get_word_score(w) - 2.0
                }
            } else {
                0.0
            };

            let score_next = if i + 1 < words.len() {
                let w_next = words[i + 1];
                if let Some(next_map) = self.bigram_scores.get(w) {
                    if let Some(&log_prob) = next_map.get(w_next) {
                        log_prob
                    } else {
                        self.get_word_score(w_next) - 2.0
                    }
                } else {
                    self.get_word_score(w_next) - 2.0
                }
            } else {
                0.0
            };

            total_score += alpha * score_uni + beta * (score_prev + score_next);
        }

        total_score / (words.len() as f64)
    }

    pub fn correct(&self, text: &str) -> String {
        self.correct_greedy(text)
    }

    /// 上下文窗口半幅（评分用）：窗口中心词前后各取 CTX_HALF 个字，
    /// 总长 ≤ 2*CTX_HALF + sz。jieba.cut 在 30 字规模开销极低。
    const CTX_HALF: usize = 15;

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
        let mut i = 0;
        while i < n {
            let mut replaced_sz = 0;
            for sz in (2..=3).rev() {
                if i + sz > n {
                    continue;
                }
                let window_word: String = chars[i..i + sz].iter().collect();
                let candidates = self.find_candidates(&window_word);
                if candidates.len() <= 1 {
                    continue;
                }

                let change_penalty = if self.is_jieba_valid_word(&window_word) {
                    -1.5
                } else {
                    -0.2
                };

                // 局部上下文评分：取窗口前后各 CTX_HALF 字做 jieba 分词，
                // 大幅减少重复全句分词开销（O(N²) → O(CTX²)）。
                let ctx_start = i.saturating_sub(Self::CTX_HALF);
                let ctx_end = (i + sz + Self::CTX_HALF).min(n);
                let offset = i - ctx_start;

                // baseline：原句上下文（含 window_word）
                let baseline_ctx: String = chars[ctx_start..ctx_end].iter().collect();
                let baseline_score = self.score_sentence(&baseline_ctx);

                let mut best_cand = window_word.clone();
                let mut best_gain = 0.0f64;

                for cand in candidates {
                    // 在 baseline_ctx 副本中做局部替换再评分
                    let mut ctx_chars: Vec<char> = baseline_ctx.chars().collect();
                    let cand_chars: Vec<char> = cand.chars().collect();
                    ctx_chars[offset..(sz + offset)].copy_from_slice(&cand_chars[..sz]);
                    let cand_ctx: String = ctx_chars.iter().collect();
                    let cand_score = self.score_sentence(&cand_ctx);

                    // 增量 gain：正值表示候选优于原词。非原词扣 change_penalty。
                    let gain = if cand != window_word {
                        cand_score - baseline_score + change_penalty
                    } else {
                        cand_score - baseline_score
                    };
                    if gain > best_gain {
                        best_gain = gain;
                        best_cand = cand;
                    }
                }

                if best_cand != window_word {
                    log::info!(
                        "[ASR Correct] Replacing '{}' with '{}' (gain: {:.2})",
                        window_word,
                        best_cand,
                        best_gain
                    );
                    let best_chars: Vec<char> = best_cand.chars().collect();
                    chars[i..(sz + i)].copy_from_slice(&best_chars[..sz]);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_corrector_homophones() {
        let corrector = get_corrector();
        let input = "我们以经坐上飞机了";
        let output = corrector.correct(input);
        assert_eq!(output, "我们已经坐上飞机了");
    }

    #[test]
    fn test_corrector_fuzzy_accent() {
        let corrector = get_corrector();
        let input = "打扫微生";
        let output = corrector.correct(input);
        assert_eq!(output, "打扫卫生");
    }

    #[test]
    fn test_corrector_unaffected() {
        let corrector = get_corrector();
        let input = "你好，世界！Hello World.";
        let output = corrector.correct(input);
        assert_eq!(output, input);
    }
}
