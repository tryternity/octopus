//! 粘贴队列——内存 VecDeque，FIFO 出栈，不持久化。
//!
//! 设计见 docs/superpowers/specs/2026-08-05-paste-stack-design.md。
//! 不进 DB——重启清空是合理的（粘贴队列是临时操作流）。

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

static PASTE_STACK: OnceLock<Mutex<VecDeque<String>>> = OnceLock::new();

fn stack() -> &'static Mutex<VecDeque<String>> {
    PASTE_STACK.get_or_init(|| Mutex::new(VecDeque::new()))
}

/// 入栈（按传入顺序）。返回栈大小。
pub fn push(ids: Vec<String>) -> usize {
    let mut s = stack().lock().unwrap();
    for id in ids {
        s.push_back(id);
    }
    s.len()
}

/// 弹出栈底 history_id（FIFO）。栈空返 None。
pub fn pop() -> Option<String> {
    stack().lock().unwrap().pop_front()
}

/// 清空栈。
pub fn clear() {
    stack().lock().unwrap().clear();
}

/// 栈状态（剩余数量 + 下一条内容预览）。
///
/// `next_preview` 取栈底（下一个要粘贴的）条目 content 前 30 字符（按字节安全截断：
/// 只在边界为 ASCII 时切片，避免切断多字节 UTF-8）。
pub fn status() -> (usize, Option<String>) {
    let s = stack().lock().unwrap();
    let remaining = s.len();
    let next_preview = s.front().and_then(|id| {
        octopus_infra::db::with_db(|conn| {
            octopus_clipboard::store::get_item_by_id(conn, id)
        })
        .ok()
        .flatten()
        .map(|item| truncate_preview(&item.content, 30))
    });
    (remaining, next_preview)
}

/// 把字符串截断到 ~`max` 字符并加省略号。按 char_indices 安全切片，不切断多字节。
fn truncate_preview(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    // 取前 max 个字符的末尾字节位置作为安全切点。
    let end = s
        .char_indices()
        .nth(max.saturating_sub(1))
        .map(|(byte, ch)| byte + ch.len_utf8())
        .unwrap_or(0);
    format!("{}…", &s[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// tests 共享全局 PASTE_STACK 静态，需串行执行，否则 push_pop_fifo_order 与
    /// clear_empties_stack 并行会互相清掉对方入栈的数据。此 Mutex 仅用于测试序列化。
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    /// 由于 status() 会触发 with_db（生产 DB），单测只覆盖 push/pop/clear 顺序语义，
    /// 不调 status（status 的 DB 部分依赖运行时 DB 初始化）。
    #[test]
    fn push_pop_fifo_order() {
        let _g = TEST_LOCK.lock().unwrap();
        // 直接操作内部静态，保证测试顺序（与 push/pop API 行为一致）
        clear();
        assert_eq!(push(vec!["a".into(), "b".into(), "c".into()]), 3);
        assert_eq!(pop(), Some("a".into())); // FIFO：先进先出
        assert_eq!(pop(), Some("b".into()));
        assert_eq!(pop(), Some("c".into()));
        assert_eq!(pop(), None); // 栈空
        clear();
    }

    #[test]
    fn clear_empties_stack() {
        let _g = TEST_LOCK.lock().unwrap();
        push(vec!["x".into(), "y".into()]);
        clear();
        assert_eq!(pop(), None);
    }

    #[test]
    fn truncate_preview_keeps_short() {
        // 纯函数，不碰全局状态，无需加锁。
        assert_eq!(truncate_preview("hi", 30), "hi");
        assert_eq!(truncate_preview("一二三", 2), "一二…");
        // 多字节边界不切断
        assert_eq!(truncate_preview("abc", 2), "ab…");
    }
}
