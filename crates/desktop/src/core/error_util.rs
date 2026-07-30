//! 错误转 String 的统一 helper（2026-07-29 DRY 重构）。
//!
//! Tauri 命令返回 `Result<T, String>`，全 crate 有 ~178 处 `.map_err(|e| e.to_string())?`。
//! 抽 `e2s`（纯转换 + log 留痕）+ `e2s_ctx`（带上下文案）统一，避免每处手写闭包。

/// 把错误转 String 的统一出口；顺手 `log::error!` 记录便于排查（多数 to_string 调用丢弃了错误，
/// log 是唯一留痕）。泛型约束 `Display + Debug`——`{e:?}` 打完整 Debug 形态，`to_string` 走 Display。
pub(crate) fn e2s<E: std::fmt::Display + std::fmt::Debug>(e: E) -> String {
    log::error!("{e:?}");
    e.to_string()
}

/// 带上下文案的错误转 String——给 `format!("xxx 失败: {e}")` 这类带前缀的场景用。
/// log 打 `{ctx}: {e}`，返回 `"{ctx}: {e}"`。
pub(crate) fn e2s_ctx<E: std::fmt::Display>(ctx: &str, e: E) -> String {
    let msg = format!("{ctx}: {e}");
    log::error!("{msg}");
    msg
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn e2s_converts_display_to_string() {
        let s = e2s(std::io::Error::new(std::io::ErrorKind::NotFound, "missing"));
        assert!(s.contains("missing"));
    }

    #[test]
    fn e2s_ctx_prefixes_context() {
        let s = e2s_ctx("解析失败", std::io::Error::new(std::io::ErrorKind::InvalidData, "bad"));
        assert!(s.starts_with("解析失败: "));
        assert!(s.contains("bad"));
    }

    #[test]
    fn e2s_accepts_string() {
        let s = e2s("plain error".to_string());
        assert_eq!(s, "plain error");
    }
}
