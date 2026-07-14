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
) -> Option<SurroundingText> {
    let packages_dir = find_sublime_packages_dir(bundle_id)?;

    // 1. 确保插件已安装
    let plugin_path = packages_dir.join("Octopus").join(SUBLIME_PLUGIN_NAME);
    ensure_plugin_installed(&plugin_path);

    // 2. 删除旧的输出文件
    let _ = std::fs::remove_file(SUBLIME_OUTPUT_PATH);

    // 3. 触发插件命令
    let subl_path = find_subl_binary()?;
    let result = std::process::Command::new(&subl_path)
        .arg("--command")
        .arg("octopus_export_context")
        .output();

    if result.is_err() {
        log::warn!("[app-context] subl --command 执行失败");
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
fn find_subl_binary() -> Option<std::path::PathBuf> {
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
    std::process::Command::new("which")
        .arg("subl")
        .output()
        .ok()
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
