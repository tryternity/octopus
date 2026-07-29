//! Prompt 文件管理 + 路径辅助（从 action_bar_commands/mod.rs 提取，Task 1.2）。
//!
//! 命令面板的 `@文件名` 引用展开、`~/.octopus/.sync/prompts/` 文件 CRUD，
//! 以及 `format_paths` / `derive_cwd` 等纯路径辅助函数。

use tauri::{AppHandle, Emitter, Manager};
use crate::core::error_util::e2s_ctx;

/// file:// URL 路径编码：仅编码空格（macOS file:// URL 的最小需求）
fn url_encode_path(path: &str) -> String {
    path.replace(' ', "%20")
}

/// 渲染 agent prompt 模板：替换 {{voice}} / {{text}} / {{files}} 占位符。
/// - voice: 语音识别结果（用户口述的指令；注入 {{voice}}）
/// - text: 选中的文本（注入 {{text}}）
/// - files: 选中的文件/文件夹路径列表（换行分隔注入 {{files}}）
pub fn render_agent_prompt(template: &str, voice: &str, text: &str, files: &[String]) -> String {
    template
        .replace("{{voice}}", voice)
        .replace("{{text}}", text)
        .replace("{{files}}", &files.join("\n"))
}

/// 解析 action_data 的 `@文件名` 引用——读 `~/.octopus/prompts/<name>.md` 作为完整 prompt。
///
/// 规则：action_data trim 后以 `@` 开头 → 尝试读 `~/.octopus/prompts/<剩余部分>.md`；
/// 成功返回文件内容，失败（文件不存在/无权限）返回原文（降级为普通 prompt）。
/// 不以 `@` 开头 → 原文返回（普通 prompt）。
///
/// 仅 agent / ai 类型在 execute_action_bar_inner 中调用。DB 存原始 `@文件名`，执行时展开。
pub fn resolve_prompt_reference(action_data: &str) -> String {
    let trimmed = action_data.trim();
    let Some(name) = trimmed.strip_prefix('@') else {
        return action_data.to_string(); // 不是引用
    };
    let name = name.trim();
    if name.is_empty() {
        return action_data.to_string(); // `@` 单独，不当引用
    }
    let path = octopus_infra::paths::octopus_config_home().join(".sync").join("prompts").join("command").join(format!("{}.md", name));
    match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(e) => {
            log::warn!("[action-bar] prompt 引用 @{} 读取失败（{}），降级为普通文本", name, e);
            action_data.to_string()
        }
    }
}

/// `~/.octopus/prompts/` 下的 prompt 文件信息（供设置页引用模式下拉选择）。
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptFileInfo {
    /// 文件名（不含扩展名），如 "tolaria"——对应 `@tolaria` 引用
    pub name: String,
    /// 完整文件名，如 "tolaria.md"
    pub file_name: String,
    /// 内容预览（前 500 字符），供 UI 预览展示
    pub preview: String,
}

/// 列出 `~/.octopus/.sync/prompts/<category>/*.md` 文件，供设置页的「引用文件」下拉选择。
/// category: "command"（命令菜单 prompt）/ "polish"（润色提示词）
/// 目录不存在时返回空 Vec（不报错——首次使用时目录还没建）。
#[tauri::command]
pub fn list_prompt_files(category: String) -> Result<Vec<PromptFileInfo>, String> {
    let dir = octopus_infra::paths::octopus_config_home().join(".sync").join("prompts").join(&category);
    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("md") {
                let file_name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
                if file_name.is_empty() {
                    continue;
                }
                let content = std::fs::read_to_string(&path).unwrap_or_default();
                let preview = content.chars().take(500).collect::<String>();
                files.push(PromptFileInfo {
                    name: file_name.clone(),
                    file_name: format!("{}.md", file_name),
                    preview,
                });
            }
        }
    }
    files.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(files)
}

/// 用 CompactEditor 打开文件全文查看/编辑（source="file"，按文件路径 md5 去重）。
/// 读 `~/.octopus/.sync/prompts/<category>/<name>.md` 全文 → emit open-tab 事件。
/// category: "command" / "polish"。同一文件只开一个 tab——已存在则激活不覆盖。
#[tauri::command]
pub fn open_file_in_editor(name: String, category: String, app: AppHandle) -> Result<(), String> {
    let path = octopus_infra::paths::octopus_config_home()
        .join(".sync")
        .join("prompts")
        .join(&category)
        .join(format!("{}.md", name));
    let text = std::fs::read_to_string(&path)
        .map_err(|e| e2s_ctx("读取文件失败: {}", e))?;
    // 文件完整路径 md5 → 取前 8 字节作 i64（固定 id，前端 file:<id> 去重）
    let path_str = path.to_string_lossy();
    let hash = octopus_sync::store::md5_hex(path_str.as_bytes());
    let item_id = i64::from_str_radix(&hash[..16], 16).unwrap_or(0);

    let window_label = crate::commands::compact_editor_window::WINDOW_LABEL;
    let payload = serde_json::json!({
        "itemId": item_id,
        "source": "file",
        "text": text,
        "filePath": path_str,
    });
    if let Some(window) = app.get_webview_window(window_label) {
        let _ = window.emit("compact-editor://open-tab", payload);
        let _ = window.show();
        let _ = window.set_focus();
    } else {
        // 窗口不存在 → 建 pending + 开窗（file source 不走 store_pending_temp，走通用 pending）
        crate::commands::compact_editor_commands::store_pending_file(item_id, text, path_str.to_string());
        crate::commands::compact_editor_window::create_compact_editor_window(&app, None);
    }
    Ok(())
}

/// 新建空白 prompt 文件。在 `~/.octopus/.sync/prompts/<category>/` 下创建空 `<name>.md`。
/// 已存在则报错（防覆盖）。创建后前端可调 open_file_in_editor 打开编辑。
#[tauri::command]
pub fn create_prompt_file(category: String, name: String) -> Result<(), String> {
    let dir = octopus_infra::paths::octopus_config_home()
        .join(".sync")
        .join("prompts")
        .join(&category);
    // 确保目录存在
    std::fs::create_dir_all(&dir).map_err(|e| e2s_ctx("创建目录失败: {}", e))?;
    let path = dir.join(format!("{}.md", name));
    if path.exists() {
        return Err(format!("文件已存在: {}.md", name));
    }
    std::fs::write(&path, "").map_err(|e| e2s_ctx("创建文件失败: {}", e))
}

/// 保存文件内容到磁盘（CompactEditor file tab 保存按钮用）。
#[tauri::command]
pub fn save_file(path: String, content: String) -> Result<(), String> {
    std::fs::write(&path, &content).map_err(|e| e2s_ctx("写入文件失败: {}", e))
}

/// 读取文件全文（CompactEditor file tab 外部变化 reload 用）。
#[tauri::command]
pub fn read_file_text(path: String) -> Result<String, String> {
    std::fs::read_to_string(&path).map_err(|e| e2s_ctx("读取文件失败: {}", e))
}

/// 按格式格式化文件路径列表（copy_path 动作用）。
/// format: "plain"（纯路径）/ "url"（file:// URL）/ "quoted"（带引号），其他值同 plain。
pub fn format_paths(files: &[String], format: &str) -> String {
    match format {
        "url" => files.iter().map(|f| format!("file://{}", url_encode_path(f))).collect::<Vec<_>>().join("\n"),
        "quoted" => files.iter().map(|f| format!("\"{}\"", f)).collect::<Vec<_>>().join("\n"),
        _ => files.join("\n"),
    }
}

/// 从文件列表推导工作目录：首个文件的父目录，无文件时 fallback HOME 或 /tmp。
pub fn derive_cwd(files: &[String]) -> String {
    files.first()
        .and_then(|f| std::path::Path::new(f).parent().map(|p| p.to_string_lossy().to_string()))
        .unwrap_or_else(|| std::env::var("HOME").unwrap_or_else(|_| "/tmp".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── render_agent_prompt ──

    #[test]
    fn test_render_agent_prompt_with_task_and_files() {
        let prompt = render_agent_prompt(
            "{{voice}}\n\n文件列表：\n{{files}}",
            "制作PPT",
            "",
            &["/a.pdf".into(), "/b.pdf".into()],
        );
        assert_eq!(prompt, "制作PPT\n\n文件列表：\n/a.pdf\n/b.pdf");
    }

    #[test]
    fn test_render_agent_prompt_no_task_placeholder() {
        // 无 {{voice}}——task 参数被忽略
        let prompt = render_agent_prompt("整理这些文件：{{files}}", "ignored task", "", &["/a".into()]);
        assert_eq!(prompt, "整理这些文件：/a");
    }

    #[test]
    fn test_render_agent_prompt_no_files_placeholder() {
        let prompt = render_agent_prompt("执行：{{voice}}", "do something", "", &["/a".into()]);
        assert_eq!(prompt, "执行：do something");
    }

    #[test]
    fn test_render_agent_prompt_no_placeholders() {
        let prompt = render_agent_prompt("固定命令", "ignored", "", &[]);
        assert_eq!(prompt, "固定命令");
    }

    #[test]
    fn test_render_agent_prompt_empty_task() {
        let prompt = render_agent_prompt("{{voice}}", "", "", &[]);
        assert_eq!(prompt, "");
    }

    #[test]
    fn test_render_agent_prompt_text_placeholder() {
        // {{text}} 占位符 = 选中文本（独立于 {{voice}}）
        let prompt = render_agent_prompt("阅读这段内容：\n\n{{text}}", "", "这是一段选中文字", &[]);
        assert_eq!(prompt, "阅读这段内容：\n\n这是一段选中文字");
    }

    #[test]
    fn test_render_agent_prompt_task_and_text_separate() {
        // {{voice}}（用户指令）和 {{text}}（选中文本）语义分离
        let prompt = render_agent_prompt("指令：{{voice}}\n内容：{{text}}", "归类这段话", "选中的话", &[]);
        assert_eq!(prompt, "指令：归类这段话\n内容：选中的话");
    }

    #[test]
    fn test_render_agent_prompt_empty_files() {
        let prompt = render_agent_prompt("文件：{{files}}", "task", "text", &[]);
        assert_eq!(prompt, "文件：");
    }

    #[test]
    fn test_render_agent_prompt_multiple_files() {
        let prompt = render_agent_prompt("{{files}}", "", "", &["/a".into(), "/b".into(), "/c".into()]);
        assert_eq!(prompt, "/a\n/b\n/c");
    }

    // ── resolve_prompt_reference（纯逻辑分支；文件存在/不存在走集成测试）──

    #[test]
    fn test_resolve_prompt_reference_plain_text() {
        // 不以 @ 开头 → 原文返回
        assert_eq!(resolve_prompt_reference("润色这段文字"), "润色这段文字");
        assert_eq!(resolve_prompt_reference(""), "");
    }

    #[test]
    fn test_resolve_prompt_reference_at_alone() {
        // @ 单独 → 不当引用，原文返回（trim 后 name 为空）
        assert_eq!(resolve_prompt_reference("@"), "@");
        assert_eq!(resolve_prompt_reference("@ "), "@ "); // 原文含尾部空格
    }

    #[test]
    fn test_resolve_prompt_reference_mixed_text() {
        // 文本中间有 @ → 不是纯引用（strip_prefix 后整段含非引用字符），但代码只看首字符
        // 「请处理：@tolaria」不以 @ 开头 → 原文
        assert_eq!(resolve_prompt_reference("请处理：@tolaria"), "请处理：@tolaria");
    }

    #[test]
    fn test_resolve_prompt_reference_file_not_exist() {
        // @文件名 但文件不存在 → 降级为原文（@nonexistent-xyz-123）
        let result = resolve_prompt_reference("@nonexistent-xyz-123");
        assert_eq!(result, "@nonexistent-xyz-123", "文件不存在时应降级为原文");
    }

    // ── format_paths ──

    #[test]
    fn test_format_paths_plain() {
        let result = format_paths(&["/a.pdf".into(), "/b.pdf".into()], "plain");
        assert_eq!(result, "/a.pdf\n/b.pdf");
    }

    #[test]
    fn test_format_paths_url() {
        let result = format_paths(&["/a/b.pdf".into()], "url");
        assert_eq!(result, "file:///a/b.pdf");
    }

    #[test]
    fn test_format_paths_url_with_spaces() {
        let result = format_paths(&["/a/b c.pdf".into()], "url");
        assert_eq!(result, "file:///a/b%20c.pdf");
    }

    #[test]
    fn test_format_paths_quoted() {
        let result = format_paths(&["/a.pdf".into(), "/b.pdf".into()], "quoted");
        assert_eq!(result, "\"/a.pdf\"\n\"/b.pdf\"");
    }

    #[test]
    fn test_format_paths_unknown_format_defaults_plain() {
        let result = format_paths(&["/a".into()], "unknown");
        assert_eq!(result, "/a");
    }

    #[test]
    fn test_format_paths_empty_list() {
        assert_eq!(format_paths(&[], "plain"), "");
        assert_eq!(format_paths(&[], "url"), "");
        assert_eq!(format_paths(&[], "quoted"), "");
    }

    // ── url_encode_path ──

    #[test]
    fn test_url_encode_path_no_spaces() {
        assert_eq!(url_encode_path("/a/b.pdf"), "/a/b.pdf");
    }

    #[test]
    fn test_url_encode_path_multiple_spaces() {
        assert_eq!(url_encode_path("/a/b c d.pdf"), "/a/b%20c%20d.pdf");
    }

    // ── derive_cwd ──

    #[test]
    fn test_derive_cwd_from_file() {
        let cwd = derive_cwd(&["/Users/x/projects/file.pdf".into()]);
        assert_eq!(cwd, "/Users/x/projects");
    }

    #[test]
    fn test_derive_cwd_root_file() {
        let cwd = derive_cwd(&["/file.txt".into()]);
        assert_eq!(cwd, "/");
    }

    #[test]
    fn test_derive_cwd_empty_files_fallback() {
        // 空列表——fallback 到 HOME 或 /tmp（不验证具体值，只验证不 panic）
        let cwd = derive_cwd(&[]);
        assert!(!cwd.is_empty());
    }
}

