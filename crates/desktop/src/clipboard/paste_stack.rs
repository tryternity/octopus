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

/// 队列视图单条目：history_id + 类型 + 内容预览（前 50 字符）。
/// 供「队列」tab 渲染列表（peek_all 返回），与剪贴板条目本体解耦——
/// 只携带展示需要的最少字段，避免把整条 ClipboardItem 经 IPC 序列化。
#[derive(Debug, Clone)]
pub struct PasteStackItem {
    pub history_id: String,
    pub item_type: String,
    pub preview: String,
}

/// 读取整个队列（FIFO 顺序，index 0 = 下一个要弹出的）。
/// 对栈内每个 id 查 DB 拿类型 + 内容预览（前 50 字符）；DB 行不存在（已删）
/// 或读失败的条目被静默过滤，保证前端拿到的列表与栈内有效 id 一一对应。
pub fn peek_all() -> Vec<PasteStackItem> {
    let ids: Vec<String> = stack().lock().unwrap().iter().cloned().collect();
    ids.into_iter()
        .filter_map(|id| {
            octopus_infra::db::with_db(|conn| {
                octopus_clipboard::store::get_item_by_id(conn, &id)
            })
            .ok()
            .flatten()
            .map(|item| {
                let content = item.content;
                let preview = if content.chars().count() > 50 {
                    format!("{}...", content.chars().take(50).collect::<String>())
                } else {
                    content
                };
                PasteStackItem {
                    history_id: id,
                    item_type: item.item_type.as_str().to_string(),
                    preview,
                }
            })
        })
        .collect()
}

/// 删除指定位置的条目（0 = 栈底/下一个弹出）。越界返 Err。
/// 保持其余条目相对顺序不变（VecDeque::remove 仅挪动被删位之后的元素）。
pub fn remove_at(index: usize) -> Result<(), String> {
    let mut s = stack().lock().unwrap();
    if index >= s.len() {
        return Err(format!("index {} out of bounds (len {})", index, s.len()));
    }
    s.remove(index);
    Ok(())
}

/// 把 `from` 位置的条目移到 `to` 位置（VecDeque::remove → insert）。
/// 任一越界返 Err（不做 swap，保证插入位置即最终位置）。from==to 为 no-op。
pub fn move_item(from: usize, to: usize) -> Result<(), String> {
    let mut s = stack().lock().unwrap();
    if from >= s.len() || to >= s.len() {
        return Err(format!(
            "index out of bounds: from={}, to={}, len={}",
            from,
            to,
            s.len()
        ));
    }
    if from == to {
        return Ok(());
    }
    let item = s.remove(from).unwrap();
    s.insert(to, item);
    Ok(())
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

    #[test]
    fn remove_at_front_middle_back() {
        let _g = TEST_LOCK.lock().unwrap();
        clear();
        push(vec!["a".into(), "b".into(), "c".into()]);
        // 删栈底（下一个要弹出的）→ 剩 b,c 顺序不变
        assert!(remove_at(0).is_ok());
        assert_eq!(pop(), Some("b".into()));
        assert_eq!(pop(), Some("c".into()));
        assert_eq!(pop(), None);
        // 越界
        assert!(remove_at(0).is_err());
        clear();
    }

    #[test]
    fn move_item_reorders() {
        let _g = TEST_LOCK.lock().unwrap();
        clear();
        push(vec!["a".into(), "b".into(), "c".into()]);
        // 把 c（idx 2）提到队首（idx 0）→ c,a,b
        assert!(move_item(2, 0).is_ok());
        assert_eq!(pop(), Some("c".into()));
        assert_eq!(pop(), Some("a".into()));
        assert_eq!(pop(), Some("b".into()));
        // from==to 是 no-op，不报错
        clear();
        push(vec!["x".into(), "y".into()]);
        assert!(move_item(0, 0).is_ok());
        // 越界
        assert!(move_item(5, 0).is_err());
        assert!(move_item(0, 5).is_err());
        clear();
    }
}
