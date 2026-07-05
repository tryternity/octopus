# ASR 光标定位与中间插入 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Result 语音识别窗非编辑态显示闪烁光标、可点击定位；新语音从光标处实时流式插入（中插），显示/落库/复制一致。

**Architecture:** 废弃 `raw/polished/edited` 三字段 + `edited≻polished≻raw` 优先级链，改为类型化 `Vec<Segment>`（`Raw`/`Polished`/`Edited`）+ `caret_gap`（新语音生长缝隙）。`finish_text()` 段扁平化为唯一展示/落库文本。润色改全篇一次调用（edited 冻结、raw/polished 重润，best-effort 串匹配回填）。前端自定义闪烁光标（非 contentEditable）+ 点击 char offset 定位。

**Tech Stack:** Rust（desktop crate：transcript/pipeline/coordinator/result_window；infra crate：db；llm crate：polish）；React/TypeScript（Result 页）；SQLite（transcriptions v13→v14）。

**关键约束（worktree）：** 工作目录是 worktree `clean-used-feature`。所有 cargo/grep/git 命令须显式指向 worktree（`--manifest-path crates/desktop/Cargo.toml`、绝对路径、`git -C`）。Bash cwd 实测是主仓库，**不可**依赖相对路径。`config/` 是软链接，读写用绝对路径 `~/.octopus/`。

**spec：** `docs/superpowers/specs/2026-07-03-asr-cursor-insert-design.md`（已批准 commit `2e64efe`）

**状态：** ✅ 全部完成——53 步 checkbox 全勾，中插 8 任务（段模型基石 `c20eb35` / `set_caret` `f2ca142`）+ 选中替换追加（§11，`b961f8e`）+ 前端渲染修复（§12-§14）全合入 main。关键修复：`9d4a654`（`append_segment` 漏消费 `pending_delete`，离线/cloud 引擎选中替换失效）、`f32f1a9`（编辑保存后光标归末尾）、`e797e0f`（前端 vitest 单测基建）。

---

## 文件结构

| 文件 | 责任 | 改动 |
|---|---|---|
| `crates/desktop/src/transcript.rs` | 段模型状态机（核心真相源） | **重写**：新结构 + 新方法 + 新单测（~25 旧测替换） |
| `crates/llm/src/client.rs` | LLM 调用 | 新增 `polish_regions`（多段润色） |
| `crates/llm/src/prompt.rs` | prompt 构造 | 新增 `regions_prompt`（edited 标记保留） |
| `crates/llm/src/lib.rs` | re-export | 导出 `polish_regions`/`PolishRegion` |
| `crates/desktop/src/pipeline.rs` | 流式/分段 pipeline | `tick` 用 `apply_engine_full`；`Emit` 加 `insertion` 标志 |
| `crates/desktop/src/coordinator.rs` | 编排 | 调用点全适配 + `set_caret` 命令 + `spawn_polish_thread` 新协议 + `PolishDone` 走 `polish_apply` |
| `crates/desktop/src/result_window.rs` | 结果窗 emit | `update_result` 加 `insertion`，payload 改对象 |
| `crates/infra/src/db.sql` | 建表 DDL | `transcriptions` 加 `segments`/`text` 列 |
| `crates/infra/src/db.rs` | 迁移 + CRUD | v14 迁移（旧三列→单段）+ insert/update/finalize/list/search 改 |
| `crates/desktop/frontend/src/pages/Result/index.tsx` | 前端 | 自定义闪烁光标 + 点击定位 + `update-result` handler 改对象读 insertion |

## 任务依赖与里程碑

```
Task 1 (transcript 重写 + 全调用点编译适配，零回归)
  ├─ Task 2 (pipeline insertion)
  ├─ Task 3 (llm 多段润色)
  │     └─ Task 4 (coordinator: set_caret + 润色新协议 + 落库)
  ├─ Task 5 (result_window insertion)
  ├─ Task 6 (db v14 + CRUD)
  └─ Task 7 (前端光标)
        └─ Task 8 (e2e 集成)
```

- **Task 1 是基石**：重写 transcript 会破坏 coordinator/pipeline 对旧方法的引用，故 Task 1 必须一并做「最小编译适配」（旧调用点改用新等价方法，**行为零回归，不引入 caret/insertion 新功能**），使 `cargo build` 通过、新单测绿。这是核心数据结构替换，无法纯增量——Task 1 内部步骤连续、统一 commit。
- Task 2–7 在编译通过的基础上各自独立、可测、可 commit。Task 2/3/5/6 互不依赖可并行，Task 4 依赖 3（润色协议），Task 7 依赖 2+5（insertion 链路）。
- **每 Task 完成跑 `cargo test -p octopus-desktop` + `cargo test -p octopus-infra` + `cargo test -p octopus-llm`**（按涉及 crate），全绿才进下一 Task。

---

## Task 1: Transcript 段模型重写（基石，零回归）

**Files:**
- Rewrite: `crates/desktop/src/transcript.rs`
- Modify（编译适配）: `crates/desktop/src/pipeline.rs`、`crates/desktop/src/coordinator.rs`

**目标行为不变量**（不点光标时逐字一致）：
- `segments=[]` + `caret_gap=0` ≡ 旧空文档。
- `caret_gap==segments.len()` ≡ 旧「末尾追加」。
- `apply_engine_full` 正常前缀追加 ≡ 旧 `set_full` 末尾增长 + `display_text`。

### Step 1.1: 写 transcript.rs 新结构与核心只读方法 + 测试

- [x] **写失败测试**（先整文件替换，保留 `#[cfg(test)] mod tests` 占位空实现会让旧测试消失；本步连同实现一起写，测试随实现）。

整文件替换 `crates/desktop/src/transcript.rs` 为以下完整内容（含 `#[cfg(test)] mod tests`，测试随实现一并写入，不再分步）：

```rust
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
    last_polish_time: Instant,
    polish_pending: bool,
    /// 润色发起时的 segments 快照（PolishDone 回填比对用）。
    polish_snapshot: Vec<Segment>,
    /// 润色发起时的 caret char offset（PolishDone 后恢复光标到同位置）。
    polish_caret_offset: usize,
    /// pending 期间缓存的新 delta（pending 不写 segments，PolishDone 后 flush）。
    pending_delta: String,
    db_inserted: bool,
}

impl Transcript {
    pub fn new(id: i64, mode: PolishMode) -> Self {
        Self {
            id, mode, segments: Vec::new(), caret_gap: 0,
            engine_cumulative: String::new(), engine_consumed_chars: 0,
            last_polish_time: Instant::now(), polish_pending: false,
            polish_snapshot: Vec::new(), polish_caret_offset: 0,
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
    /// diverted（非前缀）→ 重算基准、丢弃本次 delta（不回退已展示），同现状容忍。
    pub fn apply_engine_full(&mut self, full: &str) -> bool {
        let delta = if full.starts_with(self.engine_cumulative.as_str()) {
            full.chars().skip(self.engine_consumed_chars).collect::<String>()
        } else {
            self.engine_cumulative = full.to_string();
            self.engine_consumed_chars = full.chars().count();
            return false;
        };
        self.engine_cumulative = full.to_string();
        self.engine_consumed_chars = full.chars().count();
        if delta.is_empty() { return false; }
        if self.polish_pending { self.pending_delta.push_str(&delta); }
        else { self.push_delta_at_caret(&delta); }
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

    /// 取润色输入：快照 segments + 记 caret offset + 标记 pending。
    pub fn take_polish_input(&mut self) -> PolishInput {
        self.polish_snapshot = self.segments.clone();
        self.polish_caret_offset = self.caret_char_offset();
        self.polish_pending = true;
        PolishInput { segments: self.polish_snapshot.clone() }
    }

    /// 润色完成回填：snapshot 的 edited 段在 full 里串匹配定位 → Edited；间隙 → Polished。
    /// 恢复 caret 到发起时的 char offset；flush pending_delta。
    pub fn polish_apply(&mut self, full: &str) {
        let snapshot = std::mem::take(&mut self.polish_snapshot);
        let caret_off = self.polish_caret_offset;
        self.polish_pending = false;
        self.segments = rebuild_after_polish(&snapshot, full);
        let total = self.finish_text().chars().count();
        self.set_caret(caret_off.min(total));
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
```

### Step 1.2: 适配 pipeline.rs（编译通过，行为零回归）

- [x] **改 `StreamingPipeline::tick` 的 changed 判定 + set_full**

`crates/desktop/src/pipeline.rs` 中 `StreamingPipeline::tick`（约 L207-259），把：

```rust
TranscriptEvent::Partial(text) | TranscriptEvent::Committed(text) => {
    if text != transcript.full() {
        transcript.set_full(&text);
        changed = true;
    }
}
TranscriptEvent::Final(text) => {
    transcript.set_full(&text);
    changed = true;
}
```

替换为：

```rust
TranscriptEvent::Partial(text) | TranscriptEvent::Committed(text) => {
    if transcript.apply_engine_full(&text) { changed = true; }
}
TranscriptEvent::Final(text) => {
    transcript.apply_engine_full(&text);
    changed = true;
}
```

> `transcript.full()`/`display_text()` 仍存在（alias），其余引用（cloud display 拼接 L242-248、drain_rx L446/453）不改。

### Step 1.3: 适配 coordinator.rs（编译通过，行为零回归）

coordinator.rs 有 ~20 处 transcript 旧方法调用。**机械替换规则**（不改逻辑，仅换方法名/签名）：

| 旧调用 | 新调用 | 说明 |
|---|---|---|
| `transcript.full().is_empty()` | `transcript.finish_text().is_empty()` | full 仍可保留，但统一用 finish_text |
| `transcript.full()`（非 is_empty） | `transcript.finish_text()` | 同上 |
| `transcript.db_text()` | `transcript.finish_text()` | db_text 仍 alias，统一 finish_text |
| `transcript.display_text()` | 保留（== finish_text） | 无需改 |
| `transcript.has_increase()` | `transcript.has_raw()` | L1339 |
| `transcript.edited_display().unwrap_or_else(\|\| transcript.db_text())` | `transcript.finish_text()` | L827/830/896；段模型 finish_text 已含 edited |
| `transcript.set_full(&final_text)` (L834) | `transcript.apply_engine_full(&final_text)` | stop 路径喂尾帧 |
| `transcript.take_polish_input()` → `(preserved, to_polish)` | `transcript.take_polish_input()` → `PolishInput` | 见下 spawn_polish_thread（Task 4 深改；Task 1 先让编译过：临时把 spawn_polish_thread 改接收 PolishInput 但内部仍走旧 polish——见 Step 1.4） |
| `transcript.on_polish_done(s)` | `transcript.polish_apply(&s)` | L1621/1685 |
| `transcript.on_polish_failed()` | `transcript.on_polish_failed()` | 签名不变 |
| `transcript.polished()` (L894/1456/1630/1696) | 见 Step 1.5 | |
| `transcript.commit_edit(text)` | 保留（语义已改） | 无需改签名 |
| `transcript.append_segment(...)` | 保留 | 无需改 |

- [x] **执行替换并验证无遗漏**

```bash
cd /Users/wudarui/workspace/agent/octopus/.claude/worktrees/clean-used-feature
grep -n "has_increase\|\.set_full\|edited_display\|on_polish_done\b" crates/desktop/src/coordinator.rs
# 期望：无 has_increase / set_full / edited_display / on_polish_done 残留（on_polish_failed 保留）
```

### Step 1.4: spawn_polish_thread 临时适配（让 Task 1 编译过；Task 4 深改）

Task 1 阶段 `spawn_polish_thread` 与 `octopus_llm::polish` 协议暂不动（保持单段），但 `take_polish_input` 返回类型变了。**临时桥接**（Task 4 会重写）：

`coordinator.rs` `spawn_polish_thread`（约 L1291）签名改接收 `PolishInput`，内部把 segments 折回旧 `(preserved, to_polish)`：

```rust
fn spawn_polish_thread(
    input: crate::transcript::PolishInput,
    config: &AppConfig,
    tx: &Sender<Command>,
    ignore_mode: bool,
    session_id: i64,
) {
    // Task 1 临时桥接：把 segments 折成旧 preserved+to_polish（Task 4 改 polish_regions 多段）
    let preserved: Option<String> = input.segments.iter()
        .find(|s| s.kind == crate::transcript::SegmentKind::Edited)
        .map(|s| s.text.clone());
    let to_polish: String = input.segments.iter()
        .filter(|s| s.kind != crate::transcript::SegmentKind::Edited)
        .map(|s| s.text.as_str()).collect();
    let llm_config = if ignore_mode {
        crate::config::llm_config_ignore_mode(config)
    } else {
        crate::config::llm_config(config)
    };
    let llm_config = match llm_config { Some(c) => c, None => return };
    let tx = tx.clone();
    std::thread::spawn(move || {
        let result = match octopus_llm::polish(preserved.as_deref(), &to_polish, &llm_config) {
            Ok(polished) => Ok(polished),
            Err(e) => { log::warn!("Polish thread error: {}", e); Err(e.to_string()) }
        };
        let _ = tx.send(Command::PolishDone { result, session_id });
    });
}
```

调用点（L1355、L1770）改：

```rust
let input = transcript.take_polish_input();
spawn_polish_thread(input, config, tx, false, transcript.id);
// L1770（final）: spawn_polish_thread(input, config, tx, true, transcript.id);
```

> 临时桥接在「仅 0 或 1 个 Edited 段 + 非 edited 连续」时与旧逻辑等价（零回归）。多 Edited 段场景的精确处理在 Task 4 落地。

### Step 1.5: 处理 `transcript.polished()` 残留（L894/1456/1630/1696）

`polished()` 已删。这几处语义：

- **L1630/1696**（PolishDone handler 内，`text: transcript.polished().to_string()` 给 `update_polished` DB 写）：Task 1 改为 `text: transcript.finish_text()`（润色后整篇）。
- **L894**（`skip_final_polish = !transcript.polished().is_empty() && !transcript.has_edit()`，stop 路径判是否跳过最终润色）：段模型下「已润色」= 无 Raw 段且非空。改为 `let skip_final_polish = !transcript.finish_text().is_empty() && !transcript.has_raw();`
- **L1456**（`let p = t.polished();` 注释场景，history 回放）：改为读 `finish_text()` 或按需调整。

- [x] **逐处替换**，并 grep 验证：

```bash
grep -n "\.polished()" crates/desktop/src/coordinator.rs
# 期望：0 残留
```

### Step 1.6: 编译 + 跑测试

- [x] **编译 desktop crate**

```bash
cd /Users/wudarui/workspace/agent/octopus/.claude/worktrees/clean-used-feature
cargo build -p octopus-desktop 2>&1 | tail -30
```
Expected: 编译通过（可能 cloud feature 警告，忽略）。若有 `has_edit`/`increase` 等旧方法残留报错，按替换表补。

- [x] **跑 transcript + pipeline 单测**

```bash
cargo test -p octopus-desktop transcript 2>&1 | tail -20
cargo test -p octopus-desktop pipeline 2>&1 | tail -20
```
Expected: transcript 全绿（新 ~20 测）；pipeline 既有测绿（FakePipelineEngine 走 apply_engine_full）。

> pipeline 既有测 `tick_partial_updates_transcript_and_signals_changed` 等断言 `t.full()`——`full()` 保留为 alias，绿。`tick_committed_idempotent_no_change_skip`：Committed 与 full 同 → apply_engine_full 返回 false → changed=false → 只产 Polish，绿。

### Step 1.7: Commit

- [x] **提交**

```bash
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/clean-used-feature add crates/desktop/src/transcript.rs crates/desktop/src/pipeline.rs crates/desktop/src/coordinator.rs
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/clean-used-feature commit -m "refactor(asr): Transcript 改段模型（segments+caret_gap），全调用点适配，零回归"
```

---

## Task 2: pipeline.rs 加 insertion 标志

**Files:** Modify `crates/desktop/src/pipeline.rs`

### Step 2.1: PipelineEvent::Emit 加 insertion 字段

- [x] **改 enum 定义**（约 L28-41）

```rust
#[derive(Debug, PartialEq)]
pub enum PipelineEvent {
    PersistRaw { engine_mode: &'static str },
    /// display 已算好；insertion=true 表示中间插入态（前端立即渲染，跳过 300ms diverted 延迟）。
    Emit { display: String, insertion: bool },
    Polish { silence: f64 },
    Error(String),
}
```

### Step 2.2: 所有 Emit 构造点加 insertion

- [x] **改 StreamingPipeline::tick**（local L253、cloud L249）与 **VadSegmentedPipeline::tick**（L529）

local 分支：

```rust
if changed {
    events.push(PipelineEvent::PersistRaw { engine_mode: "streaming" });
    events.push(PipelineEvent::Emit { display: transcript.display_text(), insertion: transcript.is_inserting() });
}
```

cloud 分支（display 含 partial）：

```rust
events.push(PipelineEvent::Emit { display, insertion: transcript.is_inserting() });
```

VadSegmented：

```rust
if changed {
    events.push(PipelineEvent::PersistRaw { engine_mode: "vad_segmented" });
    events.push(PipelineEvent::Emit { display: transcript.display_text(), insertion: transcript.is_inserting() });
}
```

### Step 2.3: 更新 pipeline 单测断言

- [x] **改 pipeline.rs 测试里所有 `PipelineEvent::Emit { display }`** 为 `Emit { display, insertion: false }`

```bash
grep -n "PipelineEvent::Emit { display" crates/desktop/src/pipeline.rs
# 每处补 insertion: false（默认非插入态）
```

涉及测试：`tick_events_local_changed_produces_persist_emit_polish`、`tick_events_cloud_changed_emits_display_with_partial` 等（断言里 `Emit { display: "你好".to_string() }` → 加 `, insertion: false`）。

### Step 2.4: 测试 + Commit

- [x] 跑测

```bash
cargo test -p octopus-desktop pipeline 2>&1 | tail -15
```
Expected: 绿。

> 注意：Task 2 改了 `PipelineEvent::Emit` 形状，coordinator 的 `apply_pipeline_events`（match Emit 分支）需同步加 `insertion` 绑定。**Task 2 阶段 coordinator 的 match 会编译失败**——本步一并改 coordinator 的 `apply_pipeline_events` Emit 分支（仅解构 `insertion`，暂不传给 result_window，Task 5 才用）：

```rust
PipelineEvent::Emit { display, insertion } => {
    // Task 5 起把 insertion 传给 result_window::update_result；当前暂忽略（_insertion）
    let _ = insertion;
    crate::result_window::update_result(app_handle, &display, false);
}
```

> ⚠️ `update_result` 第三参在 Task 5 才加。**为避免 Task 2 跨任务破坏编译**，调整执行顺序：**Task 5 紧随 Task 2 执行**（见任务依赖：Task 7 依赖 2+5）。或 Task 2 先加 `update_result(app, &display)`（两参，insertion 暂存 coordinator 本地变量不用），Task 5 再改 result_window 签名。**采用后者**：Task 2 的 coordinator Emit 分支：

```rust
PipelineEvent::Emit { display, insertion: _ } => {
    crate::result_window::update_result(app_handle, &display);
}
```

- [x] Commit

```bash
git -C ... add crates/desktop/src/pipeline.rs crates/desktop/src/coordinator.rs
git -C ... commit -m "feat(asr): PipelineEvent::Emit 加 insertion 标志（中间插入态）"
```

---

## Task 5: result_window update_result 加 insertion

> 提前到此（紧跟 Task 2），让 insertion 链路后端贯通。

**Files:** Modify `crates/desktop/src/result_window.rs`、`crates/desktop/src/coordinator.rs`

### Step 5.1: update_result 签名 + payload 改对象

- [x] **改 `update_result`**（result_window.rs L224-240）

```rust
/// 更新结果窗口文本（流式更新时用）。insertion=true 时前端立即渲染（跳过 diverted 300ms 延迟）。
pub fn update_result(app: &tauri::AppHandle, text: &str, insertion: bool) {
    let need_emit = {
        let mut guard = PENDING_TEXT.lock().unwrap();
        if WINDOW_READY.load(Ordering::Relaxed) { true }
        else { *guard = Some(text.to_string()); false }
    };
    if need_emit {
        if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
            let _ = window.emit("update-result", serde_json::json!({ "text": text, "insertion": insertion }));
        }
    }
}
```

- [x] **coordinator Emit 分支接通 insertion**（Task 2 的 `insertion: _` 改为实传）

```rust
PipelineEvent::Emit { display, insertion } => {
    crate::result_window::update_result(app_handle, &display, insertion);
}
```

- [x] **coordinator 其余直调 update_result 处**（commit_edit L1814、PolishDone 后 L1705 等）补第三参 `false`（这些非流式中间插入）：

```bash
grep -n "result_window::update_result" crates/desktop/src/coordinator.rs
# 每处补 , false（除非确属流式插入路径）
```

### Step 5.2: 编译 + Commit

- [x] `cargo build -p octaurus-desktop` 通过；Commit

```bash
git -C ... commit -m "feat(asr): update_result 携带 insertion，payload 改对象"
```

> 前端 handler 读对象在 Task 7 落地（Task 7 之前前端 update-result 会收到对象但按旧 string 解析 → 显示 [object Object]；故 **Task 5 与 Task 7 之间不应跑 e2e**，仅单测）。

---

## Task 3: octopus_llm 多段润色协议

**Files:** Modify `crates/llm/src/prompt.rs`、`crates/llm/src/client.rs`、`crates/llm/src/lib.rs`

### Step 3.1: prompt.rs 加 regions_prompt

- [x] **新增**（prompt.rs，复用 CONFIRMED_MARKER）

```rust
/// 段模型多段润色 user prompt。
/// preserve=true 的段（edited）用【已确认部分】标记原样保留；其余段待润色。
/// LLM 输出整篇（edited 区 verbatim + 润色后的非 edited 区拼接），仅纯文本。
pub fn regions_prompt(regions: &[crate::PolishRegion]) -> String {
    if regions.iter().all(|r| !r.preserve) {
        // 无 edited 段 → 全量润色（与旧 user_prompt(None) 等价语义）
        let full: String = regions.iter().map(|r| r.text.as_str()).collect();
        return format!("请润色以下语音识别文本：\n{}", full);
    }
    let m = CONFIRMED_MARKER;
    let mut body = String::new();
    for r in regions {
        if r.preserve {
            body.push_str(&format!("【{m}（原样保留）】{}\n", r.text));
        } else {
            body.push_str(&format!("【待润色】{}\n", r.text));
        }
    }
    format!(
        "以下文本中，【{m}】已经用户人工校对，必须逐字原样保留、严禁修改；仅对【待润色】区域润色。\n\n\
         {body}\n请输出：所有区域按原顺序拼接为完整文本（{m} 原样），仅输出纯文本。",
    )
}
```

- [x] **加测试**（prompt.rs `#[cfg(test)]`）

```rust
#[test]
fn regions_prompt_no_preserve_is_plain() {
    let rs = vec![crate::PolishRegion { preserve: false, text: "你好".into() }];
    let p = regions_prompt(&rs);
    assert!(p.contains("请润色以下语音识别文本"));
    assert!(!p.contains("原样保留"));
}

#[test]
fn regions_prompt_marks_preserved_regions() {
    let rs = vec![
        crate::PolishRegion { preserve: true, text: "已确认".into() },
        crate::PolishRegion { preserve: false, text: "待润色".into() },
    ];
    let p = regions_prompt(&rs);
    assert!(p.contains("已确认部分"));
    assert!(p.contains("原样保留"));
    assert!(p.contains("待润色"));
}
```

### Step 3.2: 定义 PolishRegion + polish_regions（client.rs）

- [x] **client.rs 顶部加结构 + 函数**（参照既有 `polish` 实现的 LLM 调用方式；先读 client.rs 确认 `polish` 内部如何调 client，复用同套）

```bash
grep -n "pub fn polish\|fn chat\|async fn\|build_client\|invoke_llm" crates/llm/src/client.rs | head
```

读 `polish` 实现（约 30 行）后，新增 `polish_regions` 复用其 LLM 调用：

```rust
/// 一段文档区域。preserve=true → 原样保留（edited）；false → 待润色。
#[derive(Debug, Clone)]
pub struct PolishRegion { pub preserve: bool, pub text: String }

/// 多段润色：按 regions 顺序，edited 区 verbatim 保留、其余润色，返回整篇。
pub fn polish_regions(
    regions: &[PolishRegion],
    llm_config: &octopus_infra::db::CompatibleLlmConfig,
) -> anyhow::Result<String> {
    let prompt = crate::prompt::regions_prompt(regions);
    // 复用 polish() 内部的 client 调用（chat completion，system=prompt::system_prompt()）
    // —— 把既有 polish() 的「构造 messages → 调 client → 取 content」抽成内部 helper 后两处共用。
    crate::client::chat_text(&crate::prompt::system_prompt(), &prompt, llm_config)
}
```

> **实现注意**：先读 `client.rs` 里 `polish` 的完整实现，把「system+user → messages → HTTP → 取 content 文本」抽成 `fn chat_text(system, user, cfg) -> Result<String>`，让 `polish` 与 `polish_regions` 共用（DRY）。若 `polish` 内部已是这样结构，直接复用。

### Step 3.3: lib.rs re-export

- [x] **lib.rs**

```rust
pub use client::{polish, polish_regions, test_connection, PolishRegion};
```

### Step 3.4: 测试 + Commit

- [x] `cargo test -p octopus-llm prompt` 绿（regions_prompt 两测）；`cargo build -p octopus-llm` 通过

```bash
git -C ... add crates/llm/src/prompt.rs crates/llm/src/client.rs crates/llm/src/lib.rs
git -C ... commit -m "feat(llm): 新增 polish_regions 多段润色（edited 标记保留）"
```

---

## Task 4: coordinator 适配（set_caret + 润色新协议 + 落库）

**Files:** Modify `crates/desktop/src/coordinator.rs`、`crates/desktop/src/lib.rs`（命令注册）

### Step 4.1: spawn_polish_thread 改用 polish_regions

- [x] **替换 Task 1 的临时桥接**（coordinator.rs spawn_polish_thread）为真正多段：

```rust
fn spawn_polish_thread(
    input: crate::transcript::PolishInput,
    config: &AppConfig,
    tx: &Sender<Command>,
    ignore_mode: bool,
    session_id: i64,
) {
    let regions: Vec<octopus_llm::PolishRegion> = input.segments.iter().map(|s| {
        octopus_llm::PolishRegion {
            preserve: s.kind == crate::transcript::SegmentKind::Edited,
            text: s.text.clone(),
        }
    }).collect();
    let llm_config = if ignore_mode {
        crate::config::llm_config_ignore_mode(config)
    } else {
        crate::config::llm_config(config)
    };
    let llm_config = match llm_config { Some(c) => c, None => return };
    let tx = tx.clone();
    std::thread::spawn(move || {
        let result = match octopus_llm::polish_regions(&regions, &llm_config) {
            Ok(polished) => Ok(polished),
            Err(e) => { log::warn!("Polish thread error: {}", e); Err(e.to_string()) }
        };
        let _ = tx.send(Command::PolishDone { result, session_id });
    });
}
```

### Step 4.2: 新增 set_caret 命令

- [x] **coordinator 加命令方法 + handler**（参照既有 `commit_edit` 命令 L463/539 的结构）

coordinator impl 块加：

```rust
/// 前端点击光标定位：char offset → transcript.set_caret。
pub fn set_caret(&self, offset: usize) {
    let mut stage = self.stage.lock().unwrap();
    let transcript = stage_transcript(&mut stage);
    if let Some(t) = transcript {
        t.set_caret(offset);
    }
}
```

命令函数（与 `commit_edit` 命令同处）：

```rust
#[tauri::command]
pub fn set_caret(coordinator: tauri::State<'_, Coordinator>, offset: usize) {
    coordinator.set_caret(offset);
}
```

> `stage_transcript` 是 coordinator 既有 helper（返回 `Option<&mut Transcript>`，见 L383 用法）。

- [x] **lib.rs 注册命令**（`invoke_handler!` 里加 `set_caret`）

```bash
grep -n "commit_edit\|enter_edit_mode" crates/desktop/src/lib.rs
# 在同一 invoke_handler 列表加 set_caret
```

### Step 4.3: DB 落库走 segments + text（与 Task 6 协同的过渡）

> Task 6 改 db.rs 的 insert/update/finalize 签名加 segments+text。Task 4 阶段 db.rs 尚未改，coordinator 落库仍写旧 `raw_text`（值=finish_text）。**Task 6 落地后才写 segments**。故 Task 4 不动 DB 调用，仅确保 `db_text()`→`finish_text()` 已在 Task 1 完成。

- [x] 验证 coordinator 无残留旧方法：

```bash
grep -n "has_increase\|\.set_full\|edited_display\|\.polished()\|on_polish_done\b" crates/desktop/src/coordinator.rs
# 期望 0
```

### Step 4.4: 编译 + 测试 + Commit

- [x] `cargo build -p octopus-desktop` + `cargo test -p octopus-desktop` 绿

```bash
git -C ... add crates/desktop/src/coordinator.rs crates/desktop/src/lib.rs
git -C ... commit -m "feat(asr): coordinator set_caret 命令 + spawn_polish_thread 走 polish_regions 多段"
```

---

## Task 6: DB v14 迁移 + CRUD

**Files:** Modify `crates/infra/src/db.sql`、`crates/infra/src/db.rs`、`crates/desktop/src/coordinator.rs`（落库调用）

### Step 6.1: db.sql 加列

- [x] **transcriptions 建表**（db.sql L7-18）加两列：

```sql
CREATE TABLE IF NOT EXISTS transcriptions (
    id            INTEGER PRIMARY KEY,
    created_at    TEXT    NOT NULL,
    engine        TEXT    NOT NULL,
    engine_mode   TEXT,
    raw_text      TEXT    NOT NULL,          -- 兼容旧（= finish_text 扁平，迁移后仍写）
    polished_text TEXT,
    edited_text   TEXT,
    polish_status TEXT    NOT NULL DEFAULT 'off',
    duration_ms   INTEGER,
    char_count    INTEGER,
    segments      TEXT,                       -- 段 JSON [{kind,text}]
    text          TEXT                        -- = finish_text 扁平（search/clipboard 直读）
);
```

> 保留 raw_text/polished_text/edited_text 列（nullable），v14 迁移把数据迁到 segments+text 后**不删列**（SQLite 删列需重建表，开销大；spec §4 允许先保留 nullable 一个版本）。新写入只写 segments+text+raw_text(=finish_text)。
>
> **后续（v15，2026-07-04 已落地）**：rusqlite `bundled` feature 自带 SQLite ≥ 3.45，支持 `ALTER TABLE ... DROP COLUMN` 无需重建表，故 v15 迁移已 DROP 这三列（信息全在 segments/text）。详见 `docs/superpowers/plans/2026-07-04-clean-transcriptions-legacy-columns.md`（若有）与 architecture.md「transcriptions 表 schema v15」。

### Step 6.2: v14 迁移（旧三列→单段）

- [x] **db.rs init_schema 末尾（v==13 分支后）加 v14 分支**

```rust
    } else if v == 13 {
        // v13 → v14：transcriptions 加 segments + text 列；旧记录按 edited≻polished≻raw 映射为单段。
        log::info!("DB migrating v13 → v14: transcriptions 加 segments + text（段模型）...");
        // 幂等加列（旧库无 segments/text）
        let has_segments: bool = conn.prepare("PRAGMA table_info(transcriptions)")?
            .query_map([], |r| r.get::<_, String>(1))?
            .filter_map(|r| r.ok()).any(|c| c == "segments");
        if !has_segments {
            conn.execute("ALTER TABLE transcriptions ADD COLUMN segments TEXT", [])?;
            conn.execute("ALTER TABLE transcriptions ADD COLUMN text TEXT", [])?;
        }
        // 旧记录迁移：edited≻polished≻raw → 单段；text = 该段文本。
        // 用 Rust 读旧三列 + serde_json 构造 segments（纯 SQL 拼 JSON 无法转义换行/控制字符，会破坏 JSON）。
        let rows: Vec<(i64, String, Option<String>, Option<String>)> = {
            let mut stmt = conn.prepare(
                "SELECT id, raw_text, polished_text, edited_text FROM transcriptions",
            )?;
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
                .filter_map(|r| r.ok()).collect()
        };
        for (id, raw, polished, edited) in rows {
            let (kind, text) = if let Some(e) = edited.as_ref().filter(|s| !s.is_empty()) {
                ("edited", e.clone())
            } else if let Some(p) = polished.as_ref().filter(|s| !s.is_empty()) {
                ("polished", p.clone())
            } else {
                ("raw", raw.clone())
            };
            let segs = serde_json::to_string(&serde_json::json!([{ "kind": kind, "text": text }]))?;
            conn.execute(
                "UPDATE transcriptions SET segments=?1, text=?2 WHERE id=?3",
                rusqlite::params![segs, text, id],
            )?;
        }
        conn.execute("PRAGMA user_version = 14", [])?;
        log::info!("DB migrated to v14: transcriptions segments + text");
    }
```

> 新建库（v<2 分支 L156）的 `PRAGMA user_version = 13` 改为 `= 14`，日志相应改 v14。

- [x] **加 v14 迁移单测**（db.rs `#[cfg(test)]`，参照既有迁移测试风格；用内存 `open_init()` 起旧版库 → 跑迁移 → 断言）

```rust
#[test]
fn migrate_v13_to_v14_maps_legacy_to_single_segment() {
    // 1) 建 v13 库 + 插三种旧记录
    // 2) 跑 init_schema → v14
    // 3) 断言：edited 记录 segments 含 "edited"；polished-only 含 "polished"；raw-only 含 "raw"；text = 对应文本
}
```

（具体测试 harness 参照 db.rs 既有 `open_init()` / 内存 Connection 测试模式；先 `grep -n "open_init\|#\[test\]" crates/infra/src/db.rs | head` 找既有迁移测试范例照抄结构。）

### Step 6.3: 改 insert/update/finalize/list/search

- [x] **签名加 segments + text**

`insert_transcription_at_id`（L951）加 `segments: &str, text: &str` 参数，SQL 加两列：

```rust
pub fn insert_transcription_at_id(
    id: i64, text: &str, segments: &str, engine: &str, engine_mode: Option<&str>,
) -> Result<()> {
    with_db(|conn| {
        let created_at = now_string();
        let char_count = text.chars().count() as i64;
        conn.execute(
            "INSERT INTO transcriptions (id, created_at, engine, engine_mode, raw_text, polished_text, polish_status, char_count, segments, text)
             VALUES (?1, ?2, ?3, ?4, ?5, NULL, 'off', ?6, ?7, ?5)",
            params![id, created_at, engine, engine_mode, text, char_count, segments],
        )?;
        Ok(())
    })
}
```

> `raw_text` 与 `text` 同值（= finish_text），向后兼容旧读路径。

`update_raw_text`（L971）→ 改名 `update_text_segments`，写 segments+text+raw_text+char_count：

```rust
pub fn update_text_segments(id: i64, text: &str, segments: &str) -> Result<()> {
    with_db(|conn| {
        let char_count = text.chars().count() as i64;
        conn.execute(
            "UPDATE transcriptions SET raw_text=?1, text=?1, segments=?2, char_count=?3 WHERE id=?4",
            params![text, segments, char_count, id],
        )?;
        Ok(())
    })
}
```

`finalize_transcription`（L1015）加 `segments: &str` 参数，UPDATE 加 `segments=?X, text=?raw`。

`update_edited_text`（L999）→ 改 `update_edited_segments(id, text, segments)`（commit_edit 路径写 `[Edited]` 单段）。

`list_transcriptions` / `list_transcriptions_at`（L1049/1096）：SELECT 加 `segments, text`；`WHERE raw_text LIKE OR polished_text LIKE OR edited_text LIKE` → `WHERE text LIKE ?1`（单列）；`TranscriptionRecord` 加 `segments: Option<String>`、`text: Option<String>` 字段（保留 raw/polished/edited 字段供兼容，值从 text 回填或留旧）。

- [x] **search 改单列**：`list_transcriptions` search 分支 SQL：

```rust
"SELECT id, created_at, engine, raw_text, polished_text, edited_text, polish_status, duration_ms, segments, text
 FROM transcriptions WHERE text LIKE ?1 ORDER BY id DESC LIMIT ?2 OFFSET ?3"
```

### Step 6.4: coordinator 落库调用适配

- [x] **改 coordinator DB 调用点**传 segments+text（grep 定位）：

```bash
grep -n "insert_transcription_at_id\|update_raw_text\|finalize_transcription\|update_edited_text" crates/desktop/src/coordinator.rs
```

每处补 `&transcript.segments_json()` 与 `&transcript.finish_text()`。例如：

```rust
// insert/update 路径（约 L2081/2091）
crate::infra::db::update_text_segments(id, &transcript.finish_text(), &transcript.segments_json())?;
// commit_edit 路径（约 L1805 区）
crate::infra::db::update_edited_segments(id, &text, &transcript.segments_json())?;
```

### Step 6.5: 测试 + Commit

- [x] `cargo test -p octopus-infra` 绿（含新迁移测）；`cargo build -p octopus-desktop` 绿

```bash
git -C ... add crates/infra/src/db.sql crates/infra/src/db.rs crates/desktop/src/coordinator.rs
git -C ... commit -m "feat(db): transcriptions v14 迁移 + segments/text 列 + CRUD 改造"
```

---

## Task 7: 前端光标 + 点击 + update-result handler

**Files:** Modify `crates/desktop/frontend/src/pages/Result/index.tsx`

### Step 7.1: update-result handler 读对象 + insertion 立即渲染

- [x] **改 `update-result` handler**（index.tsx L157-177）

```tsx
["update-result", (p) => {
  if (editingRef.current) return;
  const payload = p as { text: string; insertion: boolean };
  const newText = payload.text;
  const insertion = payload.insertion;
  if (newText === displayedRef.current || newText === pendingDiverted.current) return;
  // 插入态（光标在中间）或纯追加：立即渲染（跳过 diverted 300ms 延迟）
  if (insertion || newText.startsWith(displayedRef.current)) {
    if (divertedTimer.current) { clearTimeout(divertedTimer.current); divertedTimer.current = null; }
    pendingDiverted.current = null;
    renderResultNow(newText);
  } else {
    // diverted（光标在末尾 + 引擎纠正早前文本）：300ms 延迟整体替换
    pendingDiverted.current = newText;
    if (!divertedTimer.current) {
      divertedTimer.current = setTimeout(() => {
        divertedTimer.current = null;
        if (pendingDiverted.current !== null) {
          renderResultNow(pendingDiverted.current);
          pendingDiverted.current = null;
        }
      }, DIVERTED_DELAY_MS);
    }
  }
}],
```

### Step 7.2: 自定义闪烁光标组件

- [x] **加 CSS keyframes + 光标定位**（index.tsx 顶部加常量，渲染区加光标 div）

CSS（加到组件文件内联 style 或现有 CSS；此处用内联 `<style>` 注入或全局 CSS 文件——项目用 Tailwind，`@keyframes` 须在全局 CSS。先查现有全局样式文件位置）：

```bash
grep -rn "voice-line-speaking\|@keyframes" crates/desktop/frontend/src/*.css crates/desktop/frontend/src/**/*.css 2>/dev/null | head
```

在找到的全局 CSS 文件加：

```css
@keyframes asr-caret-blink { 0%, 49% { opacity: 1; } 50%, 100% { opacity: 0; } }
.asr-caret { position: absolute; width: 1.5px; background: var(--foreground, #1a1a1a); animation: asr-caret-blink 1s steps(1) infinite; pointer-events: none; }
```

index.tsx 加光标定位逻辑（光标恒在「活动 Raw 段尾部」= 后端 caret_char_offset，但后端当前不传 offset 给前端。**简化**：前端用「文本末尾」作为光标位置——录音态光标闪在末尾，点击中间后由 set_caret 移动，前端点击处即时显示光标）。

> **光标位置策略（本期）**：非编辑态光标默认闪在**文本末尾**（活动 Raw 段尾）。用户点击中间 → 前端立即在点击处显示光标（本地 state）+ invoke set_caret → 后续流式从该处插入（文本右推，光标跟随末尾）。这是最简可行的「闪烁光标 + 点击定位 + 中插」体感，无需后端回传 caret offset。

index.tsx 改造：

```tsx
// state 加光标位置（char offset，null=末尾）
const [caretPos, setCaretPos] = useState<number | null>(null); // null = 末尾

// 文本区改：非编辑态加 onClick 算 offset + 渲染光标 div
// 文本区 div（L468-481）改为：
<div
  ref={textRef}
  className={cn(/* 原有 */)}
  contentEditable={editing}
  suppressContentEditableWarning
  onInput={onTextInput}
  onClick={!editing ? handleTextClick : undefined}
>
  {text}
  {!editing && <CaretBlink text={text} pos={caretPos} />}
</div>
```

`handleTextClick` + `CaretBlink` 组件：

```tsx
// 算点击处在文本的 code-point offset（非编辑态）
const handleTextClick = (e: React.MouseEvent) => {
  const el = textRef.current;
  if (!el || !text) return;
  // caretRangeFromPoint 算点击处的 DOM Range，量到文本起始的 code-point 数
  const range = document.caretRangeFromPoint?.(e.clientX, e.clientY)
    ?? (document as any).caretPositionFromPoint?.(e.clientX, e.clientY);
  if (!range) return;
  const sel = window.getSelection();
  sel?.removeAllRanges();
  // 用 Range 量 offset：从 el 起点到点击点的字符数（code-point）
  const r = document.createRange();
  r.selectNodeContents(el);
  // 排除光标 div 的影响：临时隐藏 CaretBlink 再量，或用 textRef 文本节点
  const offset = codePointOffsetBefore(el, range);
  setCaretPos(offset);
  invoke("set_caret", { offset });
};

// code-point offset：遍历 el 的文本节点，累计到 target range 的 startContainer/startOffset
function codePointOffsetBefore(container: HTMLElement, range: Range): number {
  const pre = range.cloneRange();
  pre.selectNodeContents(container);
  pre.setEnd(range.startContainer, range.startOffset);
  const str = pre.toString();
  // code-point 计数（与后端 Rust char 对齐：BMP 含中文一致，emoji 代理对需 code-point）
  return Array.from(str).length;
}

// 闪烁光标：绝对定位到 pos 处的像素位置
function CaretBlink({ text, pos }: { text: string; pos: number | null }) {
  const ref = useRef<HTMLSpanElement>(null);
  const [px, setPx] = useState<{ left: number; top: number; height: number } | null>(null);
  useEffect(() => {
    // 用 Range 量 pos 处的像素位置（相对文本容器）
    // pos=null 或 >= 文本长 → 末尾
    // 实现：在隐藏的镜像 div 里定位，或用 document.createRange + getBoundingClientRect
    setPx(measureCaretPx(text, pos));
  }, [text, pos]);
  if (!px) return null;
  return <span ref={ref} className="asr-caret" style={{ left: px.left, top: px.top, height: px.height }} />;
}
```

> `measureCaretPx` 实现：在 textRef 容器内用 Range 定位到第 `pos` 个 code-point 处，`getBoundingClientRect()` 减容器 `getBoundingClientRect()` 得相对像素。完整实现见 Step 7.3。

### Step 7.3: measureCaretPx 完整实现

- [x] **加 helper**（index.tsx 底部工具函数区）

```tsx
// 量 text 中第 pos 个 code-point 处光标的相对像素位置（相对 textRef 容器）。
// pos=null/超出 → 末尾。code-point 计数（Array.from 语义）。
function measureCaretPx(container: HTMLElement, text: string, pos: number | null): { left: number; top: number; height: number } | null {
  const chars = Array.from(text);
  const target = pos == null ? chars.length : Math.min(pos, chars.length);
  // 找 container 内第一个文本节点，用 Range 定位
  const walker = document.createTreeWalker(container, NodeFilter.SHOW_TEXT);
  const firstText = walker.nextNode() as Text | null;
  if (!firstText) {
    // 空文本：光标在容器左上
    return { left: 0, top: 0, height: 18 };
  }
  // firstText.nodeValue 可能含 CaretBlink 之外的纯文本；按 code-point 累进定位
  const cp = Array.from(firstText.nodeValue ?? "");
  const offsetInNode = Math.min(target, cp.length);
  // Range API 的 offset 是 UTF-16 code unit，code-point → code unit 偏移转换
  const utf16Offset = cp.slice(0, offsetInNode).reduce((acc, ch) => acc + ch.length, 0);
  const r = document.createRange();
  r.setStart(firstText, utf16Offset);
  r.collapse(true);
  const rect = r.getBoundingClientRect();
  const cRect = container.getBoundingClientRect();
  return { left: rect.left - cRect.left, top: rect.top - cRect.top, height: rect.height || 18 };
}
```

> `CaretBlink` 的 useEffect 改调 `measureCaretPx(textRef.current?.parentElement!, text, pos)`，textRef 是文本 div。注意 CaretBlink 渲染在 textRef 内部，量像素时 CaretBlink 自身不影响文本节点（它是 span，量第一个文本节点即可）。

### Step 7.4: build + 手动验证

- [x] **前端构建**

```bash
cd /Users/wudarui/workspace/agent/octopus/.claude/worktrees/clean-used-feature/crates/desktop/frontend
npm run build 2>&1 | tail -15
```
Expected: 无 TS 错误。

- [x] **Commit**

```bash
git -C ... add crates/desktop/frontend/src/pages/Result/index.tsx crates/desktop/frontend/src/**/*.css
git -C ... commit -m "feat(asr): 前端非编辑态闪烁光标 + 点击定位 + update-result 读 insertion"
```

---

## Task 8: 集成 e2e

**Files:** 无代码改动，纯验证。

### Step 8.1: 全量编译 + 单测

- [x] **全 crate 编译 + 测试**

```bash
cd /Users/wudarui/workspace/agent/octopus/.claude/worktrees/clean-used-feature
cargo test --workspace 2>&1 | tail -30
```
Expected: 全绿。

### Step 8.2: 桌面启动 + e2e 手动验证清单

- [x] **启动 desktop**

```bash
cargo run -p octopus-desktop 2>&1 | tail -20 &
```

- [x] **e2e 验证项**（逐条勾）：

- [x] 默认录音：不点光标，文本逐字末尾追加（零回归，与改造前逐字一致）。
- [x] 录音中，非编辑态可见闪烁光标（文本末尾）。
- [x] 录音中点击文本中间 → 光标移到点击处；继续说话 → 新词从光标处冒出、原后续文本右推。
- [x] 中插态下显示文本 = 落库 segments 扁平 = 复制内容（一致，真中插）。
- [x] mode=2 中插态自动停顿润色：停顿后活动段变 Polished，光标保持同字符位，后续新语音新建 Raw。
- [x] 编辑态：textarea 显示整篇；保存 → 单 Edited 段；取消 → 恢复。
- [x] 编辑后再点中间 → Edited 段被劈开（DB segments 多条 edited）。
- [x] DB 迁移：旧库（有历史记录）升级后 v14，历史记录 segments 单段（edited≻polished≻raw），text 列正确。
- [x] 搜索历史：按 text 列命中。
- [x] emoji/代理对：点击 emoji 之间，offset 不错位（code-point 对齐）。
- [x] 精简态（小条）+ 长篇态（放大）点击均生效。

- [x] **e2e 通过后最终 Commit（如有文档同步）**

```bash
git -C ... status
# 若有 architecture.md / specs 同步改动，一并 commit
git -C ... commit -m "docs(asr): 同步光标中插改造到文档" --allow-empty
```

---

## Self-Review

**1. Spec 覆盖：**
- §1 数据结构（Segment/Transcript/方法）→ Task 1 ✓
- §2.A 流式插入（apply_engine_full/push_delta/delta 追踪）→ Task 1 + 2 ✓
- §2.B 光标定位（set_caret 劈段/段界）→ Task 1 + Task 7 点击 ✓
- §2.C 润色（全篇一次，edited 冻、raw/polished 重润，连续非 edited 合并）→ Task 3 + Task 4 ✓
- §2.C mode=2 自动润色（has_raw 触发、pending_delta 缓存、光标恢复）→ Task 1（has_raw/pending）+ Task 4 ✓
- §2.D 编辑态（commit_edit 单 Edited、取消快照）→ Task 1 ✓（取消快照前端已有 editSnapshotRef）
- §2.E 停止/落库（segments JSON + text）→ Task 6 ✓
- §3 前端光标（自定义闪烁、点击 offset、code-point 对齐、update-result insertion）→ Task 7 ✓
- §4 DB v14 迁移（旧三列→单段、search 改 text 列）→ Task 6 ✓
- §8 边界用例（diverted/劈半词/连续非 edited 合并/LLM 擅改/空文档点光标/编辑后多次定位/pending flush/跨光标 delta 连续/空 Raw 不产生）→ Task 1 单测覆盖 ✓
- §9 测试要点（transcript/pipeline/db/前端）→ 各 Task 内测 ✓

**2. 占位符扫描：** 无 TBD/TODO；Task 6.2 迁移测试与 Task 3.2 chat_text 抽取标注了「先读既有实现照抄结构」，给了明确 grep 命令定位范例——非占位符，是「按既有模式适配」的合理指引（避免在 plan 里盲抄可能过时的代码）。

**3. 类型一致性：** 跨 Task 方法名统一核查：
- `finish_text` / `display_text`(alias) / `full`(alias) / `db_text`(alias) — Task 1 定义，Task 1/4/6 使用一致 ✓
- `apply_engine_full(&str) -> bool` — Task 1 定义，Task 2 pipeline 调用 ✓
- `is_inserting() -> bool` — Task 1 定义，Task 2 Emit 构造 + Task 5 透传 ✓
- `has_raw() -> bool` — Task 1 定义，Task 4 check_and_trigger_polish 调用 ✓
- `take_polish_input() -> PolishInput` — Task 1 定义，Task 1 临时桥接 + Task 4 真正使用 ✓
- `polish_apply(&str)` / `on_polish_failed()` — Task 1 定义，Task 1/4 调用 ✓
- `set_caret(usize)` / `caret_char_offset()` — Task 1 定义，Task 4 命令 + Task 7 前端 invoke ✓
- `segments_json()` — Task 1 定义，Task 6 落库调用 ✓
- `PolishInput { segments }` / `SegmentKind::Edited` — Task 1 定义，Task 4 转 PolishRegion ✓
- `PolishRegion { preserve, text }` — Task 3 定义，Task 4 构造 ✓
- `PipelineEvent::Emit { display, insertion }` — Task 2 定义，Task 2/5 调用 ✓
- `update_result(app, text, insertion)` — Task 5 定义，Task 5/2 调用 ✓

一致性通过。✓

---

## 追加特性：选中替换（2026-07-04）

中插特性 8 任务全完成并 e2e 通过后追加。详见 spec §11。本节记录实际实施（已编码 + 单测绿，e2e 待用户）。

**实施任务（6 步，全部完成）**：

1. **transcript.rs**：加 `pending_delete: Option<(usize,usize)>` 字段（new 初始化 None）；从 `set_caret` 抽 `fn split_at(char_off)->usize`（DRY）；新 `fn delete_range(start,end)` + `pub fn set_selection(start,end)`；`apply_engine_full` 首个非空 delta 前消费 pending_delete；`take_polish_input` 开头消费；`set_caret`/`on_polish_failed`/`commit_edit` 清除。6 个单测全绿。⚠️ 初版**仅 `apply_engine_full` 消费** `pending_delete`，`append_segment`（VadSegmented/cloud 路径）漏，离线/cloud 引擎选中替换失效——e2e 后修（见本节末「后续修复 9d4a654」）。
2. **coordinator.rs**：`Command::SetSelection{start,end}` 变体 + 命令循环 match 臂（!editing 门控 + stage_transcript）+ `Coordinator::set_selection` + `#[tauri::command] set_selection`（镜像 set_caret 六处）。
3. **main.rs**：invoke_handler 注册 `coordinator::set_selection`（紧邻 set_caret）。
4. **Result/index.tsx**：`onClick`→`onMouseUp`，`handleTextClick`→`handleTextMouseUp`（按 `isCollapsed` 分流：折叠→set_caret，非折叠→setCaretPos(null)+invoke set_selection）；抽 `codePointOffsetTo`（end 端点），`codePointOffsetBefore` 退化为 wrapper。
5. **文档**：spec §11 增补（本节同步）。
6. **验证**：`cargo test -p octopus-desktop` 64 passed（含新 6）、`npm run build` tsc+vite 通过。e2e 清单（用户执行）：拖选中间字（高亮出现、闪烁光标消失）→ 开口 → 高亮消失、选中字删、识别字从该处插、闪烁光标复现跟随；选中后点别处取消（文字保留）；选中替换后继续说话接中插（不再删）；光标中插原行为不回归。

---

**后续修复（2026-07-04，9d4a654）**：选中替换 e2e（用户用 VadSegmented 离线引擎）发现选中后说话、选中文本未删、识别字插在前面——根因 `append_segment` 漏消费 `pending_delete`（初版仅 `apply_engine_full` 消费，见步骤 1 注）。`append_segment` 开头（delta 非空检查后）补 `if let Some((s,e)) = self.pending_delete.take() { self.delete_range(s,e); }`，与 `apply_engine_full` 完全对称。新增 `set_selection_then_first_append_segment_replaces` 回归测试（拖选 → append_segment 首词删旧插新），`cargo test -p octopus-desktop` **65 passed**（原 64 + 1）。e2e 用户验证通过。详见 spec §11.7。

---

## 追加：跨会话选中替换（方案 C）+ Bug C cloud 对称（2026-07-05）

活跃态选中替换（§11）上线后，跨会话维度（Idle 选中 → Toggle 开新会话替换）暴露 bug。设计详见 spec §11.8。

### 方案 C：移除 idle_selection 长期缓存，改前端推选区两阶段 Toggle（a79ab97，已合 main）

- [x] **根因**：后端 `idle_selection: Option<(text,start,end)>` 长期缓存 → 失焦残留 / 编辑后 stale text 指错位 / 拖选后编辑残留三类 bug
- [x] `coordinator.rs`：移除 `idle_selection`；Toggle 在 Idle 走两阶段（`emit("prepare-record", prepare_id)` + spawn 200ms 看门狗 + `pending_prepare` 等待态）；抽 `begin_recording(selection)`（cloud/streaming/vad 三分支对称：`Some`→`commit_edit`+`set_selection` 种子 / `None`→普通开）；`StartRecording`/`FallbackStart` 校验 prepare_id 后调 begin_recording；Cancel/Discard/再 Toggle 中断等待；SetCaret/SetSelection 等待态 no-op
- [x] `main.rs`：invoke_handler 注册 `coordinator::start_recording`
- [x] `Result/index.tsx`：`currentSelectionRef`（拖选缓存 {start,end,text}）；listen `prepare-record` → `invoke("start_recording", {prepare_id, selection: [text,start,end]|null})`（**数组非对象**，tuple serde）；blur/selectionchange(折叠)/enterEdit/commitEdit/cancelEdit/show-result/clear-result/hide-result 清 ref
- [x] 验证：check/clippy cloud+default、tsc、vitest 14 全绿；e2e 用户验证通过

### Bug C：cloud 分支对称植入 selection 种子（1f3e162，已合 main）

- [x] **根因**：`begin_recording` cloud 分支（`use_cloud_streaming`）直接 `Transcript::new` 空实例、漏消费 `selection` → 云端跨会话选中替换退化为末尾追加（与 §11.7 `append_segment` 漏消费 `pending_delete` **同构**——状态植入/消费须全路径对称）；local streaming/vad 早已正确植入
- [x] **修复**：cloud 分支按 streaming/vad 对称 `commit_edit(text)`+`set_selection(s,e)` 种子 transcript + `is_continuation` 延续态展示旧文本。详见 spec §11.8
- [x] **同批清理**：`handle_toggle` 删 3 死参（engine/use_streaming/use_cloud_streaming，方案 C 后仅停录音不再用）/ `SetSelection` 删 `text` 死字段（跨会话选区改走 `start_recording.selection`，活跃态只需 start/end，回归 spec §11 描述）/ 2 处 `collapsible_if` 合并（cloud tick 线程 + aliyun 句子提交）。零行为变化，clippy cloud/default 全绿
- [x] 验证：check/clippy cloud+default **0 warning/0 error**、tsc exit 0、vitest 14/14；cloud 录音基本路径回归 OK；cloud 选中替换场景靠代码对称保证（与 streaming/vad 字节级同构 + 共用 Transcript/paste 路径）

---

## 追加修复：前端渲染 4 bug + 性能优化（2026-07-04）

选中替换 e2e 后深度使用暴露 Result 窗前端渲染问题，全在 `crates/desktop/frontend/src/pages/Result/index.tsx`。详见 spec §12。**前端无单测框架**（无 vitest/jest），靠 `npm run build`（tsc+vite）+ 用户 e2e 验证。

### 修复 1：文字不渲染（最关键，contentEditable 子项不 reconcile）

- **现象**：识别中文本上滚后再下滚，新识别文字**空白**、闪烁光标落在旧末尾；继续说话积压文字一次性出现。
- **根因**：`textRef` 是 `contentEditable={editing}` div，React 19（`createRoot` concurrent）对其 children 的 commit **不写 DOM**——设计上保护用户在 contentEditable 里的编辑不被覆盖，`flushSync` 强制 commit 也无效（commit 本身就跳过 children 写入）。故流式 `setText(newText)` 改了 state，但 DOM textNode 始终旧的 → 文字不渲染。
- **修法**（`renderResultNow` 内，非编辑态）：
  1. imperative `textRef.current.textContent = newText` 绕过 React 强制 DOM = state（核心）。
  2. `measureCaretPx` 长度改读 DOM `firstText.nodeValue`（移除 `text` 参数）——否则按 state 新 text 算 target、DOM `firstText` 旧文本 clamp 到旧末尾 → 光标错位。
  3. 保留 `flushSync(() => setText(newText))` 驱动 state 让子组件 `CaretBlink` 的 `useEffect[text]` 触发重测。

### 修复 2：光标滚动错位 + 视口外隐藏

- **现象**：文字多时上滚，末尾滚出视口，光标仍停容器底旧位闪烁（视觉错位）。
- **根因**：`CaretBlink` 的 `px.top` 是视口相对值（随 `scrollTop` 变），原只在 `text`/`pos` 变时重算，滚动后不更新。
- **修法**：`CaretBlink` 加 scroll 监听（`{ passive: true }` + rAF 节流）重测 px；渲染时 `px.top < -2 || px.top > clientHeight + 2` 则 return null 隐藏。

### 修复 3：滚动跟随间隙（onScroll 立即滚底）

- **现象**：滚回底部区域后，最新识别文字滞留视口下方一段空白，下个 tick 才归位。
- **根因**：onScroll 设了 `stickToBottomRef = true`，但实际滚到底部等下个渲染 tick。
- **修法**：onScroll 检测 stick 恢复 true 时立即 `el.scrollTop = el.scrollHeight`（不等 tick）。

### 修复 4：换行符显示（whitespace-pre-wrap）

- **现象**：编辑态的 `\n` 编辑时起作用、退出编辑后失效（行变一坨）。
- **根因**：textRef div 默认 `white-space:normal`，`innerText` 的 `\n` 折叠成空格。后端 `commit_edit`/`finish_text`/DB `text` 列全保留 `\n`，纯前端 CSS 问题。
- **修法**：textRef div 加 `whitespace-pre-wrap` 类。

### 性能优化（同期，#34-38）

- 滚底跟随 `stickToBottomRef`（修识别中无法上滚 + 减 reflow）、rAF 合并渲染（消 layout thrashing）、CaretBlink 光标路径统一、DB 落库节流（≥500ms UPDATE，Finalize 兜底）、`diverted_pending` 上限 + 删 dead_code（`mark_polish_pending`/`clear_polish_pending`）。

### 光标首位（修复 1 派生，O(1) 优化回归）

`measureCaretPx` 末尾态曾改 `selectNodeContents + collapse(false)`（锚容器边界）Chrome 返 zero rect 触发兜底 `(0,0)` → 光标首位。回退为文本节点内 `setStart(firstText, offset) + collapse(true)`。教训：collapsed range `getBoundingClientRect` 对锚点敏感。

### 验证

- `npm run build` tsc+vite 通过。
- e2e（用户）：4 bug 全部修复后逐条复现验证通过（流式文字滚动后正常渲染、上滚光标隐藏、滚回底部立即跟随、编辑换行保留）。

---

## 追加：前端最小测试基建（2026-07-04）

4-bug 排查耗时（flushSync 失败 2 次才定位 contentEditable 不 reconcile）暴露前端无单测框架的代价。引入 vitest 4 + jsdom 29（commit e797e0f）：

- `measureCaretPx` / `codePointOffsetTo` / `codePointOffsetBefore` 从 `Result/index.tsx` 抽到 `./caret.ts`（纯函数，隔离可测，不拉整个组件模块）。
- `caret.test.ts`（9 测全绿）锁住 **code-point → UTF-16 offset 对齐**（光标错位/首位 bug 的核心）与 null/空容器分支。
- jsdom 未实现 `Range.prototype.getBoundingClientRect`（Element 有），`defineProperty` 补零矩形；像素级光标位仍留给 e2e（jsdom 量不了）。
- `renderResultNow` 耦合组件/Tauri，组件级测试留后续。

---

## 追加修复：代码审查 2 bug（2026-07-04）

第三轮代码审查（测试基建落地后）发现 2 个 Result 窗前端渲染 bug，详见 spec §13。前端仍无组件级单测（renderResultNow 耦合 Tauri），靠 `npm run build` + `npm run test`（caret 纯函数）+ 用户 e2e 验证。

### Bug 1.1：最终文本被 pending diverted 延迟覆盖

- **文件**：`Result/index.tsx` show-result handler 的 else 分支（最终/插入态立即渲染）。
- **症状**：中途误判 diverted 启动 300ms 计时器后，最终文本立即渲染，但 300ms 后旧回调仍触发、用旧基准整体替换覆盖最终文本（文字闪回旧值）。
- **修复**：else 分支显式 `clearTimeout(divertedTimer.current)` + `pendingDiverted.current = null`，确保最终文本落地后 diverted 路径不再触发。
- **验证**：`npm run build` 绿。

### Bug 2.1：CaretBlink 初始 measure 同步 layout thrashing

- **文件**：`Result/index.tsx` `CaretBlink` 的 `useEffect[containerRef,text,pos]`。
- **症状**：初始 `measure()` 同步 `getBoundingClientRect`，与同帧 `flushSync(setText)`+`textContent` DOM 写叠加 → 强制回流（layout thrashing）；高频 ASR（10-20Hz）每帧叠加。
- **修复**：初始 `measure()` 改 `requestAnimationFrame`（DOM 写先落地、布局稳定后再读，代价 1 帧 ~16ms 光标滞后）；初始 raf 与 scroll raf 分变量（`raf`/`scrollRaf`）独立 cancel，`!el` 提前返回也 cancel 初始 raf 防泄漏。`flushSync` 保留驱动 state 让 effect 同步 schedule rAF。
- **验证**：`npm run build` + `npm run test`（caret.test.ts 9 测不受影响，measureCaretPx 签名未变）绿。

---

## 追加修复：代码审查 3.1 + 3.4（caret 多节点 + enterEdit 光标恢复，2026-07-04）

§13（审查 2 bug）之后第四/五轮审查又发现 2 个 `Result/` 前端 bug。详见 spec §14。前端靠 `npm run build` + `npm run test`（caret 纯函数）+ 用户 e2e。

### Bug 3.1：measureCaretPx 仅定位首文本节点

- **文件**：`Result/caret.ts` `measureCaretPx`。
- **症状**：`whitespace-pre-wrap`（§12.4）多行 / 编辑残留 `<br>` 使容器含多 text node；`pos` 超首节点长度时旧实现 clamp 首节点末尾 → 光标测量错位。当前结果窗 `textContent` 单行通常单节点，e2e 未触发，属防御性正确化。
- **修复**：抽共享 helper `locateCpOffset(container, pos)`（TreeWalker 收集全部 text node，按各节点 code-point 长度累加定位落点 + UTF-16 offset；`pos=null`/越界→末节点末尾）；`measureCaretPx` 改用它。单节点主路径行为不变。长度仍读 DOM `nodeValue`（§12.1）。
- **验证**：caret.test.ts 加 2 测（多节点 pos 越首节点→setStart 到后续节点；pos=null→末节点末尾），**14 测全绿**。

### Bug 3.4：进编辑态光标无条件落末尾

- **文件**：`Result/index.tsx` `enterEdit`（~L258）。
- **症状**：`setCaretPos(null)` 后 `range.collapse(false)` 无条件置末尾；长文本点过中间再进编辑，光标跑末尾。
- **修复**：`setCaretPos(null)` 前 `caretPosRef` 捕获 `restorePos`（caretPos 的 ref 镜像，useRef + 同步 effect，避免闭包 stale）；setTimeout 内 `restorePos != null && placeCaretAtCodePoint(el, restorePos)` 精准恢复，否则末尾兜底。新增 `placeCaretAtCodePoint`（复用 `locateCpOffset`）。
- **边界**：caretPos 仅纯点击设值（handleTextMouseUp 折叠分支）；拖选置 null。故拖选后进编辑仍落末尾（设计如此）。
- **验证**：caret.test.ts 加 3 测（placeCaretAtCodePoint 定位 / 空容器 false / 多节点越界→末节点末尾），14 测全绿 + `npm run build` 绿。e2e（用户）：点文本中间 → 编辑快捷键 → 光标在点击位。

### Bug（第四轮 1.2）：editingRef 异步更新致 update-result 覆盖编辑态

- **文件**：`Result/index.tsx` `enterEdit` / `commitEdit` / `cancelEdit`。
- **症状**：`editingRef` 原仅由 `useEffect([editing])` 在 commit 后同步（L89）。`setEditing(true)` 到 commit 间存在窗口；此间若 `update-result` 到达、`editingRef` 仍 false → 守护（L1235 `if (editingRef.current) return`）放行 → `renderResultNow` 写 `textContent` 覆盖刚进入的 contentEditable、打断光标。`invoke("enter_edit_mode")` 往返期间后端仍可能推 update-result，放大窗口。
- **修复**（`797e7f3`）：三个回调内 `setEditing(...)` 后**同步**置 `editingRef.current`（enterEdit=true / commitEdit+cancelEdit=false），零延迟拦截，不依赖 commit 后的 effect。
- **验证**：`tsc` 绿；e2e（用户待测）：ASR 录音中点编辑，文本不被覆盖。
- **注**：同 commit `797e7f3` 另含 polishNow 改 `polishLoadingRef` 门控（润色 listen 不随 polishLoading 重建，第四轮 2.2），与光标无关，此 plan 不展开。
