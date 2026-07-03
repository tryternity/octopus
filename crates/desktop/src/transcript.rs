// crates/desktop/src/transcript.rs
//! 识别过程文本状态机：段（segment）模型。
//!
//! `segments` = 结构化真相源（`Vec<Segment>`，每段带 `kind`）；`caret_gap` = 新语音生长缝隙
//! （0..=segments.len()，==len 即末尾追加）。`finish_text()` 段扁平纯文本（= display = 落库搜索
//! = clipboard，派生）。默认 `segments=[]`+`caret_gap=0` 等价旧空文档；`caret_gap==len` 等价
//! 旧末尾追加（零回归）。润色全篇一次：edited 冻结、raw/polished 重润（best-effort 串匹配回填）。

use crate::config::PolishMode;
use std::time::Instant;

/// 段类型。后态覆盖前态：Raw → Polished → Edited。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentKind { Raw, Polished, Edited }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment { pub kind: SegmentKind, pub text: String }

/// 给 octopus_llm 的润色输入（segments 快照）。edited 段标 preserve，其余待润色。
#[derive(Debug, Clone)]
pub struct PolishInput { pub segments: Vec<Segment> }

pub struct Transcript {
    pub id: i64,
    mode: PolishMode,
    segments: Vec<Segment>,
    /// 新语音生长缝隙，0..=segments.len()。
    caret_gap: usize,
    /// 引擎累积全量，仅作 delta 提取基准。不显示、不落库。
    engine_cumulative: String,
    engine_consumed_chars: usize,
    /// diverted（引擎纠正早前文本）时，新 full 与已展示 finish_text 的 LCP 之后的差异。
    /// 不立即展示（避免引擎抖动），下次 apply_engine_full 时连同本次 delta 补发。
    diverted_pending: String,
    last_polish_time: Instant,
    polish_pending: bool,
    /// 润色发起时的 segments 快照（PolishDone 回填比对用）。
    polish_snapshot: Vec<Segment>,
    /// 润色发起时的 caret char offset（PolishDone 后恢复光标到同位置）。
    polish_caret_offset: usize,
    /// 润色发起时 caret 是否在末尾（caret_gap==segments.len()）。
    /// true → polish 后 caret 停新末尾；false → 精确恢复 char offset（中插态）。
    polish_caret_at_tail: bool,
    /// pending 期间缓存的新 delta（pending 不写 segments，PolishDone 后 flush）。
    pending_delta: String,
    db_inserted: bool,
}

impl Transcript {
    pub fn new(id: i64, mode: PolishMode) -> Self {
        Self {
            id, mode, segments: Vec::new(), caret_gap: 0,
            engine_cumulative: String::new(), engine_consumed_chars: 0, diverted_pending: String::new(),
            last_polish_time: Instant::now(), polish_pending: false,
            polish_snapshot: Vec::new(), polish_caret_offset: 0, polish_caret_at_tail: true,
            pending_delta: String::new(), db_inserted: false,
        }
    }

    pub fn db_inserted(&self) -> bool { self.db_inserted }
    pub fn mark_db_inserted(&mut self) { self.db_inserted = true; }

    /// 段顺序拼接 → 纯文本（= display = 落库搜索 = clipboard）。派生。
    pub fn finish_text(&self) -> String {
        self.segments.iter().map(|s| s.text.as_str()).collect()
    }
    /// 兼容旧名；段模型下 == finish_text。
    pub fn display_text(&self) -> String { self.finish_text() }
    /// 兼容旧名（pipeline/coordinator 调用点过渡用）；== finish_text。
    pub fn full(&self) -> String { self.finish_text() }
    /// 兼容旧名；== finish_text。
    pub fn db_text(&self) -> String { self.finish_text() }

    /// 引擎累积全量 → 取尾部 delta → 在 caret_gap 生长。返回是否变化（delta 非空）。
    /// diverted（非前缀，引擎纠正早前文本）→ 新 full 与已展示 finish_text 的 LCP 之后差异
    /// 暂存 diverted_pending（不立即展示，避免抖动），重算基准；下次 apply 时连同补发。
    /// （不回退已展示——no rollback。）
    pub fn apply_engine_full(&mut self, full: &str) -> bool {
        // 先 flush 上次 diverted 暂存（连同本次一起处理）
        let mut combined_delta;
        let is_prefix = full.starts_with(self.engine_cumulative.as_str());
        if is_prefix {
            combined_delta = full.chars().skip(self.engine_consumed_chars).collect::<String>();
        } else {
            // diverted：算新 full 与当前 finish_text 的 LCP，之后的差异暂存
            let shown = self.finish_text();
            let lcp = common_prefix_len(&shown, full);
            let diff: String = full.chars().skip(lcp).collect();
            self.diverted_pending.push_str(&diff);
            self.engine_cumulative = full.to_string();
            self.engine_consumed_chars = full.chars().count();
            return false; // 当次不展示（diverted 延迟确认）
        }
        self.engine_cumulative = full.to_string();
        self.engine_consumed_chars = full.chars().count();
        // flush diverted_pending（若有）到本次 delta 前
        if !self.diverted_pending.is_empty() {
            let dp = std::mem::take(&mut self.diverted_pending);
            let mut s = dp;
            s.push_str(&combined_delta);
            combined_delta = s;
        }
        if combined_delta.is_empty() { return false; }
        if self.polish_pending { self.pending_delta.push_str(&combined_delta); }
        else { self.push_delta_at_caret(&combined_delta); }
        true
    }

    /// VadSegmented append_segment（delta 直接生长，不经 engine_cumulative）。
    pub fn append_segment(&mut self, delta: &str) {
        if delta.is_empty() { return; }
        if self.polish_pending { self.pending_delta.push_str(delta); }
        else { self.push_delta_at_caret(delta); }
    }

    /// 在 caret_gap 处确保有 Raw 段并追加 delta：
    /// - 前邻段（caret_gap-1）为 Raw → 追加到该段。
    /// - 否则插入一条 Raw 段到 caret_gap，caret_gap 后移到新段之后。
    fn push_delta_at_caret(&mut self, delta: &str) {
        if delta.is_empty() { return; }
        let gap = self.caret_gap.min(self.segments.len());
        if gap > 0 && self.segments[gap - 1].kind == SegmentKind::Raw {
            self.segments[gap - 1].text.push_str(delta);
        } else {
            self.segments.insert(gap, Segment { kind: SegmentKind::Raw, text: delta.to_string() });
            self.caret_gap = gap + 1;
        }
    }

    /// 前端点击 → char offset → 定位光标。落段内→劈段（同 kind 一分为二）；落段界→置 gap。clamp [0,len]。
    pub fn set_caret(&mut self, char_off: usize) {
        let mut acc = 0usize;
        for (i, seg) in self.segments.iter().enumerate() {
            let len = seg.text.chars().count();
            if char_off < acc + len {
                if char_off == acc {
                    self.caret_gap = i;
                } else {
                    let rel = char_off - acc;
                    let chars: Vec<char> = seg.text.chars().collect();
                    let left: String = chars[..rel].iter().collect();
                    let right: String = chars[rel..].iter().collect();
                    let kind = seg.kind;
                    self.segments[i] = Segment { kind, text: left };
                    self.segments.insert(i + 1, Segment { kind, text: right });
                    self.caret_gap = i + 1;
                }
                return;
            }
            acc += len;
        }
        self.caret_gap = self.segments.len();
    }

    /// 当前 caret 在 finish_text 的 char offset（前端光标像素定位用）。
    pub fn caret_char_offset(&self) -> usize {
        let gap = self.caret_gap.min(self.segments.len());
        self.segments[..gap].iter().map(|s| s.text.chars().count()).sum()
    }

    /// 是否处于中间插入态（caret_gap < 段数）。pipeline Emit insertion 标志用。
    pub fn is_inserting(&self) -> bool { self.caret_gap < self.segments.len() }

    /// 编辑提交：整篇压成一条 Edited；raw/polished 清零。空串→清空。
    pub fn commit_edit(&mut self, flat: &str) {
        if flat.is_empty() { self.segments.clear(); self.caret_gap = 0; return; }
        self.segments = vec![Segment { kind: SegmentKind::Edited, text: flat.to_string() }];
        self.caret_gap = 1;
    }

    /// 是否含 Raw 段（mode=2 中间润色触发判定，替代旧 has_increase）。
    pub fn has_raw(&self) -> bool { self.segments.iter().any(|s| s.kind == SegmentKind::Raw) }

    /// 是否含 Edited 段（替代旧 has_edit，PolishDone 落库分支用）。
    pub fn has_edit(&self) -> bool { self.segments.iter().any(|s| s.kind == SegmentKind::Edited) }

    /// 取润色输入：快照 segments + 记 caret offset/末尾态 + 标记 pending。
    pub fn take_polish_input(&mut self) -> PolishInput {
        self.polish_snapshot = self.segments.clone();
        self.polish_caret_offset = self.caret_char_offset();
        self.polish_caret_at_tail = self.caret_gap >= self.segments.len();
        self.polish_pending = true;
        PolishInput { segments: self.polish_snapshot.clone() }
    }

    /// 润色完成回填：snapshot 的 edited 段在 full 里串匹配定位 → Edited；间隙 → Polished。
    /// 恢复 caret：发起时末尾态→停新末尾；中插态→精确恢复 char offset。flush pending_delta。
    pub fn polish_apply(&mut self, full: &str) {
        let snapshot = std::mem::take(&mut self.polish_snapshot);
        let caret_off = self.polish_caret_offset;
        let at_tail = self.polish_caret_at_tail;
        self.polish_pending = false;
        self.segments = rebuild_after_polish(&snapshot, full);
        if at_tail {
            self.caret_gap = self.segments.len();
        } else {
            let total = self.finish_text().chars().count();
            self.set_caret(caret_off.min(total));
        }
        let pending = std::mem::take(&mut self.pending_delta);
        if !pending.is_empty() { self.push_delta_at_caret(&pending); }
        self.last_polish_time = Instant::now();
    }

    /// 润色失败：清 pending；flush pending_delta（保留新语音）。segments 不变。
    pub fn on_polish_failed(&mut self) {
        self.polish_pending = false;
        self.polish_snapshot.clear();
        let pending = std::mem::take(&mut self.pending_delta);
        if !pending.is_empty() { self.push_delta_at_caret(&pending); }
    }

    pub fn polish_pending(&self) -> bool { self.polish_pending }
    pub fn mark_polish_pending(&mut self) {
        self.polish_snapshot = self.segments.clone();
        self.polish_caret_offset = self.caret_char_offset();
        self.polish_caret_at_tail = self.caret_gap >= self.segments.len();
        self.polish_pending = true;
    }
    #[allow(dead_code)] pub fn clear_polish_pending(&mut self) { self.polish_pending = false; }
    pub fn last_polish_time(&self) -> Instant { self.last_polish_time }
    pub fn set_mode(&mut self, mode: PolishMode) { self.mode = mode; }

    /// 段序列化给 DB（JSON）。 [{"kind":"raw|polished|edited","text":"..."}]
    pub fn segments_json(&self) -> String {
        serde_json::to_string(
            &self.segments.iter().map(|s| {
                let k = match s.kind {
                    SegmentKind::Raw => "raw", SegmentKind::Polished => "polished", SegmentKind::Edited => "edited",
                };
                serde_json::json!({ "kind": k, "text": s.text })
            }).collect::<Vec<_>>(),
        ).unwrap_or_else(|_| "[]".to_string())
    }
}

/// 润色回填：snapshot + LLM 输出 full → 新 segments（edited 串匹配定位，间隙 Polished，无 Raw）。
fn rebuild_after_polish(snapshot: &[Segment], full: &str) -> Vec<Segment> {
    let edited: Vec<&str> = snapshot.iter()
        .filter(|s| s.kind == SegmentKind::Edited).map(|s| s.text.as_str()).collect();
    if edited.is_empty() {
        return vec![Segment { kind: SegmentKind::Polished, text: full.to_string() }];
    }
    let full_chars: Vec<char> = full.chars().collect();
    let mut segs = Vec::new();
    let mut cursor = 0usize;
    for ed in &edited {
        let ed_chars: Vec<char> = ed.chars().collect();
        match find_from(&full_chars, &ed_chars, cursor) {
            Some(start) => {
                if start > cursor {
                    let gap: String = full_chars[cursor..start].iter().collect();
                    if !gap.is_empty() { segs.push(Segment { kind: SegmentKind::Polished, text: gap }); }
                }
                let end = start + ed_chars.len();
                segs.push(Segment { kind: SegmentKind::Edited, text: full_chars[start..end].iter().collect() });
                cursor = end;
            }
            None => {
                // 匹配不到（LLM 擅改）：剩余全作 Polished，停止（best-effort）
                if cursor < full_chars.len() {
                    segs.push(Segment { kind: SegmentKind::Polished, text: full_chars[cursor..].iter().collect() });
                    cursor = full_chars.len();
                }
                break;
            }
        }
    }
    if cursor < full_chars.len() {
        let rest: String = full_chars[cursor..].iter().collect();
        if !rest.is_empty() { segs.push(Segment { kind: SegmentKind::Polished, text: rest }); }
    }
    segs
}

/// chars 里从 from 找子串 sub（返回起始 char index）。
fn find_from(chars: &[char], sub: &[char], from: usize) -> Option<usize> {
    if sub.is_empty() { return Some(from); }
    let mut i = from;
    while i + sub.len() <= chars.len() {
        if chars[i..i + sub.len()] == *sub { return Some(i); }
        i += 1;
    }
    None
}

/// 两字符串的公共前缀 char 长度。
fn common_prefix_len(a: &str, b: &str) -> usize {
    a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(t: &str) -> Segment { Segment { kind: SegmentKind::Raw, text: t.into() } }
    fn pol(t: &str) -> Segment { kind_seg(SegmentKind::Polished, t) }
    fn edt(t: &str) -> Segment { kind_seg(SegmentKind::Edited, t) }
    fn kind_seg(k: SegmentKind, t: &str) -> Segment { Segment { kind: k, text: t.into() } }

    // ── 默认零回归 ──
    #[test]
    fn empty_default_finish_empty() {
        let t = Transcript::new(1, PolishMode::Intermediate);
        assert_eq!(t.finish_text(), "");
        assert_eq!(t.display_text(), "");
        assert!(!t.is_inserting());
        assert!(!t.has_raw());
    }

    #[test]
    fn apply_engine_full_appends_at_tail_by_default() {
        // 默认 caret_gap==0==len：首 delta 新建 Raw 段，后续追加同段（≡ 旧末尾追加）
        let mut t = Transcript::new(2, PolishMode::Intermediate);
        assert!(t.apply_engine_full("你好"));
        assert!(t.apply_engine_full("你好世界"));
        assert_eq!(t.finish_text(), "你好世界");
        assert!(!t.is_inserting()); // caret_gap==len
    }

    #[test]
    fn apply_engine_full_diverted_drops_delta_no_rollback() {
        let mut t = Transcript::new(3, PolishMode::Intermediate);
        t.apply_engine_full("你好");
        let changed = t.apply_engine_full("替换全文"); // 非「你好」前缀 = diverted
        assert!(!changed);
        assert_eq!(t.finish_text(), "你好"); // 不回退已展示
        // 重算基准后，后续正常追加
        assert!(t.apply_engine_full("替换全文后"));
        assert_eq!(t.finish_text(), "你好替换全文后");
    }

    #[test]
    fn append_segment_vad_accumulates() {
        let mut t = Transcript::new(4, PolishMode::FinalOnly);
        t.append_segment("甲");
        t.append_segment("乙");
        assert_eq!(t.finish_text(), "甲乙");
    }

    // ── set_caret（劈段/段界/clamp）──
    #[test]
    fn set_caret_at_segment_boundary_sets_gap() {
        let mut t = Transcript::new(5, PolishMode::Intermediate);
        t.apply_engine_full("你好世界");
        t.set_caret(2); // 段界（「你好」|「世界」在单 Raw 段内 offset 2）
        assert!(t.is_inserting());
        assert_eq!(t.caret_char_offset(), 2);
        t.apply_engine_full("你好世界中间");
        // 新 delta 在 offset 2 生长：前「你好」+ 新「中间」+ 后「世界」
        assert_eq!(t.finish_text(), "你好中间世界");
    }

    #[test]
    fn set_caret_splits_edited_segment() {
        let mut t = Transcript::new(6, PolishMode::Intermediate);
        t.commit_edit("abcdef"); // 单 Edited 段，caret_gap=1
        t.set_caret(3); // 劈 Edited → [Edited(abc)][Edited(def)]，caret_gap=1
        assert_eq!(t.segments.len(), 2);
        assert_eq!(t.segments[0].kind, SegmentKind::Edited);
        assert_eq!(t.segments[0].text, "abc");
        assert_eq!(t.segments[1].text, "def");
        assert!(t.is_inserting());
    }

    #[test]
    fn set_caret_clamps_beyond_end() {
        let mut t = Transcript::new(7, PolishMode::Intermediate);
        t.apply_engine_full("abc");
        t.set_caret(999); // 超出 → 末尾
        assert!(!t.is_inserting());
    }

    #[test]
    fn set_caret_empty_doc_clamps_zero() {
        let mut t = Transcript::new(8, PolishMode::Intermediate);
        t.set_caret(5);
        assert_eq!(t.caret_gap, 0);
    }

    // ── push_delta_at_caret 边界 ──
    #[test]
    fn push_delta_creates_new_raw_when_prev_not_raw() {
        let mut t = Transcript::new(9, PolishMode::Intermediate);
        t.commit_edit("edited"); // [Edited], caret_gap=1
        t.set_caret(0); // caret_gap=0（Edited 段之前）
        t.apply_engine_full("新语音");
        // caret_gap=0，前邻无 → 新建 Raw 段插入到 0，caret_gap=1
        assert_eq!(t.finish_text(), "新语音edited");
        assert_eq!(t.segments[0].kind, SegmentKind::Raw);
        assert_eq!(t.segments[1].kind, SegmentKind::Edited);
    }

    // ── commit_edit ──
    #[test]
    fn commit_edit_flattens_to_single_edited_clears_others() {
        let mut t = Transcript::new(10, PolishMode::Intermediate);
        t.apply_engine_full("raw1");
        t.commit_edit("手改");
        assert_eq!(t.segments, vec![edt("手改")]);
        assert_eq!(t.caret_gap, 1);
    }

    #[test]
    fn commit_edit_empty_clears_all() {
        let mut t = Transcript::new(11, PolishMode::Intermediate);
        t.apply_engine_full("raw");
        t.commit_edit("");
        assert!(t.segments.is_empty());
        assert_eq!(t.caret_gap, 0);
    }

    // ── polish_apply（润色回填）──
    #[test]
    fn polish_apply_raw_only_becomes_single_polished() {
        let mut t = Transcript::new(12, PolishMode::Intermediate);
        t.apply_engine_full("你好世界");
        let _input = t.take_polish_input();
        assert!(t.polish_pending);
        t.polish_apply("你好，世界。");
        assert_eq!(t.segments, vec![pol("你好，世界。")]);
        assert!(!t.has_raw()); // 不变量：润色后无 Raw
        assert!(!t.polish_pending);
    }

    #[test]
    fn polish_apply_preserves_edited_and_polishes_gap() {
        let mut t = Transcript::new(13, PolishMode::Intermediate);
        t.commit_edit("用户编辑");
        t.set_caret(4); // 中间
        t.apply_engine_full("raw尾"); // [Edited(用户编)][Raw 辑raw尾] → 实际 push 到 gap
        // 构造明确快照：手动设 segments
        t.segments = vec![edt("已确认"), raw("待润色")];
        t.caret_gap = 1;
        let _input = t.take_polish_input();
        t.polish_apply("已确认润色后");
        // edited「已确认」在 full 定位 → Edited；间隙/尾部 → Polished
        assert_eq!(t.segments[0].kind, SegmentKind::Edited);
        assert_eq!(t.segments[0].text, "已确认");
        assert!(t.segments.iter().all(|s| s.kind != SegmentKind::Raw));
    }

    #[test]
    fn polish_apply_edited_not_found_best_effort_all_polished() {
        let mut t = Transcript::new(14, PolishMode::Intermediate);
        t.segments = vec![edt("原文edited"), raw("x")];
        let _input = t.take_polish_input();
        // LLM 擅改，找不到「原文edited」→ 剩余全 Polished
        t.polish_apply("完全不同的润色");
        assert!(t.segments.iter().all(|s| s.kind == SegmentKind::Polished));
        assert_eq!(t.finish_text(), "完全不同的润色");
    }

    #[test]
    fn polish_apply_restores_caret_char_offset() {
        let mut t = Transcript::new(15, PolishMode::Intermediate);
        t.apply_engine_full("你好世界");
        t.set_caret(2);
        let off_before = t.caret_char_offset(); // 2
        let _input = t.take_polish_input();
        t.polish_apply("你好，世界。");
        assert_eq!(t.caret_char_offset(), off_before); // 润色后光标回同字符位
    }

    #[test]
    fn polish_apply_pending_delta_flushed_after() {
        let mut t = Transcript::new(16, PolishMode::Intermediate);
        t.apply_engine_full("你好");
        let _input = t.take_polish_input();
        // pending 期间新 delta 进 pending_delta（不写 segments）
        t.apply_engine_full("你好新语音");
        assert_eq!(t.finish_text(), "你好"); // pending 期间段不变
        t.polish_apply("你好。");
        // flush pending_delta → 新建 Raw「新语音」
        assert!(t.finish_text().ends_with("新语音"));
    }

    #[test]
    fn on_polish_failed_flushes_pending_delta() {
        let mut t = Transcript::new(17, PolishMode::Intermediate);
        t.apply_engine_full("你好");
        let _input = t.take_polish_input();
        t.apply_engine_full("你好新");
        t.on_polish_failed();
        assert!(!t.polish_pending);
        assert!(t.finish_text().ends_with("新")); // pending_delta 已 flush
    }

    // ── 类型不变量 ──
    #[test]
    fn invariant_no_raw_after_polish() {
        let mut t = Transcript::new(18, PolishMode::Intermediate);
        t.segments = vec![raw("a"), edt("b"), raw("c")];
        let _input = t.take_polish_input();
        t.polish_apply("a润b润c润");
        assert!(t.segments.iter().all(|s| s.kind != SegmentKind::Raw));
    }

    #[test]
    fn invariant_only_edited_after_commit() {
        let mut t = Transcript::new(19, PolishMode::Intermediate);
        t.segments = vec![raw("a"), pol("b")];
        t.commit_edit("flat");
        assert!(t.segments.iter().all(|s| s.kind == SegmentKind::Edited));
    }

    // ── segments_json ──
    #[test]
    fn segments_json_roundtrip_shape() {
        let mut t = Transcript::new(20, PolishMode::Intermediate);
        t.segments = vec![raw("a"), edt("b")];
        let j = t.segments_json();
        assert!(j.contains("\"kind\":\"raw\""));
        assert!(j.contains("\"kind\":\"edited\""));
        assert!(j.contains("\"text\":\"a\""));
    }
}
