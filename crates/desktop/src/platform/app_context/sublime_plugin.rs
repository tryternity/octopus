//! Sublime Text 自绘编辑器取数器。
//!
//! Sublime Text 的 AX 树不含真实编辑器内容（只有 "UNREGISTERED" 静态文本），
//! 磁盘 fallback 仅对已保存文件有效。
//!
//! 本方案：自动安装一个 Sublime 插件，通过 `subl --command octopus_export_context`
//! 触发插件导出当前 view 的完整内容 + 选区位置到 /tmp JSON 文件，
//! 然后读取并切前后文。对未保存的 untitled 文件同样有效。

#![cfg(target_os = "macos")]

use super::*;

const SUBLIME_PLUGIN_NAME: &str = "octopus_context.py";
const SUBLIME_OUTPUT_PATH: &str = "/tmp/octopus_sublime_ctx.json";

/// Sublime 插件源码。
const SUBLIME_PLUGIN_SOURCE: &str = r#"import sublime
import sublime_plugin
import json

class OctopusExportContextCommand(sublime_plugin.TextCommand):
    def run(self, edit):
        view = self.view
        content = view.substr(sublime.Region(0, view.size()))
        sel = view.sel()
        if sel:
            r = sel[0]
            data = {
                "content": content,
                "sel_start": r.begin(),
                "sel_end": r.end(),
                "file_path": view.file_name() or "",
            }
        else:
            data = {"content": content, "sel_start": 0, "sel_end": 0}
        with open("/tmp/octopus_sublime_ctx.json", "w") as f:
            json.dump(data, f)
"#;

/// 尝试通过 Sublime 插件获取上下文。
///
/// 流程：
/// 1. 确保 Sublime 插件已安装（Packages/Octopus/octopus_context.py）
/// 2. 调用 `subl --command octopus_export_context`
/// 3. 读取 /tmp/octopus_sublime_ctx.json
/// 4. 用选区位置切前后文
///
/// 对未保存文件（untitled）同样有效——插件直接读 view 内容，不依赖磁盘文件。
pub fn try_sublime_plugin_context(
    bundle_id: &str,
    selected_text: &str,
    deadline: std::time::Instant,
) -> Option<SurroundingText> {
    let packages_dir = find_sublime_packages_dir(bundle_id)?;

    // 1. 确保插件已安装
    let plugin_path = packages_dir.join("Octopus").join(SUBLIME_PLUGIN_NAME);
    ensure_plugin_installed(&plugin_path);

    // 2. 删除旧的输出文件
    let _ = std::fs::remove_file(SUBLIME_OUTPUT_PATH);

    // 3. 触发插件命令（受 deadline 约束，防 subl 挂起卡死 trigger worker）
    let subl_path = find_subl_binary(deadline)?;
    let mut cmd = std::process::Command::new(&subl_path);
    cmd.arg("--command").arg("octopus_export_context");
    let result = super::run_command_with_deadline(cmd, deadline);

    if result.is_none() {
        log::warn!("[app-context] subl --command 执行失败或超时");
        return None;
    }

    // 4. 等待输出文件（最多 300ms）
    let mut waited = 0;
    while waited < 300 {
        if std::path::Path::new(SUBLIME_OUTPUT_PATH).exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
        waited += 50;
    }

    // 5. 读取 JSON
    let content = std::fs::read_to_string(SUBLIME_OUTPUT_PATH).ok()?;
    let json: serde_json::Value = serde_json::from_str(&content).ok()?;

    let full_text = json["content"].as_str().unwrap_or("");
    let sel_start = json["sel_start"].as_u64().unwrap_or(0) as usize;
    let sel_end = json["sel_end"].as_u64().unwrap_or(0) as usize;

    if full_text.is_empty() {
        return None;
    }

    log::info!(
        "[app-context] Sublime 插件: content_len={} sel=[{},{}]",
        full_text.chars().count(),
        sel_start,
        sel_end
    );

    // 6. 切前后文（用插件返回的选区位置，精确）
    let limit = 1000;
    let chars: Vec<char> = full_text.chars().collect();
    let total = chars.len();
    let start = sel_start.min(total);
    let end = sel_end.max(start).min(total);

    let before_start = start.saturating_sub(limit);
    let before: String = chars[before_start..start].iter().collect();
    let after_end = (end + limit).min(total);
    let after: String = chars[end..after_end].iter().collect();

    let window_title = json["file_path"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(|s| {
            std::path::Path::new(s)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| s.to_string())
        });

    // 7. 内容校验——确保选中文本在全文中
    let sel_trimmed = selected_text.trim();
    if !sel_trimmed.is_empty() && !full_text.contains(sel_trimmed) {
        log::info!("[app-context] Sublime 插件：选中文本不在 view 内容中");
        return None;
    }

    Some(SurroundingText {
        before: if before.is_empty() { None } else { Some(before) },
        after: if after.is_empty() { None } else { Some(after) },
        window_title,
    })
}

/// 前台是否为 Sublime Text。
///
/// 2026-07-20 perf：从 osascript 改 NSWorkspace 直调（< 1ms vs osascript 启动 ~200ms）。
/// Sublime 的 bundle id 是 `com.sublimetext.4` / `com.sublimetext.3`。
pub fn is_sublime_frontmost() -> bool {
    #[cfg(target_os = "macos")]
    {
        super::macos_ax::frontmost_bundle_id()
            .map(|bid| bid.contains("sublimetext"))
            .unwrap_or(false)
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// 通过插件读取 Sublime 当前选区文本。
///
/// detect_selection 专用——Sublime 4 默认 `copy_with_empty_selection: true`
/// （无选中时 Cmd+C 复制当前行），导致 changeCount 方案失效（changeCount +1 且
/// 剪贴板有当前行内容，误判有选中）。本函数用插件的 `sel_start/sel_end` 精确判定：
/// - 返回 `Some(text)`：有选中文本（sel_start != sel_end）
/// - 返回 `None`：无选中（sel_start == sel_end）或插件不可用
///
/// 复用 try_sublime_plugin_context 的插件机制（subl --command + JSON 输出），
/// 但只取选区文本，不切前后文（前后文留给后续 gather_context）。
pub fn get_sublime_selection(deadline: std::time::Instant) -> Option<String> {
    // bundle_id 用 Sublime 4 兜底（detect 阶段不精确读 bundle_id，find_sublime_packages_dir 会自动找）
    let packages_dir = find_sublime_packages_dir("com.sublimetext.4")?;
    let plugin_path = packages_dir.join("Octopus").join(SUBLIME_PLUGIN_NAME);
    ensure_plugin_installed(&plugin_path);

    let _ = std::fs::remove_file(SUBLIME_OUTPUT_PATH);

    let subl_path = find_subl_binary(deadline)?;
    let mut cmd = std::process::Command::new(&subl_path);
    cmd.arg("--command").arg("octopus_export_context");
    let result = super::run_command_with_deadline(cmd, deadline);
    if result.is_none() {
        log::info!("[app-context][detect] subl --command 失败，Sublime 选区检测降级");
        return None;
    }

    let mut waited = 0;
    while waited < 300 {
        if std::path::Path::new(SUBLIME_OUTPUT_PATH).exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
        waited += 50;
    }

    let content = std::fs::read_to_string(SUBLIME_OUTPUT_PATH).ok()?;
    let json: serde_json::Value = serde_json::from_str(&content).ok()?;
    let full_text = json["content"].as_str().unwrap_or("");
    let sel_start = json["sel_start"].as_u64().unwrap_or(0) as usize;
    let sel_end = json["sel_end"].as_u64().unwrap_or(0) as usize;

    let selected = extract_sublime_selection(full_text, sel_start, sel_end);
    match &selected {
        Some(s) => log::info!(
            "[app-context][detect] Sublime sel=[{},{}] = 选中 {} 字符",
            sel_start, sel_end, s.chars().count()
        ),
        None => log::info!("[app-context][detect] Sublime sel=[{},{}] = 无选中", sel_start, sel_end),
    }
    selected
}

/// 从 Sublime 插件的 full_text + sel_start + sel_end 提取选中文本（纯函数，可单测）。
///
/// 判定规则（绕过 Sublime 4 `copy_with_empty_selection` 的 Cmd+C 陷阱）：
/// - `sel_start >= sel_end` 或 full_text 空 → None（无选中）
/// - 选区文本 trim 后空 → None（纯空白选中不算）
/// - 否则 → Some(选区文本)
///
/// sel_start/sel_end 是 Sublime 的**字符偏移**（非字节），用 chars() 切片保证 CJK 正确。
/// 偏移越界时 clamp 到 full_text 长度（Sublime 插件理论上不越界，但防御性 clamp）。
pub fn extract_sublime_selection(full_text: &str, sel_start: usize, sel_end: usize) -> Option<String> {
    if sel_start >= sel_end || full_text.is_empty() {
        return None;
    }
    let chars: Vec<char> = full_text.chars().collect();
    let total = chars.len();
    let start = sel_start.min(total);
    let end = sel_end.min(total);
    if start >= end {
        return None;
    }
    let selected: String = chars[start..end].iter().collect();
    if selected.trim().is_empty() {
        return None;
    }
    Some(selected)
}

/// 查找 Sublime Text 的 Packages 目录。
fn find_sublime_packages_dir(bundle_id: &str) -> Option<std::path::PathBuf> {
    let home = dirs::home_dir()?;
    let candidates = if bundle_id.contains("sublimetext.4") {
        vec![
            home.join("Library/Application Support/Sublime Text/Packages"),
            home.join("Library/Application Support/Sublime Text 3/Packages"),
        ]
    } else {
        vec![
            home.join("Library/Application Support/Sublime Text 3/Packages"),
            home.join("Library/Application Support/Sublime Text/Packages"),
        ]
    };

    for dir in &candidates {
        if dir.exists() {
            return Some(dir.clone());
        }
    }

    // 都不存在——创建 ST4 路径（首次安装）
    let target = candidates[0].clone();
    let _ = std::fs::create_dir_all(&target);
    Some(target)
}

/// 确保插件已安装。如果插件文件不存在或内容不一致，写入最新版本。
fn ensure_plugin_installed(plugin_path: &std::path::Path) {
    let need_write = match std::fs::read_to_string(plugin_path) {
        Ok(existing) => existing != SUBLIME_PLUGIN_SOURCE,
        Err(_) => true,
    };

    if need_write {
        if let Some(parent) = plugin_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(plugin_path, SUBLIME_PLUGIN_SOURCE);
        log::info!("[app-context] Sublime 插件已安装到 {}", plugin_path.display());
        // 插件刚安装——Sublime 需要重新加载才能识别
        // Sublime 会在下次 focus 时自动扫描 Packages，但首次可能需要等一下
    }
}

/// 查找 subl 命令行工具。
fn find_subl_binary(deadline: std::time::Instant) -> Option<std::path::PathBuf> {
    // 常见安装路径
    let candidates = [
        std::path::PathBuf::from("/Applications/Sublime Text.app/Contents/SharedSupport/bin/subl"),
        std::path::PathBuf::from("/usr/local/bin/subl"),
        std::path::PathBuf::from("/opt/homebrew/bin/subl"),
    ];

    for path in &candidates {
        if path.exists() {
            return Some(path.clone());
        }
    }

    // 尝试 PATH 中的 subl
    let mut cmd = std::process::Command::new("which");
    cmd.arg("subl");
    super::run_command_with_deadline(cmd, deadline)
    .filter(|o| o.status.success())
    .and_then(|o| {
        let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
        if s.is_empty() {
            None
        } else {
            Some(std::path::PathBuf::from(s))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 回归：无选中（sel_start == sel_end）→ None。
    /// 这是 Sublime `copy_with_empty_selection` 陷阱的核心防护——
    /// sel_start == sel_end 必须判定为无选中，否则 detect 误判 Text。
    #[test]
    fn extract_sublime_selection_no_selection() {
        assert_eq!(extract_sublime_selection("hello world", 5, 5), None);
        assert_eq!(extract_sublime_selection("hello", 0, 0), None);
        assert_eq!(extract_sublime_selection("任意文本", 2, 2), None);
    }

    /// 回归：sel_start > sel_end（反向选区或异常）→ None
    #[test]
    fn extract_sublime_selection_reversed_range() {
        assert_eq!(extract_sublime_selection("hello", 4, 2), None);
        assert_eq!(extract_sublime_selection("hello", 3, 3), None);
    }

    /// 正常有选中 → 返回选区文本
    #[test]
    fn extract_sublime_selection_normal() {
        assert_eq!(extract_sublime_selection("hello world", 0, 5), Some("hello".into()));
        assert_eq!(extract_sublime_selection("hello world", 6, 11), Some("world".into()));
        assert_eq!(extract_sublime_selection("选中Text", 0, 8), Some("选中Text".into()));
    }

    /// CJK 文本按字符偏移切（非字节）—— sel_start/sel_end 是字符偏移
    #[test]
    fn extract_sublime_selection_cjk_char_offset() {
        // "你好世界" 4 个字符，选 [1,3] = "好世"
        assert_eq!(extract_sublime_selection("你好世界", 1, 3), Some("好世".into()));
    }

    /// 选区纯空白 → None（trim 后空不算选中）
    #[test]
    fn extract_sublime_selection_whitespace_only() {
        assert_eq!(extract_sublime_selection("a   b", 1, 4), None); // 选 "   " 纯空格
        assert_eq!(extract_sublime_selection("a\n\nb", 1, 3), None); // 选 "\n\n"
    }

    /// 偏移越界 clamp 到 full_text 长度（防御性）
    #[test]
    fn extract_sublime_selection_clamps_overflow() {
        // full_text "abc" 只有 3 字符，sel [1, 100] → clamp 到 [1,3] = "bc"
        assert_eq!(extract_sublime_selection("abc", 1, 100), Some("bc".into()));
        // sel_start 也越界
        assert_eq!(extract_sublime_selection("abc", 50, 100), None); // clamp 后 start(3) >= end(3)
    }

    /// 空 full_text → None
    #[test]
    fn extract_sublime_selection_empty_text() {
        assert_eq!(extract_sublime_selection("", 0, 5), None);
    }

    /// 回归核心不变量：sel_start < sel_end 且非空白 → 必须返回 Some
    /// （detect_selection 的 Sublime 分支依赖此判定区分有无选中）
    #[test]
    fn extract_sublime_selection_valid_range_returns_some() {
        let cases = [
            ("hello", 0, 1),
            ("选中Text", 0, 2),
            ("a\nb", 0, 3),
            ("你好", 0, 2),
        ];
        for (text, s, e) in cases {
            assert!(
                extract_sublime_selection(text, s, e).is_some(),
                "sel=[{},{}] of {:?} 应返回 Some", s, e, text
            );
        }
    }
}
