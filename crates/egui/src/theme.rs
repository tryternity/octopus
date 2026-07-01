//! 视觉主题：深色基底 + indigo 强调色 + 来源徽标色编码。
//! 配合 frontend-design 的设计判断（一个辨识色 + 克制不堆砌），落地到 egui Visuals/Style。
//! 字段名按 egui 0.34（panel_fill 非 panel_bg；Widgets 无 selected——选中态由 selection.bg_fill 承担）。

use egui::Color32;
use octopus_notepad::NoteSource;

/// 强调色（indigo-400，暗底下够亮），用于选中/hover/链接。
pub const ACCENT: Color32 = Color32::from_rgb(129, 140, 248);
const ACCENT_DIM: Color32 = Color32::from_rgb(99, 102, 241); // indigo-500（选中底）

/// 次要文字色（zinc-400），用于标签/元信息。
pub const MUTED: Color32 = Color32::from_rgb(161, 161, 170);

/// 应用自定义深色主题 + spacing/圆角。
pub fn setup(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();

    // 面板/窗口背景层次（zinc-900/950，比 egui 默认深一点有质感）
    v.panel_fill = Color32::from_rgb(24, 24, 27); // zinc-950：面板/窗口主背景
    v.faint_bg_color = Color32::from_rgb(30, 30, 35);
    v.extreme_bg_color = Color32::from_rgb(18, 18, 21); // TextEdit/凹陷区，比面板略深
    v.window_fill = Color32::from_rgb(30, 30, 35);

    // 选中 / 链接用强调色
    v.selection.bg_fill = ACCENT_DIM;
    v.selection.stroke = egui::Stroke::new(1.0, ACCENT);
    v.hyperlink_color = ACCENT;

    // widget 配色：inactive 用 zinc-800，hover/active 用强调色半透
    // （egui 0.34 的 Widgets 无 selected 变体——选中态由 selection.bg_fill 统一承担）
    v.widgets.noninteractive.bg_fill = Color32::from_rgb(39, 39, 42);
    // panel 间分隔线（SidePanel↔CentralPanel 边界）：默认偏黑偏突兀，调 zinc-700 细线弱化。
    v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, Color32::from_rgb(63, 63, 70));
    v.widgets.inactive.bg_fill = Color32::from_rgb(39, 39, 42); // zinc-800
    v.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, Color32::from_rgb(212, 212, 216));
    v.widgets.hovered.bg_fill = ACCENT.linear_multiply(0.35);
    v.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, Color32::WHITE);
    v.widgets.active.bg_fill = ACCENT.linear_multiply(0.55);
    v.widgets.active.fg_stroke = egui::Stroke::new(1.0, Color32::WHITE);

    // 圆角统一（0.34：Rounding→CornerRadius，字段 rounding→corner_radius，same 取 u8）
    let r = egui::CornerRadius::same(6);
    v.widgets.noninteractive.corner_radius = r;
    v.widgets.inactive.corner_radius = r;
    v.widgets.hovered.corner_radius = r;
    v.widgets.active.corner_radius = r;

    ctx.set_visuals(v);

    // spacing：item_spacing + button_padding（window_margin 是 Margin 类型，留默认）
    let mut style = (*ctx.global_style()).clone();
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(10.0, 4.0);
    ctx.set_global_style(style);
}

/// 笔记来源徽标色（列表项前小圆点 ●）。
pub fn source_color(src: NoteSource) -> Color32 {
    match src {
        NoteSource::Asr => Color32::from_rgb(96, 165, 250), // blue-400（语音）
        NoteSource::Ocr => Color32::from_rgb(251, 146, 60), // orange-400（OCR）
        NoteSource::Clipboard => Color32::from_rgb(192, 132, 252), // purple-400
        NoteSource::Manual => Color32::from_rgb(161, 161, 170), // zinc-400
    }
}
