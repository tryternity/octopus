// crates/desktop/src/transcript.rs
//! 识别过程文本状态机：段（segment）模型。
//!
//! `segments` = 结构化真相源（`Vec<Segment>`，每段带 `kind`）；`caret_gap` = 新语音生长缝隙
//! （0..=segments.len()，==len 即末尾追加）。`finish_text()` 段扁平纯文本（= display = 落库搜索
//! = clipboard，派生）。默认 `segments=[]`+`caret_gap=0` 等价旧空文档；`caret_gap==len` 等价
//! 旧末尾追加（零回归）。润色全篇一次：edited 冻结、raw/polished 重润（best-effort 串匹配回填）。

use crate::config::PolishMode;
use std::time::{Duration, Instant};

/// diverted_pending（引擎纠正延迟确认暂存）累积上限（char）。超限强制 flush 展示，
/// 避免引擎持续异常纠正时无限累积、用户看空白。
const DIVERTED_PENDING_LIMIT: usize = 500;

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
    /// 选中替换待删范围（扁平 char [start,end)）。set_selection 记录，apply_engine_full
    /// 首个非空 delta 或 take_polish_input 消费时真删。运行时态，不入库。
    pending_delete: Option<(usize, usize)>,
    /// 选中替换的插入点（selection start）。set_selection 时记录，跨润色持久——
    /// 润色后 caret 须精确恢复到此 char offset，而非跑到末尾。apply_engine_full/append_segment
    /// 消费 pending_delete 后不清零（润色仍需用它恢复 caret）；set_caret/clear_pending_delete 清零。
    selection_insert_offset: Option<usize>,
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
    /// 最近一次 DB 落库时间（落库节流用，Finalize 兜底完整写入）。None=未落库过。
    last_db_write: Option<Instant>,
}

impl Transcript {
    pub fn new(id: i64, mode: PolishMode) -> Self {
        Self {
            id, mode, segments: Vec::new(), caret_gap: 0,
            engine_cumulative: String::new(), engine_consumed_chars: 0, diverted_pending: String::new(),
            pending_delete: None,
            selection_insert_offset: None,
            last_polish_time: Instant::now(), polish_pending: false,
            polish_snapshot: Vec::new(), polish_caret_offset: 0, polish_caret_at_tail: true,
            pending_delta: String::new(), db_inserted: false,
            last_db_write: None,
        }
    }

    pub fn db_inserted(&self) -> bool { self.db_inserted }
    pub fn mark_db_inserted(&mut self) { self.db_inserted = true; }
    /// 标记已落库（更新 last_db_write = now，落库节流计时基准）。
    pub fn mark_db_written(&mut self) { self.last_db_write = Some(Instant::now()); }
    /// 距上次落库是否 ≥ threshold（节流判定）。未落库过 → true（应落库）。
    pub fn db_flush_due(&self, threshold: Duration) -> bool {
        self.last_db_write.map(|t| t.elapsed() >= threshold).unwrap_or(true)
    }

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
        // 消费 pending_delete（选中替换）——在任何 early return 之前。
        // 引擎静音期每 tick 产出 same-as-before 的 full（delta 空），旧代码在 delta 空
        // 时 return false 跳过消费 → 选区永远不删。现在只要 apply 被调用就消费。
        let mut selection_deleted = false;
        if let Some((s, e)) = self.pending_delete.take() {
            self.delete_range(s, e);
            selection_deleted = true;
            log::debug!(
                "[select] consumed apply_engine_full t={} range=[{},{}] full_len={} cum_len={} prefix={}",
                self.id, s, e, full.chars().count(),
                self.engine_cumulative.chars().count(),
                full.starts_with(self.engine_cumulative.as_str()));
            // 删除后的关键状态快照——用于排查 diverted LCP 失配
            log::debug!(
                "[seldbg] post-delete t={} shown='{}' cum='{}' consumed={} gap={} segs={}",
                self.id, self.finish_text(), self.engine_cumulative,
                self.engine_consumed_chars, self.caret_gap,
                self.debug_segments_str());
        }

        let mut combined_delta;
        let is_prefix = full.starts_with(self.engine_cumulative.as_str());
        if is_prefix {
            combined_delta = full.chars().skip(self.engine_consumed_chars).collect::<String>();
        } else {
            // diverted：算新 full 与当前 finish_text 的 LCP，之后的差异暂存
            let shown = self.finish_text();
            let lcp = common_prefix_len(&shown, full);
            let diff: String = full.chars().skip(lcp).collect();
            log::debug!(
                "[seldbg] DIVERTED t={} shown='{}' full='{}' lcp={} diff='{}' sel_del={} dp_len={}",
                self.id, shown, full, lcp, diff, selection_deleted,
                self.diverted_pending.chars().count() + diff.chars().count());
            self.diverted_pending.push_str(&diff);
            self.engine_cumulative = full.to_string();
            self.engine_consumed_chars = full.chars().count();
            if self.diverted_pending.chars().count() < DIVERTED_PENDING_LIMIT {
                // diverted 延迟确认——但如果选区刚被删，文本确实变了，须返回 true 让前端刷新
                return selection_deleted;
            }
            log::warn!("diverted_pending 累积超限({}), 强制 flush 展示", DIVERTED_PENDING_LIMIT);
            combined_delta = std::mem::take(&mut self.diverted_pending);
        }
        self.engine_cumulative = full.to_string();
        self.engine_consumed_chars = full.chars().count();
        if !self.diverted_pending.is_empty() {
            let dp = std::mem::take(&mut self.diverted_pending);
            let mut s = dp;
            s.push_str(&combined_delta);
            combined_delta = s;
        }
        if combined_delta.is_empty() && !selection_deleted { return false; }
        if !combined_delta.is_empty() {
            if self.polish_pending { self.pending_delta.push_str(&combined_delta); }
            else { self.push_delta_at_caret(&combined_delta); }
        }
        if selection_deleted || is_prefix {
            log::debug!(
                "[seldbg] emit t={} delta='{}' final='{}' gap={} segs={}",
                self.id, combined_delta, self.finish_text(),
                self.caret_gap, self.debug_segments_str());
        }
        true
    }

    /// VadSegmented append_segment（delta 直接生长，不经 engine_cumulative）。
    pub fn append_segment(&mut self, delta: &str) {
        // 消费 pending_delete（同 apply_engine_full，在任何 early return 之前）
        let mut selection_deleted = false;
        if let Some((s, e)) = self.pending_delete.take() {
            self.delete_range(s, e);
            selection_deleted = true;
            log::debug!(
                "[select] consumed append_segment t={} range=[{},{}] delta_len={}",
                self.id, s, e, delta.chars().count());
        }
        if delta.is_empty() && !selection_deleted { return; }
        if !delta.is_empty() {
            if self.polish_pending { self.pending_delta.push_str(delta); }
            else { self.push_delta_at_caret(delta); }
        }
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

    /// 在 char_off 处劈段，返回劈后 gap index（落段内→劈成两段返回 i+1；落段界→返回 i；
    /// 超出末尾→返回 segments.len()）。幂等：char_off 已在段界则不重复劈。set_caret 与
    /// delete_range 共用（DRY）。
    fn split_at(&mut self, char_off: usize) -> usize {
        let mut acc = 0usize;
        for (i, seg) in self.segments.iter().enumerate() {
            let len = seg.text.chars().count();
            if char_off < acc + len {
                if char_off == acc {
                    return i;
                }
                let rel = char_off - acc;
                let chars: Vec<char> = seg.text.chars().collect();
                let left: String = chars[..rel].iter().collect();
                let right: String = chars[rel..].iter().collect();
                let kind = seg.kind;
                self.segments[i] = Segment { kind, text: left };
                self.segments.insert(i + 1, Segment { kind, text: right });
                return i + 1;
            }
            acc += len;
        }
        self.segments.len()
    }

    /// 前端点击 → char offset → 定位光标（= 取消待删选区）。落段内→劈段；落段界→置 gap。clamp [0,len]。
    pub fn set_caret(&mut self, char_off: usize) {
        self.caret_gap = self.split_at(char_off);
        if self.pending_delete.is_some() {
            log::debug!("[select] cleared set_caret t={} off={} range={:?}",
                self.id, char_off, self.pending_delete);
        }
        self.pending_delete = None;
        self.selection_insert_offset = None;
    }

    /// 删除扁平 char 范围 [start,end)。先 split_at(start) 再 split_at(end)，drain 中间段，
    /// caret_gap 落到 start 位置。split_at 幂等（start 已段界不重复劈）。
    fn delete_range(&mut self, start: usize, end: usize) {
        let g1 = self.split_at(start);
        let g2 = self.split_at(end);
        if g1 < g2 {
            self.segments.drain(g1..g2);
        }
        self.caret_gap = g1.min(self.segments.len());
    }

    /// 选中替换：记录待删范围 + 劈 caret_gap 到 start。**不立即删字**（保留浏览器原生
    /// 高亮反馈，用户可重新选择）。apply_engine_full / append_segment 下次调用时消费。
    pub fn set_selection(&mut self, start: usize, end: usize) {
        log::debug!("[select] set t={} range=[{},{}]", self.id, start, end);
        self.pending_delete = Some((start, end));
        self.selection_insert_offset = Some(start);
        self.caret_gap = self.split_at(start);
    }

    /// 当前 caret 在 finish_text 的 char offset（前端光标像素定位用）。
    pub fn caret_char_offset(&self) -> usize {
        let gap = self.caret_gap.min(self.segments.len());
        self.segments[..gap].iter().map(|s| s.text.chars().count()).sum()
    }

    /// 取消选中（失焦 / 新会话）时调用：清 pending_delete 防幽灵删除。
    #[allow(dead_code)]
    pub fn clear_pending_delete(&mut self) {
        if self.pending_delete.is_some() {
            log::debug!("[select] cleared clear_pending_delete t={} range={:?}",
                self.id, self.pending_delete);
        }
        self.pending_delete = None;
        self.selection_insert_offset = None;
    }

    /// 调试用：segments 的紧凑表示（kind:text | kind:text ...）。
    fn debug_segments_str(&self) -> String {
        self.segments.iter()
            .map(|s| match s.kind {
                SegmentKind::Raw => "R",
                SegmentKind::Polished => "P",
                SegmentKind::Edited => "E",
            })
            .zip(self.segments.iter().map(|s| s.text.as_str()))
            .map(|(k, t)| format!("{}:\"{}\"", k, t))
            .collect::<Vec<_>>()
            .join(" | ")
    }

    /// 录音结束收尾：把滞留的 `diverted_pending`（引擎 end-of-stream 纠正，非前缀差异）
    /// 补进 segments / pending_delta。
    ///
    /// `apply_engine_full` 的 diverted 分支延迟确认（< `DIVERTED_PENDING_LIMIT` 字早返回），
    /// 指望「下次 apply 连同补发」。但 stop/close 后不再有下次 apply——若不 flush，
    /// `finalize_*` 只读 `finish_text`（= segments）会把这段纠正**静默丢弃**（末尾文字丢失）。
    /// 与流式内 flush 同语义：polish_pending → pending_delta，否则 push_delta_at_caret。
    pub fn flush_diverted(&mut self) {
        if self.diverted_pending.is_empty() { return; }
        let dp = std::mem::take(&mut self.diverted_pending);
        if self.polish_pending {
            self.pending_delta.push_str(&dp);
        } else {
            self.push_delta_at_caret(&dp);
        }
    }

    /// 是否处于中间插入态（caret_gap < 段数）。pipeline Emit insertion 标志用。
    pub fn is_inserting(&self) -> bool { self.caret_gap < self.segments.len() }

    /// 提交编辑：按 dirty ranges 劈段，dirty 区域标 Edited，区间外保留原 kind。
    /// dirty_ranges 为扁平 char offset 区间（左闭右开），已排序无重叠。
    /// has_edited=false → 整篇保留原 kind（纯删除不改 kind）。
    /// has_edited=true 且 dirty_ranges 空 → 整篇 Edited 兜底。
    pub fn commit_edit(&mut self, flat: &str, dirty_ranges: &[(usize, usize)], has_edited: bool) {
        if self.pending_delete.is_some() {
            log::debug!("[select] cleared commit_edit t={} range={:?}",
                self.id, self.pending_delete);
        }
        self.pending_delete = None;
        self.selection_insert_offset = None;
        if flat.is_empty() { self.segments.clear(); self.caret_gap = 0; return; }
        if dirty_ranges.is_empty() {
            if has_edited {
                self.segments = vec![Segment { kind: SegmentKind::Edited, text: flat.to_string() }];
                self.caret_gap = 1;
            }
            // has_edited=false → 纯删除，不改原段 kind（segments 不变，仅文本缩短已在 flat 中反映）
            // 但 segments 文本需与 flat 同步——用 rebuild_segments 重建（无 dirty = 全 clean）
            else {
                let old_segments = self.segments.clone();
                self.segments = rebuild_segments(&old_segments, flat, &[]);
                self.caret_gap = self.segments.len();
            }
            return;
        }
        let old_segments = self.segments.clone();
        self.segments = rebuild_segments(&old_segments, flat, dirty_ranges);
        self.caret_gap = self.segments.len();
    }

    /// 是否含 Raw 段（mode=2 中间润色触发判定，替代旧 has_increase）。
    pub fn has_raw(&self) -> bool { self.segments.iter().any(|s| s.kind == SegmentKind::Raw) }

    /// 是否有待删选区（set_selection 记录、待首个 delta 消费）。    /// pause-polish 据此跳过：用户选中尚未说话，自动润色不应提前删（守「说话才删」）。
    pub fn has_pending_delete(&self) -> bool { self.pending_delete.is_some() }

    /// 是否含 Edited 段（替代旧 has_edit，PolishDone 落库分支用）。
    pub fn has_edit(&self) -> bool { self.segments.iter().any(|s| s.kind == SegmentKind::Edited) }

    /// 取润色输入：快照 segments + 记 caret offset/末尾态 + 标记 pending。
    pub fn take_polish_input(&mut self) -> PolishInput {
        // 选中替换：润色前删除待删范围，避免快照含选中旧字。
        // 若消费了 pending_delete，强制 polish_caret_at_tail=false——用户显式选定了插入点
        // （selection start），polish 后 caret 须精确恢复到该位置，而非跑到新末尾。
        if let Some((s, e)) = self.pending_delete.take() {
            self.delete_range(s, e);
            log::debug!("[select] consumed take_polish_input t={} range=[{},{}]", self.id, s, e);
        }
        self.polish_snapshot = self.segments.clone();
        self.polish_caret_offset = self.caret_char_offset();
        self.polish_caret_at_tail = self.selection_insert_offset.is_none()
            && self.caret_gap >= self.segments.len();
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
            let target = caret_off.min(total);
            // 不用 set_caret（会清 selection_insert_offset）；手动 split_at 保持选中插入点
            self.caret_gap = self.split_at(target);
            self.selection_insert_offset = Some(target);
        }
        let pending = std::mem::take(&mut self.pending_delta);
        if !pending.is_empty() { self.push_delta_at_caret(&pending); }
        self.last_polish_time = Instant::now();
    }

    /// 润色失败：清 pending；flush pending_delta（保留新语音）。segments 不变。
    pub fn on_polish_failed(&mut self) {
        self.polish_pending = false;
        if self.pending_delete.is_some() {
            log::debug!("[select] cleared on_polish_failed t={} range={:?}",
                self.id, self.pending_delete);
        }
        self.pending_delete = None;
        self.polish_snapshot.clear();
        let pending = std::mem::take(&mut self.pending_delta);
        if !pending.is_empty() { self.push_delta_at_caret(&pending); }
    }

    pub fn polish_pending(&self) -> bool { self.polish_pending }
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

    /// 从 DB JSON 恢复 segments（Idle 态编辑时用——保留已有 Raw/Polished/Edited 标记）。
    /// JSON 格式：[{"kind":"raw|polished|edited","text":"..."}]
    pub fn restore_segments(&mut self, json: &str) {
        if let Ok(arr) = serde_json::from_str::<Vec<serde_json::Value>>(json) {
            self.segments.clear();
            for item in &arr {
                let kind = match item.get("kind").and_then(|v| v.as_str()) {
                    Some("polished") => SegmentKind::Polished,
                    Some("edited") => SegmentKind::Edited,
                    _ => SegmentKind::Raw,
                };
                let text = item.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string();
                if !text.is_empty() {
                    self.segments.push(Segment { kind, text });
                }
            }
            self.caret_gap = self.segments.len();
        }
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

/// 同 kind 相邻段合并（减少碎片）。
fn push_or_merge(result: &mut Vec<Segment>, kind: SegmentKind, text: &str) {
    if text.is_empty() { return; }
    if let Some(last) = result.last_mut() {
        if last.kind == kind {
            last.text.push_str(text);
            return;
        }
    }
    result.push(Segment { kind, text: text.to_string() });
}

/// 按 dirty ranges 重建段列表。
/// dirty 区间内标 Edited；区间外用 LCS diff 对齐 old_flat → new_flat，
/// 保留匹配字符的原 segment kind，不匹配的（因删除导致偏移）标 Raw 兜底。
/// dirty ranges 被 clamp 到 [0, total] 防越界。
fn rebuild_segments(
    old_segments: &[Segment],
    new_flat: &str,
    dirty: &[(usize, usize)],
) -> Vec<Segment> {
    let new_chars: Vec<char> = new_flat.chars().collect();
    let total = new_chars.len();

    // 1. 构建 old_flat + 每个 char 的 kind 映射
    let old_flat: String = old_segments.iter().map(|s| s.text.as_str()).collect();
    let old_chars: Vec<char> = old_flat.chars().collect();
    let old_kinds: Vec<SegmentKind> = {
        let mut v = Vec::with_capacity(old_chars.len());
        for seg in old_segments {
            for _ in 0..seg.text.chars().count() {
                v.push(seg.kind);
            }
        }
        v
    };

    // 2. 对每个 new char 判断是否在 dirty range 内
    let mut is_dirty = vec![false; total];
    for &(raw_start, raw_end) in dirty {
        let s = raw_start.min(total);
        let e = raw_end.min(total);
        if s < e {
            for i in s..e {
                is_dirty[i] = true;
            }
        }
    }

    // 3. 对 non-dirty 区域用 LCS 对齐 old_flat，保留 kind
    // LCS diff：逐字符比较 old_flat 和 new_flat 的 non-dirty 部分
    // 简化方案：对 non-dirty 区域，用字符匹配 walk——按顺序匹配 old_chars 中的对应字符
    let mut result = Vec::new();
    let mut old_idx = 0usize;
    let mut pos = 0usize;

    while pos < total {
        if is_dirty[pos] {
            // dirty 区域——收集连续 dirty chars 标 Edited
            let start = pos;
            while pos < total && is_dirty[pos] { pos += 1; }
            let text: String = new_chars[start..pos].iter().collect();
            push_or_merge(&mut result, SegmentKind::Edited, &text);
        } else {
            // clean 区域——逐字符匹配 old_flat 保留 kind
            let start = pos;
            while pos < total && !is_dirty[pos] { pos += 1; }
            let clean_chars = &new_chars[start..pos];

            // 在 old_chars[old_idx..] 中按序匹配 clean_chars 的每个字符
            for &ch in clean_chars {
                // 跳过 old 中被删除的字符（不在 new 中出现）
                while old_idx < old_chars.len() && old_chars[old_idx] != ch {
                    old_idx += 1;
                }
                if old_idx < old_chars.len() {
                    push_or_merge(&mut result, old_kinds[old_idx], &ch.to_string());
                    old_idx += 1;
                } else {
                    // old 中找不到（不应发生——clean 区域应完全匹配）→ Raw 兜底
                    push_or_merge(&mut result, SegmentKind::Raw, &ch.to_string());
                }
            }
        }
    }

    result
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
    fn apply_engine_full_diverted_buffers_and_replays_next_tick() {
        // diverted（非前缀纠正）→ 当次不展示（buffered），下次前缀 apply 时连同 delta 一次性补发。
        let mut t = Transcript::new(3, PolishMode::Intermediate);
        t.apply_engine_full("你好");
        let changed = t.apply_engine_full("替换全文"); // 非「你好」前缀 = diverted
        assert!(!changed);
        assert_eq!(t.finish_text(), "你好"); // 不回退已展示
        // 下次前缀 apply：pending「替换全文」+ 新 delta「后」拼成「替换全文后」追加
        assert!(t.apply_engine_full("替换全文后"));
        assert_eq!(t.finish_text(), "你好替换全文后");
    }

    #[test]
    fn apply_engine_full_consecutive_diverted_accumulate_pending() {
        // 连续两次 diverted：pending 累积，第三次前缀 apply 一次性补发全部累积。
        let mut t = Transcript::new(31, PolishMode::Intermediate);
        t.apply_engine_full("你好");
        assert!(!t.apply_engine_full("甲乙")); // diverted #1：pending = "甲乙"
        assert_eq!(t.finish_text(), "你好");
        assert!(!t.apply_engine_full("丙丁")); // diverted #2：pending = "甲乙丙丁"
        assert_eq!(t.finish_text(), "你好");
        // 第三次是 "丙丁" 的前缀扩展：delta = "后"，flush pending 在前 → "甲乙丙丁后"
        assert!(t.apply_engine_full("丙丁后"));
        assert_eq!(t.finish_text(), "你好甲乙丙丁后");
    }

    #[test]
    fn apply_engine_full_diverted_then_prefix_combines_pending_and_delta() {
        // diverted pending + 新前缀 delta 正确拼接顺序（pending 在前，delta 在后）。
        let mut t = Transcript::new(32, PolishMode::Intermediate);
        t.apply_engine_full("开头");
        assert!(!t.apply_engine_full("纠正")); // diverted：pending = "纠正"
        assert_eq!(t.finish_text(), "开头");
        // 新 full = "纠正" + "尾巴"：combined = pending("纠正") + delta("尾巴") = "纠正尾巴"
        assert!(t.apply_engine_full("纠正尾巴"));
        assert_eq!(t.finish_text(), "开头纠正尾巴");
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
        t.commit_edit("abcdef", &[], true); // 单 Edited 段，caret_gap=1
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
        t.commit_edit("edited", &[], true); // [Edited], caret_gap=1
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
        t.commit_edit("手改", &[], true);
        assert_eq!(t.segments, vec![edt("手改")]);
        assert_eq!(t.caret_gap, 1);
    }

    #[test]
    fn commit_edit_empty_clears_all() {
        let mut t = Transcript::new(11, PolishMode::Intermediate);
        t.apply_engine_full("raw");
        t.commit_edit("", &[], true);
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
        t.commit_edit("用户编辑", &[], true);
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
        t.commit_edit("flat", &[], true);
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

    // ── 选中替换（delete_range / set_selection / pending_delete）──
    #[test]
    fn delete_range_basic() {
        // "你好世界再见" 删 [2,4)（"世界"）→ "你好再见"，caret 落 "你好" 后。
        let mut t = Transcript::new(101, PolishMode::Intermediate);
        t.apply_engine_full("你好世界再见"); // 单 Raw 段
        t.delete_range(2, 4);
        assert_eq!(t.finish_text(), "你好再见");
        assert_eq!(t.caret_char_offset(), 2); // "你好" 后
    }

    #[test]
    fn delete_range_spans_segments() {
        // 多段跨段删除：[raw 甲][edited 乙丙][raw 丁]，删 [1,3)（"乙丙"）→ "甲丁"。
        let mut t = Transcript::new(102, PolishMode::Intermediate);
        t.segments = vec![raw("甲"), edt("乙丙"), raw("丁")];
        t.delete_range(1, 3);
        assert_eq!(t.finish_text(), "甲丁");
    }

    #[test]
    fn set_selection_then_first_delta_replaces() {
        // 拖选 [2,4)（"世界"）→ 首词到达时删选中、新词从 start 插、caret 跟随增长。
        let mut t = Transcript::new(103, PolishMode::Intermediate);
        t.apply_engine_full("你好世界");
        t.set_selection(2, 4);
        assert_eq!(t.caret_char_offset(), 2); // 选中态 caret 在 start
        let off_before = t.caret_char_offset();
        assert!(t.apply_engine_full("你好世界新词"));
        assert_eq!(t.finish_text(), "你好新词"); // "世界" 被删、"新词" 插入
        assert!(t.caret_char_offset() > off_before); // caret 随插入右移
    }

    #[test]
    fn set_selection_then_first_append_segment_replaces() {
        // VadSegmented / cloud partial 路径（append_segment）须与流式路径对称：
        // 拖选 [2,4)（"世界"）→ 首词到达时删选中、新词从 start 插。
        let mut t = Transcript::new(105, PolishMode::Intermediate);
        t.apply_engine_full("你好世界"); // 建初始文本（单 Raw 段）
        t.set_selection(2, 4);
        assert_eq!(t.caret_char_offset(), 2); // 选中态 caret 在 start
        t.append_segment("新词"); // VadSegmented 首词
        assert_eq!(t.finish_text(), "你好新词"); // "世界" 被删、"新词" 插入
    }

    #[test]
    fn set_caret_clears_pending_delete() {
        // 选中后 set_caret 取消 → clear_pending_delete；后续 apply 不删
        let mut t = Transcript::new(104, PolishMode::Intermediate);
        t.apply_engine_full("你好世界");
        t.set_selection(2, 4);
        t.set_caret(0); // 取消选区
        t.apply_engine_full("你好世界后"); // delta "后"，不删
        assert_eq!(t.finish_text(), "后你好世界"); // "世界" 保留（证明未删）
    }

    #[test]
    fn pending_delete_consumed_once() {
        // 首次 apply 消费待删；第二次不再删。
        let mut t = Transcript::new(105, PolishMode::Intermediate);
        t.apply_engine_full("你好世界");
        t.set_selection(2, 4);
        // 静音 tick（same full）：消费 pending_delete 删"世界"，delta 空 → 返回 true
        let changed = t.apply_engine_full("你好世界");
        assert!(changed, "选区删除应返回 true");
        assert_eq!(t.finish_text(), "你好");
        // 新词到达
        t.apply_engine_full("你好世界甲"); // delta "甲" → "你好甲"
        assert_eq!(t.finish_text(), "你好甲");
        t.apply_engine_full("你好世界甲乙"); // delta "乙" → "你好甲乙"
        assert_eq!(t.finish_text(), "你好甲乙");
    }

    #[test]
    fn pending_delete_consumed_in_take_polish_input() {
        // 选中后润色：take_polish_input 先删待删区，快照基于删后文本。
        let mut t = Transcript::new(106, PolishMode::Intermediate);
        t.apply_engine_full("你好世界");
        t.set_selection(2, 4);
        let input = t.take_polish_input();
        assert_eq!(input.segments.len(), 1);
        assert_eq!(input.segments[0].text, "你好");
        assert!(!t.polish_snapshot.iter().any(|s| s.text.contains("世界")));
    }

    #[test]
    fn selection_then_polish_then_delta_inserts_at_selection_start() {
        // 停顿触发润色后 caret 恢复到选中起点（selection_insert_offset）。
        let mut t = Transcript::new(108, PolishMode::Intermediate);
        t.apply_engine_full("你好世界");
        t.set_selection(2, 4);
        // 模拟静音 tick → 消费 pending_delete
        t.apply_engine_full("你好世界"); // same → 删"世界"
        assert_eq!(t.finish_text(), "你好");
        let _input = t.take_polish_input();
        t.polish_apply("你好。"); // LLM 润色
        assert_eq!(t.caret_char_offset(), 2, "caret 须恢复到选中起点");
        t.apply_engine_full("你好世界新词"); // delta = skip 4 = "新词"
        assert_eq!(t.finish_text(), "你好新词。", "新词须从选中起点生长");
    }

    #[test]
    fn selection_at_start_then_polish_then_delta_inserts_at_start() {
        // 选中开头 → 首次 apply 删选区 → 润色 → 新词从开头生长。
        let mut t = Transcript::new(109, PolishMode::Intermediate);
        t.apply_engine_full("你好世界");
        t.set_selection(0, 2);
        t.apply_engine_full("你好世界"); // same → 删"你好"
        assert_eq!(t.finish_text(), "世界");
        t.apply_engine_full("你好世界新"); // delta "新" → "新世界"
        assert_eq!(t.finish_text(), "新世界");
        let _input = t.take_polish_input();
        t.polish_apply("新。世界。");
        assert_ne!(t.caret_gap, t.segments.len(), "caret 不能在末尾");
        t.apply_engine_full("你好世界新词"); // delta "词"
        let result = t.finish_text();
        assert!(result.starts_with("新词"), "新词须在开头生长，实际: {}", result);
    }

    #[test]
    fn selection_then_engine_new_phrase_not_diverted() {
        // 活跃会话选中替换完整流程
        let mut t = Transcript::new(110, PolishMode::Intermediate);
        t.apply_engine_full("你好世界");
        assert_eq!(t.finish_text(), "你好世界");
        t.set_selection(0, 2); // pending_delete
        // 静音 tick → 消费 pending_delete
        let changed = t.apply_engine_full("你好世界"); // same full → delta 空
        assert!(changed, "选区删除应返回 true");
        assert_eq!(t.finish_text(), "世界");
        // 引擎前缀续作 + 新词
        t.apply_engine_full("你好世界，欢迎来到");
        assert_eq!(t.finish_text(), "，欢迎来到世界");
    }

    #[test]
    fn selection_mid_text_then_engine_new_phrase_replaces() {
        let mut t = Transcript::new(111, PolishMode::Intermediate);
        t.apply_engine_full("你好世界再见");
        t.set_selection(2, 4);
        // 静音 tick → 消费 pending_delete 删"世界"
        t.apply_engine_full("你好世界再见"); // same → 删
        assert_eq!(t.finish_text(), "你好再见");
        // 新词
        t.apply_engine_full("你好世界再见，欢迎"); // delta "，欢迎"
        assert_eq!(t.finish_text(), "你好，欢迎再见", "新词从选中起点生长");
    }

    #[test]
    fn selection_deleted_on_first_engine_tick() {
        // pending_delete 在首次 apply 即时消费（不等非空 delta）。
        let mut t = Transcript::new(112, PolishMode::Intermediate);
        t.apply_engine_full("你好世界");
        t.set_selection(0, 2);
        // 静音 tick（same full）
        let changed = t.apply_engine_full("你好世界");
        assert!(changed, "选区删除须返回 true 即使 delta 空");
        assert_eq!(t.finish_text(), "世界", "选区在首次 apply 即时删除");
        // 新语音
        t.apply_engine_full("你好世界新"); // delta "新"
        assert_eq!(t.finish_text(), "新世界", "新词从选中起点生长");
    }

    #[test]
    fn db_flush_due_respects_threshold() {
        let mut t = Transcript::new(107, PolishMode::Intermediate);
        // 未落库过 → due（即便 threshold 很大）
        assert!(t.db_flush_due(Duration::from_secs(3600)));
        t.mark_db_written();
        // threshold 0 → 总 due（elapsed ≥ 0）
        assert!(t.db_flush_due(Duration::from_millis(0)));
        // threshold 远大于已 elapsed（刚写入）→ not due
        assert!(!t.db_flush_due(Duration::from_secs(3600)));
    }

    #[test]
    fn diverted_pending_flushes_when_over_limit() {
        // 引擎持续纠正（每次 full 非前缀）→ diverted_pending 累积，超 DIVERTED_PENDING_LIMIT
        // 后强制 flush 展示，避免用户看空白。
        let mut t = Transcript::new(108, PolishMode::Intermediate);
        t.apply_engine_full("基"); // engine_cumulative="基", shown="基"
        // 每次「基+递增数字」非 engine_cumulative 前缀 → 持续 diverted 累积
        for i in 0..400 {
            t.apply_engine_full(&format!("基{}", i));
        }
        // 超限（500 char）后强制 flush → finish_text 增长（不再卡在"基"）
        assert!(
            t.finish_text().chars().count() > 100,
            "diverted 超限应强制 flush，finish_text={}",
            t.finish_text()
        );
    }
}


#[cfg(test)]
mod user_scenario_tests {
    use super::*;
    use crate::config::PolishMode;

    #[test]
    fn user_scenario_select_hello_speak_welcome() {
        // 用户场景：原文"你好新世界"，选中"你好"，说"欢迎来到"
        let mut t = Transcript::new(200, PolishMode::Intermediate);
        t.apply_engine_full("你好新世界");
        assert_eq!(t.finish_text(), "你好新世界");
        // 选中 "你好" [0,2) — 不立即删
        t.set_selection(0, 2);
        assert_eq!(t.finish_text(), "你好新世界", "选中不立即删");
        // 静音 tick → 消费 pending_delete
        let changed = t.apply_engine_full("你好新世界"); // same → 删"你好"
        assert!(changed);
        assert_eq!(t.finish_text(), "新世界");
        // 新语音（Zipformer accumulated + separator + new segment）
        t.apply_engine_full("你好新世界，欢"); // delta "，欢"
        assert_eq!(t.finish_text(), "，欢新世界");
        t.apply_engine_full("你好新世界，欢迎");
        assert_eq!(t.finish_text(), "，欢迎新世界");
        t.apply_engine_full("你好新世界，欢迎来到");
        assert_eq!(t.finish_text(), "，欢迎来到新世界");
    }

    #[test]
    fn user_scenario_select_mid_speak_welcome() {
        // 选中中间文本替换
        let mut t = Transcript::new(201, PolishMode::Intermediate);
        t.apply_engine_full("你好新世界");
        t.set_selection(2, 5); // 选中 "新世界"
        // 静音 tick → 消费
        t.apply_engine_full("你好新世界"); // same → 删
        assert_eq!(t.finish_text(), "你好");
        // 新语音
        t.apply_engine_full("你好新世界，欢迎来到"); // delta "，欢迎来到"
        assert_eq!(t.finish_text(), "你好，欢迎来到", "中间选区替换");
    }

    // ── diverted 末尾丢失（Bug：finalize 丢弃滞留 diverted_pending）──
    #[test]
    fn diverted_pending_flushed_not_dropped() {
        // 末尾引擎纠正（非前缀，< 500 字）→ diverted 分支早返回，diff 滞留 diverted_pending。
        // 修复前 finalize 只读 segments（finish_text）→ 纠正被静默丢弃。
        let mut t = Transcript::new(110, PolishMode::Intermediate);
        t.apply_engine_full("你好世"); // engine_cumulative="你好世"，segments=[raw("你好世")]
        // 引擎 end-of-stream 纠正：与最后 partial 发散（"界"≠"世"，非前缀）→ diverted，
        // diff="界" 滞留（< 500 字早返回，未入 segments）。
        t.apply_engine_full("你好界");
        assert!(!t.finish_text().contains('界'),
            "修复前 diverted 应滞留、未进 segments（finish_text={}）", t.finish_text());

        // finalize 顶部调用 flush_diverted → 滞留纠正补回，不再丢弃。
        t.flush_diverted();
        assert!(t.finish_text().contains('界'),
            "flush 后 diverted 纠正应保留（finish_text={}）", t.finish_text());
        assert!(t.diverted_pending.is_empty(), "flush 后 diverted_pending 应清空");
    }

    #[test]
    fn diverted_pending_flushed_into_pending_delta_when_polish_pending() {
        // polish_pending 时 flush 走 pending_delta（润色回填后由 polish_apply 统一 flush），
        // 不直接改 segments——避免与润色快照竞争。
        let mut t = Transcript::new(111, PolishMode::Intermediate);
        t.apply_engine_full("你好世");
        t.apply_engine_full("你好界"); // diverted，"界" 滞留
        let _ = t.take_polish_input(); // 标记 polish_pending
        let before = t.finish_text();
        t.flush_diverted();
        // segments 不变（进了 pending_delta，待 polish_apply）；diverted 已清。
        assert_eq!(t.finish_text(), before, "polish_pending 时不应直接改 segments");
        assert!(t.diverted_pending.is_empty());
    }

    // ── commit_edit with dirty ranges ──

    #[test]
    fn commit_edit_with_dirty_ranges_marks_edited() {
        let mut t = Transcript::new(1, PolishMode::Disabled);
        t.append_segment("你好世界");
        // 用户在 offset 2 插入"朋友"（dirty [2,4)），offset 5-7 改为"再见"（dirty [5,7)）
        t.commit_edit("你好朋友世再见", &[(2, 4), (5, 7)], true);
        let segs = &t.segments;
        // Raw("你好") + Edited("朋友") + Raw("世") + Edited("再见")
        assert_eq!(segs[0].kind, SegmentKind::Raw);
        assert_eq!(segs[0].text, "你好");
        assert_eq!(segs[1].kind, SegmentKind::Edited);
        assert_eq!(segs[1].text, "朋友");
        assert_eq!(segs[2].kind, SegmentKind::Raw);
        assert_eq!(segs[2].text, "世");
        assert_eq!(segs[3].kind, SegmentKind::Edited);
        assert_eq!(segs[3].text, "再见");
    }

    #[test]
    fn commit_edit_empty_dirty_ranges_fallback_all_edited() {
        let mut t = Transcript::new(1, PolishMode::Disabled);
        t.append_segment("你好");
        t.commit_edit("你好修改", &[], true);
        assert_eq!(t.segments.len(), 1);
        assert_eq!(t.segments[0].kind, SegmentKind::Edited);
    }

    #[test]
    fn commit_edit_empty_text_clears_segments() {
        let mut t = Transcript::new(1, PolishMode::Disabled);
        t.append_segment("你好");
        t.commit_edit("", &[(0, 0)], true);
        assert!(t.segments.is_empty());
    }

    #[test]
    fn rebuild_segments_preserves_clean_kind() {
        let old = vec![
            Segment { kind: SegmentKind::Raw, text: "AB".into() },
            Segment { kind: SegmentKind::Polished, text: "CD".into() },
        ];
        let result = rebuild_segments(&old, "ABCD", &[(2, 4)]);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].kind, SegmentKind::Raw);
        assert_eq!(result[0].text, "AB");
        assert_eq!(result[1].kind, SegmentKind::Edited);
        assert_eq!(result[1].text, "CD");
    }

    #[test]
    fn rebuild_segments_multiple_dirty_ranges() {
        let old = vec![
            Segment { kind: SegmentKind::Raw, text: "ABCDEF".into() },
        ];
        let result = rebuild_segments(&old, "ABCDEF", &[(1, 2), (4, 5)]);
        assert_eq!(result.len(), 5);
        assert_eq!(result[0], Segment { kind: SegmentKind::Raw, text: "A".into() });
        assert_eq!(result[1], Segment { kind: SegmentKind::Edited, text: "B".into() });
        assert_eq!(result[2], Segment { kind: SegmentKind::Raw, text: "CD".into() });
        assert_eq!(result[3], Segment { kind: SegmentKind::Edited, text: "E".into() });
        assert_eq!(result[4], Segment { kind: SegmentKind::Raw, text: "F".into() });
    }

    #[test]
    fn segment_kind_at_offset_correct() {
        let segs = vec![
            Segment { kind: SegmentKind::Raw, text: "AB".into() },
            Segment { kind: SegmentKind::Polished, text: "CD".into() },
            Segment { kind: SegmentKind::Edited, text: "EF".into() },
        ];
        assert_eq!(segment_kind_at_offset(&segs, 0), SegmentKind::Raw);
        assert_eq!(segment_kind_at_offset(&segs, 1), SegmentKind::Raw);
        assert_eq!(segment_kind_at_offset(&segs, 2), SegmentKind::Polished);
        assert_eq!(segment_kind_at_offset(&segs, 3), SegmentKind::Polished);
        assert_eq!(segment_kind_at_offset(&segs, 4), SegmentKind::Edited);
        assert_eq!(segment_kind_at_offset(&segs, 5), SegmentKind::Edited);
        assert_eq!(segment_kind_at_offset(&segs, 99), SegmentKind::Raw);
    }

    #[test]
    fn push_or_merge_same_kind() {
        let mut result = vec![Segment { kind: SegmentKind::Raw, text: "AB".into() }];
        push_or_merge(&mut result, SegmentKind::Raw, "CD");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].text, "ABCD");
        push_or_merge(&mut result, SegmentKind::Edited, "EF");
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn rebuild_clean_range_spans_multiple_kinds() {
        // old: [Raw("AB"), Polished("CD")] → dirty [4,6)（在末尾加"EF"）
        // clean 区域 "ABCD" 跨越 Raw + Polished → 应各自保留 kind
        let old = vec![
            Segment { kind: SegmentKind::Raw, text: "AB".into() },
            Segment { kind: SegmentKind::Polished, text: "CD".into() },
        ];
        let result = rebuild_segments(&old, "ABCDEF", &[(4, 6)]);
        // Raw("AB") + Polished("CD") + Edited("EF")
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].kind, SegmentKind::Raw);
        assert_eq!(result[0].text, "AB");
        assert_eq!(result[1].kind, SegmentKind::Polished);
        assert_eq!(result[1].text, "CD");
        assert_eq!(result[2].kind, SegmentKind::Edited);
        assert_eq!(result[2].text, "EF");
    }

    #[test]
    fn rebuild_pure_delete_preserves_segment_kinds() {
        // old: [Raw("A"), Polished("B")] → 用户删除 "A" → new = "B"
        // 无 dirty ranges，has_edited=false → rebuild_segments 保留原 kind
        let old = vec![
            Segment { kind: SegmentKind::Raw, text: "A".into() },
            Segment { kind: SegmentKind::Polished, text: "B".into() },
        ];
        let result = rebuild_segments(&old, "B", &[]);
        // "B" 应保持 Polished（不退化为 Raw）
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].kind, SegmentKind::Polished);
        assert_eq!(result[0].text, "B");
    }

    #[test]
    fn rebuild_mid_delete_preserves_remaining_kinds() {
        // old: [Raw("A"), Polished("B"), Edited("C")] → 用户删除中间 "B" → new = "AC"
        // has_edited=true, dirty_ranges=[] → 整篇 Edited 兜底（删除不改 kind 无法精确保留）
        // 但 has_edited=false + dirty_ranges=[] 时走 rebuild_segments
        // 此时 new_flat="AC" 在 old_flat="ABC" 中不是连续子串
        // old_chars walk: A 匹配 old[0]=Raw, C 匹配 old[2]=Edited（跳过被删的 B）
        let old = vec![
            Segment { kind: SegmentKind::Raw, text: "A".into() },
            Segment { kind: SegmentKind::Polished, text: "B".into() },
            Segment { kind: SegmentKind::Edited, text: "C".into() },
        ];
        let result = rebuild_segments(&old, "AC", &[]);
        let actual_text: String = result.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(actual_text, "AC", "文本必须等于 new_flat");
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].kind, SegmentKind::Raw);
        assert_eq!(result[0].text, "A");
        assert_eq!(result[1].kind, SegmentKind::Edited);
        assert_eq!(result[1].text, "C");
    }
}
