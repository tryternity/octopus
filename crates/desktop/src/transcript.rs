// crates/desktop/src/transcript.rs
//! 识别过程文本状态机：统一管理原生(raw)/润色(polished)/增量(increase)三文本。
//!
//! 内部用 `full`（当前完整 ASR）+ `raw_len`（上次停顿快照的 char 长度）派生 raw/increase：
//! - raw      = full[..raw_len]   （停顿快照，润色基准）
//! - increase = full[raw_len..]   （停顿后新增）
//! 停顿触发润色时 raw_len 推进到 full 长度，increase 自动清空。
//! mode=0/1 不做中间润色，display/db 直接用 full。

use crate::config::PolishMode;
use std::time::Instant;

pub struct Transcript {
    /// 识别开始时刻毫秒时间戳（DB 主键 + 时长计算基准）
    pub id: i64,
    mode: PolishMode,
    /// 当前完整 ASR（流式 set_full / 伪流式 append_segment）
    full: String,
    /// 上次停顿快照的 char 长度（raw 的边界）
    raw_len: usize,
    /// 对 raw 的润色结果（仅 mode=2 中间润色 / 各 mode 最终润色后填值）
    polished: String,
    /// 用户编辑后的 committed 文本（空 = 未编辑；非空时覆盖 polished/raw，优先级最高）。
    edited: String,
    last_polish_time: Instant,
    polish_pending: bool,
    /// 是否已 INSERT 过 DB（首次有文本时 INSERT 后置 true，之后走 UPDATE）
    db_inserted: bool,
}

impl Transcript {
    pub fn new(id: i64, mode: PolishMode) -> Self {
        Self {
            id,
            mode,
            full: String::new(),
            raw_len: 0,
            polished: String::new(),
            edited: String::new(),
            last_polish_time: Instant::now(),
            polish_pending: false,
            db_inserted: false,
        }
    }

    pub fn db_inserted(&self) -> bool {
        self.db_inserted
    }

    pub fn mark_db_inserted(&mut self) {
        self.db_inserted = true;
    }

    /// 流式：设置当前完整 ASR（引擎 accept_samples/flush 返回全量）。
    pub fn set_full(&mut self, text: &str) {
        self.full = text.to_string();
    }

    /// 伪流式：追加一段识别文本（delta）。
    pub fn append_segment(&mut self, delta: &str) {
        self.full.push_str(delta);
    }

    /// 当前完整 ASR（= raw + increase）。
    pub fn full(&self) -> &str {
        &self.full
    }

    /// 停顿后增量（仅 mode=2 有意义；mode=0/1 恒空，符合 spec §2.2 不变量）。
    #[cfg(test)]
    pub fn increase(&self) -> String {
        if self.mode == PolishMode::Intermediate {
            self.full.chars().skip(self.raw_len).collect()
        } else {
            String::new()
        }
    }

    /// 检查是否有新增内容（避免分配 String 的开销）
    pub fn has_increase(&self) -> bool {
        self.mode == PolishMode::Intermediate && self.full.chars().count() > self.raw_len
    }

    /// 停顿触发：返回完整 ASR 作为润色输入，并推进 raw_len（increase 清空）。
    pub fn snapshot_for_polish(&mut self) -> String {
        self.raw_len = self.full.chars().count();
        self.full.clone()
    }

    /// 润色完成：更新 polished（raw_len 已在 snapshot_for_polish 推进）。
    pub fn on_polish_done(&mut self, polished: String) {
        self.polished = polished;
        self.polish_pending = false;
        self.last_polish_time = Instant::now();
    }

    /// 润色失败：保持 polished 不变，清 pending。
    pub fn on_polish_failed(&mut self) {
        self.polish_pending = false;
    }

    /// 用户提交编辑：edited = 文本，raw_len 推进到 full 末尾（increase 清空），full（raw）不变。
    /// 空串 → 清空 edited（回退到 polished/raw）。
    pub fn commit_edit(&mut self, text: &str) {
        if text.is_empty() {
            self.edited.clear();
        } else {
            self.edited = text.to_string();
            self.raw_len = self.full.chars().count();
        }
    }

    /// 是否已编辑（edited 非空）。
    pub fn has_edit(&self) -> bool {
        !self.edited.is_empty()
    }

    /// edited 文本（未编辑返回 None）。
    pub fn edited_text(&self) -> Option<&str> {
        if self.edited.is_empty() {
            None
        } else {
            Some(&self.edited)
        }
    }

    /// 停止时喂给「无润色粘贴/兜底」的文本。
    /// edited 非空 → display（用户编辑结果 + 新增，不补标点）。
    /// 否则 None → 调用方走原 raw 逻辑（db_text + 按需补「。」）。
    pub fn edited_display(&self) -> Option<String> {
        if self.edited.is_empty() {
            None
        } else {
            Some(self.display_text())
        }
    }

    pub fn polish_pending(&self) -> bool {
        self.polish_pending
    }

    pub fn mark_polish_pending(&mut self) {
        self.polish_pending = true;
    }

    pub fn clear_polish_pending(&mut self) {
        self.polish_pending = false;
    }

    pub fn last_polish_time(&self) -> Instant {
        self.last_polish_time
    }

    /// 运行时更新润色模式（工具栏 live 切换用）。Coordinator 单线程访问，无需同步。
    pub fn set_mode(&mut self, mode: PolishMode) {
        self.mode = mode;
    }

    /// 展示文本：committed 前缀 + increase。
    /// committed 优先级：edited ≻ polished ≻ full[..raw_len]。
    /// edited 为空时与旧行为等价（full[..raw_len] + full[raw_len..] = full）。
    pub fn display_text(&self) -> String {
        let committed = if !self.edited.is_empty() {
            self.edited.clone()
        } else if !self.polished.is_empty() {
            self.polished.clone()
        } else {
            self.full.chars().take(self.raw_len).collect()
        };
        let inc: String = self.full.chars().skip(self.raw_len).collect();
        let mut s = committed;
        s.push_str(&inc);
        s
    }

    /// 落库文本：完整 ASR（raw + increase）。
    pub fn db_text(&self) -> String {
        self.full.clone()
    }

    /// polished（最终润色后有值；否则空）。
    pub fn polished(&self) -> &str {
        &self.polished
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_disabled_display_is_full() {
        let mut t = Transcript::new(1, PolishMode::Disabled);
        t.set_full("你好世界");
        assert_eq!(t.display_text(), "你好世界");
        assert_eq!(t.db_text(), "你好世界");
        assert_eq!(t.increase(), ""); // mode=0 恒空（spec §2.2）
        assert_eq!(t.db_inserted(), false);
    }

    #[test]
    fn mode_finalonly_display_is_full() {
        let mut t = Transcript::new(2, PolishMode::FinalOnly);
        t.append_segment("第一段");
        t.append_segment("第二段");
        assert_eq!(t.display_text(), "第一段第二段");
        assert_eq!(t.db_text(), "第一段第二段");
    }

    #[test]
    fn mode_intermediate_snapshot_and_merge() {
        let mut t = Transcript::new(3, PolishMode::Intermediate);
        // 说了一段
        t.set_full("你好世界");
        assert_eq!(t.display_text(), "你好世界"); // polished 空，increase=full

        // 停顿快照 → 送润色
        let snap = t.snapshot_for_polish();
        assert_eq!(snap, "你好世界");
        assert_eq!(t.increase(), ""); // 快照后 increase 空（raw_len 推进到 full）

        // 润色完成
        t.on_polish_done("你好，世界。".into());
        assert_eq!(t.display_text(), "你好，世界。"); // polished + 空 increase
    }

    #[test]
    fn mode_intermediate_increase_after_snapshot() {
        // 验证：快照后新内容进 increase，display = polished + increase
        let mut t = Transcript::new(4, PolishMode::Intermediate);
        t.set_full("原始文本");
        t.snapshot_for_polish();
        t.on_polish_done("润色文本".into());

        // 流式：raw 前缀稳定，full 追加新内容
        t.set_full("原始文本新增部分");
        assert_eq!(t.increase(), "新增部分"); // raw 前缀稳定，新增进 increase
        assert_eq!(t.display_text(), "润色文本新增部分");
    }

    #[test]
    fn append_segment_accumulates() {
        let mut t = Transcript::new(5, PolishMode::Intermediate);
        t.append_segment("A");
        t.append_segment("B");
        assert_eq!(t.full(), "AB");
    }

    #[test]
    fn polish_failed_keeps_polished() {
        let mut t = Transcript::new(6, PolishMode::Intermediate);
        t.set_full("原文");
        t.snapshot_for_polish();
        t.on_polish_done("润色".into());
        t.mark_polish_pending();
        t.on_polish_failed(); // 失败
        assert_eq!(t.polished(), "润色"); // 保持上次值
        assert!(!t.polish_pending());
    }

    #[test]
    fn empty_full_initial_state() {
        let t = Transcript::new(10, PolishMode::Intermediate);
        assert_eq!(t.display_text(), ""); // 空 full → display 空
        assert_eq!(t.full(), "");
        assert_eq!(t.increase(), ""); // increase 空
        assert!(!t.polish_pending());
        assert!(!t.db_inserted());

        // 空快照：返回空串，raw_len 保持 0
        let mut t2 = Transcript::new(11, PolishMode::Intermediate);
        let snap = t2.snapshot_for_polish();
        assert_eq!(snap, "");
        assert_eq!(t2.increase(), "");
    }

    #[test]
    fn consecutive_snapshots_overwrite_polished() {
        let mut t = Transcript::new(12, PolishMode::Intermediate);

        // 第一次停顿快照 + 润色
        t.set_full("第一段");
        let s1 = t.snapshot_for_polish();
        assert_eq!(s1, "第一段");
        t.on_polish_done("润色一".into());
        assert_eq!(t.display_text(), "润色一"); // polished + 空 increase

        // 继续说 → increase 出现
        t.set_full("第一段第二段");
        assert_eq!(t.increase(), "第二段");
        assert_eq!(t.display_text(), "润色一第二段");

        // 第二次停顿快照 + 润色（覆盖第一次 polished）
        let s2 = t.snapshot_for_polish();
        assert_eq!(s2, "第一段第二段");
        assert_eq!(t.increase(), ""); // raw_len 推进到 full → increase 清空
        t.on_polish_done("润色一二".into());
        assert_eq!(t.display_text(), "润色一二"); // 第二次润色覆盖第一次
    }

    #[test]
    fn set_mode_changes_intermediate_behavior_live() {
        // 起始 mode=2（中间润色）：说一段 + 快照 + 润色
        let mut t = Transcript::new(20, PolishMode::Intermediate);
        t.set_full("原文");
        t.snapshot_for_polish();
        t.on_polish_done("润色".into());
        assert_eq!(t.display_text(), "润色");

        // 继续说 → increase 出现（mode=2 行为）
        t.set_full("原文新增");
        assert_eq!(t.increase(), "新增");
        assert_eq!(t.display_text(), "润色新增");

        // live 切到 mode=0（关闭）：increase（公开 API）立即恒空；
        // display 仍展示 polished + 新增（polished 非空时 display 不看 mode）
        t.set_mode(PolishMode::Disabled);
        assert_eq!(t.increase(), "");
        assert_eq!(t.display_text(), "润色新增"); // polished + display_increase
    }

    #[test]
    fn commit_edit_sets_edited_and_advances_boundary() {
        let mut t = Transcript::new(30, PolishMode::Intermediate);
        t.set_full("你好世界");
        t.snapshot_for_polish(); // T1 阶段仍用旧 snapshot；T5 替换
        t.on_polish_done("你好，世界。".into());
        assert_eq!(t.display_text(), "你好，世界。");

        t.commit_edit("你好世界（手改）");
        assert_eq!(t.edited_text(), Some("你好世界（手改）"));
        assert!(t.has_edit());
        // raw_len 推进到 full 末尾 → increase 清空
        assert_eq!(t.display_text(), "你好世界（手改）");
    }

    #[test]
    fn commit_edit_preserves_raw_and_appends_new() {
        let mut t = Transcript::new(31, PolishMode::Intermediate);
        t.set_full("原文");
        t.commit_edit("原文（手改）");
        assert_eq!(t.full(), "原文"); // raw（full）原样保留
        t.set_full("原文新增");
        assert_eq!(t.display_text(), "原文（手改）新增"); // edited + 新增
    }

    #[test]
    fn edited_takes_priority_over_polished_and_raw() {
        let mut t = Transcript::new(32, PolishMode::Intermediate);
        t.set_full("raw文本");
        t.snapshot_for_polish();
        t.on_polish_done("polished文本".into());
        t.commit_edit("edited文本".into());
        assert_eq!(t.display_text(), "edited文本"); // edited ≻ polished ≻ raw
    }

    #[test]
    fn empty_commit_clears_edit_falls_back() {
        let mut t = Transcript::new(33, PolishMode::Intermediate);
        t.set_full("原文");
        t.commit_edit("手改".into());
        assert!(t.has_edit());
        t.commit_edit("");
        assert!(!t.has_edit());
        assert_eq!(t.edited_text(), None);
        assert_eq!(t.display_text(), "原文"); // 回退 raw
    }

    #[test]
    fn edited_display_returns_display_when_edited_else_none() {
        let mut t = Transcript::new(34, PolishMode::Intermediate);
        t.set_full("原文");
        assert_eq!(t.edited_display(), None); // 未编辑
        t.commit_edit("手改".into());
        assert_eq!(t.edited_display().as_deref(), Some("手改"));
        t.set_full("原文新增");
        assert_eq!(t.edited_display().as_deref(), Some("手改新增")); // = display
    }
}
