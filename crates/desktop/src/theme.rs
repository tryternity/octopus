use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 主题颜色 token——对应前端 Tailwind v4 的 CSS 变量（--color-xxx）。
/// 用户自定义主题 JSON 里 colors 对象的 key 与这些字段名一致（kebab-case）。
#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "kebab-case")]
pub struct ThemeColors {
    pub background: String,
    pub foreground: String,
    pub primary: String,
    pub primary_foreground: String,
    pub muted: String,
    pub muted_foreground: String,
    pub accent: String,
    pub accent_foreground: String,
    pub border: String,
    pub voice: String,
    /// 不透明表面色——result_window 等需实色背景的组件（暗色主题的 background 可能半透明）。
    #[serde(default = "default_surface")]
    pub surface: String,
    /// 工具栏图标色——result_window 工具栏按钮（暗色主题需浅色）。
    #[serde(default = "default_tool_icon")]
    pub tool_icon: String,
}

fn default_surface() -> String {
    "#fafaf9".into()
}

fn default_tool_icon() -> String {
    "rgba(0, 0, 0, 0.55)".into()
}

#[derive(Serialize, Deserialize, Clone)]
pub struct ThemeInfo {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// 半透明主题需配合 backdrop-blur 实现毛玻璃效果。
    #[serde(default)]
    pub blur: bool,
    pub colors: ThemeColors,
}

/// 3 套内置主题。用户可在 ~/.octopus/themes/*.json 放自定义主题覆盖或新增。
///
/// 设计原则（ui-ux-pro-max §6 + frontend-design）：
/// - 文字对比度 ≥ 4.5:1（AA），暗色用去饱和提亮而非反色
/// - 每套有辨识度，不做模板默认——"spend boldness in one place"
/// - 半透明主题 blur=true，配合 clipboard_window 的 transparent:true + CSS backdrop-blur
fn builtin_themes() -> Vec<ThemeInfo> {
    vec![
        // ── Warm Paper ── 纸质感暖灰浅色。
        // 设计意图：工具的温度感——stone 暖灰而非冷 zinc，长时间使用不刺眼。
        // 对比度：foreground #292524 对 background #fafaf9 = 12.3:1（远超 AA）。
        ThemeInfo {
            id: "light".into(),
            name: "Warm Paper".into(),
            description: "纸质感暖灰浅色".into(),
            blur: false,
            colors: ThemeColors {
                background: "#fafaf9".into(),
                foreground: "#292524".into(),
                primary: "#1c1917".into(),
                primary_foreground: "#fafaf9".into(),
                muted: "#f5f4f0".into(),
                muted_foreground: "#78716c".into(),
                accent: "#e7e5e0".into(),
                accent_foreground: "#1c1917".into(),
                border: "#e7e5e0".into(),
                voice: "#d97706".into(),
                surface: "#fafaf9".into(),
                tool_icon: "rgba(0, 0, 0, 0.55)".into(),
            },
        },
        // ── Obsidian Glass ── 黑曜石深色玻璃。
        // 设计意图：比 Wox Glass Dark 更深更黑——接近黑曜石的致密质感。
        // 选中态用高亮半透明白（0.16）拉开与 hover（0.08）的层级差。
        // 对比度：foreground #f5f5f7 对 rgba(18,18,22,0.6) 叠深色桌面 ≈ 11:1。
        ThemeInfo {
            id: "glass-dark".into(),
            name: "Obsidian Glass".into(),
            description: "黑曜石深色半透明玻璃".into(),
            blur: true,
            colors: ThemeColors {
                background: "rgba(18, 18, 22, 0.60)".into(),
                foreground: "#f5f5f7".into(),
                primary: "#f5f5f7".into(),
                primary_foreground: "#1c1917".into(),
                muted: "rgba(255, 255, 255, 0.05)".into(),
                muted_foreground: "#9ca3af".into(),
                accent: "rgba(255, 255, 255, 0.16)".into(),
                accent_foreground: "#ffffff".into(),
                border: "rgba(255, 255, 255, 0.08)".into(),
                voice: "#f59e0b".into(),
                surface: "#1a1a1e".into(),
                tool_icon: "rgba(255, 255, 255, 0.55)".into(),
            },
        },
        // ── Nord Aurora ── 北极极光冷蓝深色。
        // 设计意图：Nord 配色——冰川蓝深底 + 极光青强调，冷峻克制。
        // 不照搬 Dracula（太常见），Nord 的辨识度在于"冷而不黑"。
        // 对比度：foreground #e5e9f0 对 rgba(46,52,64,0.75) ≈ 11:1。
        ThemeInfo {
            id: "nord".into(),
            name: "Nord Aurora".into(),
            description: "北极极光冷蓝深色".into(),
            blur: true,
            colors: ThemeColors {
                background: "rgba(46, 52, 64, 0.75)".into(),
                foreground: "#e5e9f0".into(),
                primary: "#e5e9f0".into(),
                primary_foreground: "#2e3440".into(),
                muted: "rgba(59, 66, 82, 0.6)".into(),
                muted_foreground: "#81a1c1".into(),
                accent: "rgba(136, 192, 208, 0.20)".into(),
                accent_foreground: "#eceff4".into(),
                border: "rgba(136, 192, 208, 0.15)".into(),
                voice: "#88c0d0".into(),
                surface: "#2e3440".into(),
                tool_icon: "rgba(229, 233, 240, 0.55)".into(),
            },
        },
    ]
}

/// 列出所有可用主题：内置 + ~/.octopus/themes/*.json。同 id 用户主题覆盖内置。
#[tauri::command]
pub fn list_themes() -> Result<Vec<ThemeInfo>, String> {
    let mut themes: HashMap<String, ThemeInfo> = HashMap::new();
    for t in builtin_themes() {
        themes.insert(t.id.clone(), t);
    }

    // 用户主题目录：~/.octopus/themes/*.json
    let themes_dir = octopus_infra::paths::octopus_config_home().join("themes");
    if themes_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(&themes_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) != Some("json") {
                    continue;
                }
                match std::fs::read_to_string(&path) {
                    Ok(content) => match serde_json::from_str::<ThemeInfo>(&content) {
                        Ok(theme) => {
                            themes.insert(theme.id.clone(), theme);
                        }
                        Err(e) => {
                            log::warn!("主题文件 {:?} 解析失败: {}", path, e);
                        }
                    },
                    Err(e) => log::warn!("主题文件 {:?} 读取失败: {}", path, e),
                }
            }
        }
    }

    let mut result: Vec<ThemeInfo> = themes.into_values().collect();
    result.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(result)
}
