//! include/exclude 文件过滤，对齐 huggingface-cli（Python fnmatch）。
//! 语义：多 include = 任一匹配则含（OR）；多 exclude = 任一匹配则排（OR）；exclude 优先于 include。

/// 单个 path 是否应被下载。
/// - include 为空 → 视为匹配所有（全含）
/// - 否则 path 须匹配至少一个 include 模式
/// - 再排除匹配任一 exclude 模式的
pub(crate) fn should_download(path: &str, include: &[String], exclude: &[String]) -> bool {
    let included = include.is_empty() || include.iter().any(|pat| fnmatch(pat, path));
    if !included { return false; }
    !exclude.iter().any(|pat| fnmatch(pat, path))
}

/// fnmatch 兼容匹配：`*` 跨任意字符（含 `/`）、`?` 单字符、`[...]` 字符类。
/// 手写实现以保证与 Python fnmatch 一致（glob crate 的 * 不跨 /）。
pub(crate) fn fnmatch(pattern: &str, name: &str) -> bool {
    fn rec(p: &[u8], n: &[u8]) -> bool {
        let (mut pi, mut ni) = (0, 0);
        let (mut star_p, mut star_n): (Option<usize>, usize) = (None, 0);
        while ni < n.len() {
            if pi < p.len() {
                match p[pi] {
                    b'?' => { pi += 1; ni += 1; continue; }
                    b'*' => { star_p = Some(pi); star_n = ni; pi += 1; continue; }
                    b'[' => {
                        // 字符类 [abc] 或 [a-z]，支持末尾 ]
                        if let Some(close) = p[pi..].iter().position(|&c| c == b']') {
                            let class = &p[pi + 1..pi + close];
                            if class_match(class, n[ni]) { pi += close + 1; ni += 1; continue; }
                        }
                    }
                    c if c == n[ni] => { pi += 1; ni += 1; continue; }
                    _ => {}
                }
            }
            // 回溯到上一个 *
            if let Some(sp) = star_p {
                pi = sp + 1;
                star_n += 1;
                ni = star_n;
            } else {
                return false;
            }
        }
        // 跳过末尾 *
        while pi < p.len() && p[pi] == b'*' { pi += 1; }
        pi == p.len()
    }
    fn class_match(class: &[u8], c: u8) -> bool {
        let (negate, body) = if !class.is_empty() && (class[0] == b'!' || class[0] == b'^') {
            (true, &class[1..])
        } else { (false, class) };
        let mut hit = false;
        let mut i = 0;
        while i < body.len() {
            if i + 2 < body.len() && body[i + 1] == b'-' {
                if body[i] <= c && c <= body[i + 2] { hit = true; }
                i += 3;
            } else {
                if body[i] == c { hit = true; }
                i += 1;
            }
        }
        hit ^ negate
    }
    rec(pattern.as_bytes(), name.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(ss: &[&str]) -> Vec<String> { ss.iter().map(|x| x.to_string()).collect() }

    #[test]
    fn star_matches_across_slash() {
        // fnmatch 的 * 跨 / —— 关键差异点
        assert!(fnmatch("*", "onnx/model_int8.onnx"));
        assert!(fnmatch("onnx/*_int8.onnx", "onnx/model_int8.onnx"));
    }

    #[test]
    fn include_or_exclude_priority() {
        // 用户例子：include=['*','onnx/*_int8.onnx'], exclude=['*/*','onnx/*_merged_int8.onnx']
        let inc = s(&["*", "onnx/*_int8.onnx"]);
        let exc = s(&["*/*", "onnx/*_merged_int8.onnx"]);
        // 根目录文件：被 * 含，不被 */* 排 → 下
        assert!(should_download("config.json", &inc, &exc));
        // onnx/model_int8.onnx：被 * 含，但被 */* 排（含 /）→ 不下
        assert!(!should_download("onnx/model_int8.onnx", &inc, &exc));
        // merged 被显式排
        assert!(!should_download("onnx/model_merged_int8.onnx", &inc, &exc));
    }

    #[test]
    fn empty_include_matches_all() {
        assert!(should_download("any/file", &[], &[]));
        assert!(!should_download("any/file", &[], &s(&["any/*"])));
    }

    #[test]
    fn question_mark_single_char() {
        assert!(fnmatch("?.txt", "a.txt"));
        assert!(!fnmatch("?.txt", "ab.txt"));
    }

    #[test]
    fn char_class() {
        assert!(fnmatch("[abc].txt", "a.txt"));
        assert!(fnmatch("[a-c].txt", "b.txt"));
        assert!(!fnmatch("[!abc].txt", "a.txt"));
    }
}
