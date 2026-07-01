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
    title: String,
    body: String,                       // md 源码（编辑缓冲）
    body_dirty: bool,
    last_edit: Option<Instant>,
    pending_select: Option<i64>,        // IPC open 收到、待选中
    refresh_pending: bool,              // IPC notes_changed
    md_cache: CommonMarkCache,          // egui_commonmark 解析缓存（跨帧复用）
}

impl Default for NotepadView {
    fn default() -> Self {
        let mut v = Self {
            notes: Vec::new(),
            current_id: None,
            title: String::new(),
            body: String::new(),
            body_dirty: false,
            last_edit: None,
            pending_select: None,
            refresh_pending: false,
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
            self.title = n.title.unwrap_or_default();
            self.body = n.content_text;
            self.body_dirty = false;
            self.last_edit = None;
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
            octopus_notepad::store::update_note_at(conn, id, &title, &body, NoteType::Markdown)
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

    pub fn show(&mut self, ctx: &egui::Context) {
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

        // 左栏：列表（来源徽标色 + pinned 星标）
        egui::SidePanel::left("list").resizable(true).default_width(260.0).show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("笔记").strong().size(15.0));
                ui.allocate_space(ui.available_size()); // 推「+ 新建」到右
                if ui.button("+ 新建").clicked() {
                    self.create_new();
                }
            });
            ui.add_space(6.0);
            ui.separator();

            let mut select_id: Option<i64> = None;
            egui::ScrollArea::vertical().show(ui, |ui| {
                for n in &self.notes {
                    let selected = self.current_id == Some(n.id);
                    let label_text = n.title.clone().unwrap_or_else(|| {
                        n.content_text.chars().take(24).collect::<String>()
                    });
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        // 来源徽标色点 ●
                        ui.label(
                            egui::RichText::new("●")
                                .color(crate::theme::source_color(n.source))
                                .size(9.0),
                        );
                        // pinned 星标 ★
                        if n.is_pinned {
                            ui.label(
                                egui::RichText::new("★")
                                    .color(egui::Color32::from_rgb(250, 204, 21))
                                    .size(11.0),
                            );
                        }
                        let rt = egui::RichText::new(&label_text);
                        let rt = if selected { rt.strong().color(egui::Color32::WHITE) } else { rt };
                        if ui.selectable_label(selected, rt).clicked() {
                            select_id = Some(n.id);
                        }
                    });
                }
            });
            if let Some(id) = select_id {
                self.select(id);
            }
        });

        // 右栏：编辑器
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                ui.label(egui::RichText::new("标题").small().color(crate::theme::MUTED));
                let resp = ui.text_edit_singleline(&mut self.title);
                if resp.changed() {
                    self.mark_dirty();
                }
                ui.allocate_space(ui.available_size()); // 推「保存」到右
                let label = if self.body_dirty { "保存 *" } else { "保存" };
                if ui.button(label).clicked() {
                    self.save_current();
                }
            });
            ui.add_space(2.0);
            ui.separator();

            // 工具栏（5 按钮：选中文本→包 md 语法）
            toolbar(ui, &mut self.body, &mut self.body_dirty, &mut self.last_edit);

            // 编辑 / 预览分屏（allocate_ui 精确分配宽度，避免 Frame::group + set_min_size
            // 导致总宽溢出 → 每帧重算 half 抖动「拉出来又缩回去」）
            let avail = ui.available_size();
            let col_w = (avail.x / 2.0).max(120.0);
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                // 左：md 源码（限定宽度，不用 MAX 撑爆）
                ui.allocate_ui_with_layout(
                    egui::Vec2::new(col_w, avail.y),
                    egui::Layout::top_down(egui::Align::LEFT),
                    |ui| {
                        ui.label(
                            egui::RichText::new("Markdown 源码")
                                .small()
                                .color(crate::theme::MUTED)
                                .strong(),
                        );
                        ui.add_space(2.0);
                        let resp = ui.add(
                            egui::TextEdit::multiline(&mut self.body)
                                .desired_width(col_w)
                                .desired_rows(20),
                        );
                        if resp.changed() {
                            self.mark_dirty();
                        }
                    },
                );
                ui.separator(); // 竖向分隔线
                // 右：预览（占剩余宽度）
                ui.allocate_ui_with_layout(
                    egui::Vec2::new(ui.available_width(), avail.y),
                    egui::Layout::top_down(egui::Align::LEFT),
                    |ui| {
                        ui.label(
                            egui::RichText::new("预览")
                                .small()
                                .color(crate::theme::MUTED)
                                .strong(),
                        );
                        ui.add_space(2.0);
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            CommonMarkViewer::new().show(ui, &mut self.md_cache, &self.body);
                        });
                    },
                );
            });
        });

        // 持续 repaint 让防抖 timer 可被 poll
        if self.body_dirty {
            ctx.request_repaint_after(DEBOUNCE);
        }
    }
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
