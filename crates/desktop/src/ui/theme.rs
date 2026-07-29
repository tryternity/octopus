use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use crate::core::error_util::e2s;
use std::sync::OnceLock;

/// 主题颜色 token——对应前端 Tailwind v4 的 CSS 变量（--color-xxx）。

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
    /// 截图工具栏图标 CSS filter——暗色主题需反色让黑色 SVG 图标可见。
    #[serde(default = "default_icon_filter")]
    pub icon_filter: String,
}

fn default_surface() -> String {
    "#fafaf9".into()
}

fn default_tool_icon() -> String {
    "rgba(0, 0, 0, 0.55)".into()
}

fn default_icon_filter() -> String {
    "none".into()
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

/// 4 套内置主题。用户可在 ~/.octopus/themes/*.json 放自定义主题覆盖或新增。
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
                icon_filter: "none".into(),
            },
        },
        // ── Obsidian Glass ── 黑曜石深色。
        // 设计意图：致密黑曜石质感——剪贴板是文字密集窗口，可读性优先于玻璃透感。
        // α 0.96 几乎不透明，消除背后内容透出（滑动鼠标时尤为明显）。
        // 选中态用高亮半透明白（0.16）拉开与 hover（0.08）的层级差。
        ThemeInfo {
            id: "glass-dark".into(),
            name: "Obsidian Glass".into(),
            description: "黑曜石深色".into(),
            blur: false,
            colors: ThemeColors {
                background: "#121216".into(),
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
                icon_filter: "brightness(0) invert(1)".into(),
            },
        },
        // ── Nord Aurora ── 北极极光冷蓝深色。
        // 设计意图：Nord 配色——冰川蓝深底 + 极光青强调，冷峻克制。
        // 不照搬 Dracula（太常见），Nord 的辨识度在于"冷而不黑"。
        // α 0.96 几乎不透明，消除背后内容透出。
        ThemeInfo {
            id: "nord".into(),
            name: "Nord Aurora".into(),
            description: "北极极光冷蓝深色".into(),
            blur: false,
            colors: ThemeColors {
                background: "#2e3440".into(),
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
                icon_filter: "brightness(0) invert(1)".into(),
            },
        },
        // ── Raycast ── 精准仪器深色。
        // 设计意图：DESIGN.md 的 macOS 原生工具感——近黑蓝底 #07080a（非纯黑，蓝冷偏移），
        // 几乎不可见的 rgba(255,255,255,0.08) 结构边框，配多层 inset 阴影模拟物理深度。
        // voice 用应用图标浅蓝 #6EB5FF（≈ Raycast Blue hsl(210,100%,71%)），贴合 octopus 品牌
        // 又落在 DESIGN.md 的交互蓝区间。对比度：foreground #f9f9f9 对 background #07080a ≈ 17:1。
        // 状态色（success/info/warning/destructive）在 index.css [data-theme="raycast"] 硬编码。
        ThemeInfo {
            id: "raycast".into(),
            name: "Raycast".into(),
            description: "精准仪器深色".into(),
            blur: false,
            colors: ThemeColors {
                background: "#07080a".into(),
                foreground: "#f9f9f9".into(),
                primary: "#f9f9f9".into(),
                primary_foreground: "#07080a".into(),
                muted: "rgba(255, 255, 255, 0.05)".into(),
                muted_foreground: "#9c9c9d".into(),
                accent: "rgba(255, 255, 255, 0.06)".into(),
                accent_foreground: "#ffffff".into(),
                border: "rgba(255, 255, 255, 0.08)".into(),
                voice: "#6eb5ff".into(),
                surface: "#101111".into(),
                tool_icon: "rgba(255, 255, 255, 0.55)".into(),
                icon_filter: "brightness(0) invert(1)".into(),
            },
        },
        // ── Azure Mist ── HashiCorp 风明亮浅蓝。
        // 设计意图（参考 DESIGN.md「HashiCorp」）：enterprise infra made approachable——
        // 清白底 + 深靛蓝按钮 + sky blue 品牌色。色值贴 DESIGN.md：
        // - muted-foreground 直接用 Dark Gray #656a76（helper text token）
        // - primary 用 Link Blue #2264d6 的深化变体 #1563a8（白字 ~7.6:1 AAA）——
        //   DESIGN.md 的 Link Blue 作按钮底白字仅 4.6:1，深化保 AAA
        // - voice 用 Bright Blue #2b89ff（active link 色），与 primary 形成深浅层次
        // 对比度：foreground #1a2840 对 background #f6f8fb ≈ 13:1（远超 AA）。
        // 状态色在 index.css [data-theme="azure"] 硬编码（沿用 light 标准值）。
        ThemeInfo {
            id: "azure".into(),
            name: "Azure Mist".into(),
            description: "HashiCorp 风明亮浅蓝".into(),
            blur: false,
            colors: ThemeColors {
                background: "#f6f8fb".into(),
                foreground: "#1a2840".into(),
                primary: "#1563a8".into(),
                primary_foreground: "#ffffff".into(),
                muted: "#edf1f7".into(),
                muted_foreground: "#656a76".into(),
                accent: "#e3edf7".into(),
                accent_foreground: "#1a2840".into(),
                border: "#d8e1ec".into(),
                voice: "#2b89ff".into(),
                surface: "#ffffff".into(),
                tool_icon: "rgba(0, 0, 0, 0.55)".into(),
                icon_filter: "none".into(),
            },
        },
    ]
}

/// 列出所有可用主题：内置 + ~/.octopus/themes/*.json。同 id 用户主题覆盖内置。
/// 结果进程内缓存（OnceLock）——主题列表在运行期不变，避免每次调用读文件系统。
#[tauri::command]
pub fn list_themes() -> Result<Vec<ThemeInfo>, String> {
    static CACHE: OnceLock<Vec<ThemeInfo>> = OnceLock::new();
    Ok(CACHE.get_or_init(load_themes).clone())
}

fn load_themes() -> Vec<ThemeInfo> {
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
    result
}

/// 只返回当前主题 id（轻量，避免前端 applyThemeFromConfig 调全量 get_config）。
#[tauri::command]
pub fn get_theme_id() -> Result<String, String> {
    let id = octopus_infra::db::load_config_key("clipboard_theme")
        .map_err(e2s)?
        .unwrap_or_else(|| "light".into());
    Ok(id)
}

/// 返回指定窗口应使用的背景色 hex（不含 #），用于 URL 注入。
/// 非透明窗口（settings/compact_editor）返回主题背景色；透明窗口返回 None。
/// 各窗口 HTML 脚本读 URL bg 参数直接设 #hex——零 CSS 依赖，首帧即有色。
pub fn window_bg_hex(window_label: &str) -> Option<String> {
    // 白名单：只有常规非透明窗口需要背景色。透明窗口（result/clipboard/screenshot）
    // 不注入——它们靠 transparent:true + body transparent 实现穿透/遮罩。
    let is_opaque = match window_label {
        "settings_window" | "compact_editor_window" => true,
        _ => false,
    };
    if !is_opaque {
        return None;
    }
    // 读当前主题的 background 色——用缓存的 list_themes()（OnceLock），不走 load_themes() 重复扫盘
    let themes = list_themes().unwrap_or_default();
    let theme_id = octopus_infra::db::load_config_key("clipboard_theme")
        .ok()
        .flatten()
        .unwrap_or_else(|| "light".into());
    let theme = themes.iter().find(|t| t.id == theme_id)
        .or_else(|| themes.iter().find(|t| t.id == "light"))?;
    // 从 background 色提取 hex（去掉 # 前缀或 rgba 前缀）
    let bg = &theme.colors.background;
    if let Some(hex) = bg.strip_prefix('#') {
        return Some(hex.to_string());
    }
    // rgba/rgb 格式——原样返回（各窗口 HTML 脚本判断有无 # 决定是否加 #）
    Some(bg.clone())
}
