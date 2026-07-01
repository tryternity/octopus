//! NotepadView：列表 + md 源码编辑 + egui_commonmark 分屏预览 + 5 按钮工具栏。
//! 直连 octopus_notepad::store（经 octopus_infra::db::with_db 用本进程全局连接，WAL）。
//! 编辑走 800ms 防抖保存（对齐原 webview 行为）。

use crate::ipc::IpcMsg;
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use octopus_notepad::{Note, NoteFilter, NoteSource, NoteType};
use std::time::{Duration, Instant};

const DEBOUNCE: Duration = Duration::from_millis(800);

pub struct NotepadView {
    notes: Vec<Note>,
    current_id: Option<i64>,
    current_type: NoteType,            // 当前笔记类型（text→纯编辑器；markdown→md 工具栏+可收起预览）
    title: String,
    body: String,                       // md/text 源码（编辑缓冲）
    body_dirty: bool,
    last_edit: Option<Instant>,
    pending_select: Option<i64>,        // IPC open 收到、待选中
    refresh_pending: bool,              // IPC notes_changed
    show_preview: bool,                 // markdown 预览开关（仅 Markdown 笔记生效，可手动收起）
    md_cache: CommonMarkCache,          // egui_commonmark 解析缓存（跨帧复用）
}

impl Default for NotepadView {
    fn default() -> Self {
        let mut v = Self {
            notes: Vec::new(),
            current_id: None,
            current_type: NoteType::Text,
            title: String::new(),
            body: String::new(),
            body_dirty: false,
            last_edit: None,
            pending_select: None,
            refresh_pending: false,
            show_preview: false,
            md_cache: CommonMarkCache::default(),
        };
        v.reload_notes();
        // 默认选第一条
        if let Some(first) = v.notes.first().map(|n| n.id) {
            v.select(first);
        }
        v
    }
}

impl NotepadView {
    /// 处理一条 IPC 消息。
    pub fn handle_ipc(&mut self, msg: IpcMsg) {
        match msg {
            IpcMsg::Open { note_id } => {
                self.pending_select = Some(note_id);
            }
            IpcMsg::NotesChanged => {
                self.refresh_pending = true;
            }
            IpcMsg::Show => {
                // show/focus 由 main.rs 的 eframe 层处理（这里无操作）
            }
        }
    }

    fn reload_notes(&mut self) {
        self.notes = octopus_infra::db::with_db(|conn| {
            octopus_notepad::store::list_notes_at(conn, &NoteFilter::default())
        })
        .unwrap_or_default();
    }

    /// 选中某笔记：先把当前 dirty 落库，再载入选中。
    fn select(&mut self, id: i64) {
        self.flush_if_dirty();
        let note = octopus_infra::db::with_db(|conn| {
            octopus_notepad::store::get_note_at(conn, id)
        })
        .ok()
        .flatten();
        if let Some(n) = note {
            self.current_id = Some(n.id);
            self.current_type = n.note_type;
            self.title = n.title.unwrap_or_default();
            self.body = n.content_text;
            self.body_dirty = false;
            self.last_edit = None;
            // 预览仅 markdown 默认开（text 无需预览）；切笔记按新 type 重置。
            self.show_preview = matches!(self.current_type, NoteType::Markdown);
        }
    }

    fn mark_dirty(&mut self) {
        self.body_dirty = true;
        self.last_edit = Some(Instant::now());
    }

    /// 防抖落库：距上次编辑 ≥ DEBOUNCE 才写。
    fn flush_if_dirty(&mut self) {
        if !self.body_dirty {
            return;
        }
        if let Some(t) = self.last_edit {
            if t.elapsed() >= DEBOUNCE {
                self.save_current();
            }
        }
    }

    fn save_current(&mut self) {
        let Some(id) = self.current_id else { return };
        let title = self.title.clone();
        let body = self.body.clone();
        let _ = octopus_infra::db::with_db(|conn| {
            octopus_notepad::store::update_note_at(conn, id, &title, &body, self.current_type)
        });
        self.body_dirty = false;
        self.last_edit = None;
        self.reload_notes(); // 列表 updated_at 刷新
    }

    /// 新建空笔记（source=manual, type=text）并选中。
    fn create_new(&mut self) {
        self.flush_if_dirty(); // 先存当前未保存内容
        if let Ok(id) = octopus_infra::db::with_db(|conn| {
            octopus_notepad::store::create_note_at(conn, NoteSource::Manual, None, "", NoteType::Text)
        }) {
            self.reload_notes();
            self.select(id);
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui) {
        // 退出前 flush（ctx 即将 drop 不易感知，靠防抖 + 切换笔记 flush 兜底）
        self.flush_if_dirty();

        // 处理待选 / 刷新
        if let Some(id) = self.pending_select.take() {
            self.select(id);
            self.reload_notes();
        }
        if self.refresh_pending {
            self.refresh_pending = false;
            self.reload_notes();
        }

        // 左栏：列表（来源徽标色 + pinned 星标）。
        // 固定宽度（exact_width）：resizable 的拖动 handle 在本主题下渲染成粗黑线、且放开后
        // 弹回默认宽（egui memory 不落盘，进程内也不持久）。列表 260 固定够用，去掉 handle。
        egui::Panel::left("list")
            .resizable(false)
            .exact_size(260.0)
            .show_separator_line(false) // 不画 panel 边界竖线（dark 主题下偏粗黑）
            .show_inside(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("笔记").strong().size(15.0));
                // 按钮推到右：用 right_to_left 子布局，而非 allocate_space(available_width)。
                // 后者在 panel content ui 内返回值超出 content 区，把 horizontal 的 min_rect
                // 撑过 panel 宽 → frame response rect 外扩（实测 panel 从 260 撑到 276，显黑区且会抖）。
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("+ 新建").clicked() {
                        self.create_new();
                    }
                });
            });
            ui.add_space(6.0);
            ui.separator();

            let mut select_id: Option<i64> = None;
            egui::ScrollArea::vertical().show(ui, |ui| {
                for n in &self.notes {
                    let selected = self.current_id == Some(n.id);
                    // 无标题时用内容当标题：split_whitespace 去掉所有分行/多余空白成单行
                    // （多行内容直接显示会撑高列表项、很难看），统一截断 24 字符 + …。
                    let raw = n.title.clone().unwrap_or_else(|| {
                        n.content_text.split_whitespace().collect::<Vec<_>>().join(" ")
                    });
                    let label_text = if raw.trim().is_empty() {
                        "（空）".to_owned()
                    } else {
                        truncate_label(&raw, 24)
                    };
                    // 选中态背景条（indigo 半透 + 圆角），比仅文字加粗更醒目；
                    // Frame 在 panel content ui 内、不用 available_width，不撑宽 panel（有测试保障）。
                    let frame = egui::Frame::NONE
                        .corner_radius(egui::CornerRadius::same(4))
                        .inner_margin(egui::Margin::symmetric(6, 3))
                        .fill(if selected {
                            crate::theme::ACCENT.linear_multiply(0.2)
                        } else {
                            egui::Color32::TRANSPARENT
                        });
                    frame.show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            ui.label(
                                egui::RichText::new("●")
                                    .color(crate::theme::source_color(n.source))
                                    .size(9.0),
                            );
                            if n.is_pinned {
                                ui.label(
                                    egui::RichText::new("★")
                                        .color(egui::Color32::from_rgb(250, 204, 21))
                                        .size(11.0),
                                );
                            }
                            let rt = egui::RichText::new(&label_text);
                            let rt = if selected {
                                rt.strong().color(egui::Color32::WHITE)
                            } else {
                                rt
                            };
                            if ui.selectable_label(selected, rt).clicked() {
                                select_id = Some(n.id);
                            }
                        });
                    });
                }
            });
            if let Some(id) = select_id {
                self.select(id);
            }
        });

        // 右栏：编辑器
        egui::CentralPanel::default().show_inside(ui, |ui| {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                // 笔记总数
                ui.label(
                    egui::RichText::new(format!("共 {} 条", self.notes.len()))
                        .small()
                        .color(crate::theme::MUTED),
                );
                ui.separator();
                ui.label(egui::RichText::new("标题").small().color(crate::theme::MUTED));
                let resp = ui.text_edit_singleline(&mut self.title);
                if resp.changed() {
                    self.mark_dirty();
                }
                // 保存按钮推到右：right_to_left 子布局（同 list header，避免 allocate_space 撑宽）
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let label = if self.body_dirty { "保存 *" } else { "保存" };
                    if ui.button(label).clicked() {
                        self.save_current();
                    }
                    // 预览切换（仅 markdown；right_to_left 下先 add 者在最右，故预览在保存左侧）
                    if matches!(self.current_type, NoteType::Markdown) {
                        let plabel = if self.show_preview { "预览 ✓" } else { "预览" };
                        if ui.button(plabel).clicked() {
                            self.show_preview = !self.show_preview;
                        }
                    }
                });
            });
            ui.add_space(2.0);
            ui.separator();

            // markdown 笔记：md 工具栏 + 编辑/预览（可收起）；
            // text 笔记：纯文本编辑器（无 md 工具栏、无预览——纯文本无需 md 语法/预览）。
            if matches!(self.current_type, NoteType::Markdown) {
                toolbar(ui, &mut self.body, &mut self.body_dirty, &mut self.last_edit);

                if self.show_preview {
                    // 编辑 / 预览分屏：ui.columns 等宽两列（egui 标准等宽分栏 API，内部 allocate，
                    // 不依赖手算 avail —— 手算的 allocate_ui_with_layout 在非交互帧尺寸会漂移）
                    ui.columns(2, |cols| {
                        cols[0].vertical(|ui| {
                            editor_pane(
                                ui,
                                &mut self.body,
                                &mut self.body_dirty,
                                &mut self.last_edit,
                                "Markdown 源码",
                            );
                        });
                        cols[1].vertical(|ui| {
                            ui.label(
                                egui::RichText::new("预览")
                                    .small()
                                    .color(crate::theme::MUTED)
                                    .strong(),
                            );
                            ui.add_space(2.0);
                            // auto_shrink 默认 true：预览内容短时 ScrollArea 会缩到内容高度，
                            // 与左列编辑器底部不对齐、下方留白。关掉，固定占满列高。
                            egui::ScrollArea::vertical()
                                .auto_shrink([false; 2])
                                .show(ui, |ui| {
                                    CommonMarkViewer::new().show(ui, &mut self.md_cache, &self.body);
                                });
                        });
                    });
                } else {
                    // 预览收起：编辑器单列占满
                    editor_pane(
                        ui,
                        &mut self.body,
                        &mut self.body_dirty,
                        &mut self.last_edit,
                        "Markdown 源码",
                    );
                }
            } else {
                // text 笔记：纯文本编辑器占满
                editor_pane(
                    ui,
                    &mut self.body,
                    &mut self.body_dirty,
                    &mut self.last_edit,
                    "正文",
                );
            }
        });
        // 持续 repaint 让防抖 timer 可被 poll
        if self.body_dirty {
            ui.ctx().request_repaint_after(DEBOUNCE);
        }
    }
}

/// 文本编辑区：标签 + 滚动 multiline TextEdit。changed 时置 dirty + last_edit
/// （防抖 flush_if_dirty 依赖 last_edit，仅置 dirty 不置 last_edit 会导致永不落库）。
fn editor_pane(
    ui: &mut egui::Ui,
    body: &mut String,
    dirty: &mut bool,
    last_edit: &mut Option<Instant>,
    label: &str,
) {
    ui.label(egui::RichText::new(label).small().color(crate::theme::MUTED).strong());
    ui.add_space(2.0);
    // 编辑器撑满视口 + 内容超出可滚动。egui TextEdit 本身无可见滚动条：固定高度
    // (add_sized) 时内容超出会截断底部、无法手动滚动（用户反馈「下面文字看不见」）。
    // 正确做法：外层 ScrollArea 占 available_height；内层 TextEdit desired_rows 取
    // max(视口可容行数, 实际内容行)——短内容占满视口不留白，长内容超出由 ScrollArea 滚动，
    // 且输入光标自动 scroll_to_cursor 跟入视野。
    let line_h = ui.text_style_height(&egui::TextStyle::Body);
    let min_rows = ((ui.available_height() / line_h).floor() as usize).max(1);
    egui::ScrollArea::vertical()
        .auto_shrink([false; 2])
        .show(ui, |ui| {
            let resp = ui.add(
                egui::TextEdit::multiline(body)
                    .desired_width(f32::INFINITY)
                    .desired_rows(min_rows),
            );
            if resp.changed() {
                *dirty = true;
                *last_edit = Some(Instant::now());
            }
        });
}

/// 5 按钮工具栏：点按钮在末尾插入 md 语法标记。
fn toolbar(
    ui: &mut egui::Ui,
    body: &mut String,
    dirty: &mut bool,
    last_edit: &mut Option<Instant>,
) {
    ui.horizontal_wrapped(|ui| {
        let pairs: &[(&str, &str, &str)] = &[
            ("粗体", "**", "**"),
            ("斜体", "*", "*"),
            ("标题", "# ", ""),
            ("列表", "- ", ""),
            ("代码", "`", "`"),
        ];
        for (label, pre, post) in pairs {
            if ui.small_button(egui::RichText::new(*label).small()).clicked() {
                wrap_selection_or_append(body, pre, post);
                *dirty = true;
                *last_edit = Some(Instant::now());
            }
        }
    });
    ui.add_space(2.0);
    ui.separator();
}

/// 简化版：在末尾追加 pre+post（egui 0.29 的 TextEdit 选区 API 有限，
/// 第一版用末尾插入语法标记，对用户可见可点；选区包覆留作后续优化）。
fn wrap_selection_or_append(body: &mut String, pre: &str, post: &str) {
    body.push_str(pre);
    body.push_str(post);
}

/// 截断到 max_chars 字符，超出加省略号「…」（列表项标题/内容预览单行显示用）。
fn truncate_label(s: &str, max_chars: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_chars {
        s.to_owned()
    } else {
        let mut t: String = chars[..max_chars].iter().collect();
        t.push('…');
        t
    }
}
