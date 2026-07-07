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
fn builtin_themes() -> Vec<ThemeInfo> {
    vec![
        ThemeInfo {
            id: "light".into(),
            name: "Light".into(),
            description: "暖灰浅色（默认）".into(),
            blur: false,
            colors: ThemeColors {
                background: "#fafaf9".into(),
                foreground: "#44403c".into(),
                primary: "#1c1917".into(),
                primary_foreground: "#fafaf9".into(),
                muted: "#f5f4f0".into(),
                muted_foreground: "#a8a29e".into(),
                accent: "#f5f4f0".into(),
                accent_foreground: "#1c1917".into(),
                border: "#e7e5e0".into(),
                voice: "#d97706".into(),
            },
        },
        ThemeInfo {
            id: "glass-dark".into(),
            name: "Glass Dark".into(),
            description: "半透明深色玻璃质感".into(),
            blur: true,
            colors: ThemeColors {
                background: "rgba(22, 22, 26, 0.52)".into(),
                foreground: "#f5f5f7".into(),
                primary: "#f5f5f7".into(),
                primary_foreground: "#1c1917".into(),
                muted: "rgba(255, 255, 255, 0.06)".into(),
                muted_foreground: "#a8a8b3".into(),
                accent: "rgba(255, 255, 255, 0.14)".into(),
                accent_foreground: "#ffffff".into(),
                border: "rgba(255, 255, 255, 0.10)".into(),
                voice: "#f59e0b".into(),
            },
        },
        ThemeInfo {
            id: "dracula".into(),
            name: "Dracula".into(),
            description: "深紫暗色经典配色".into(),
            blur: true,
            colors: ThemeColors {
                background: "rgba(40, 42, 54, 0.7)".into(),
                foreground: "#f8f8f2".into(),
                primary: "#f8f8f2".into(),
                primary_foreground: "#282a36".into(),
                muted: "rgba(68, 71, 90, 0.5)".into(),
                muted_foreground: "#6272a4".into(),
                accent: "#44475a".into(),
                accent_foreground: "#f8f8f2".into(),
                border: "rgba(98, 114, 164, 0.3)".into(),
                voice: "#ff79c6".into(),
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
