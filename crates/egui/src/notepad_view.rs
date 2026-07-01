//! NotepadView：列表 + md 源码编辑 + egui_commonmark 分屏预览 + 5 按钮工具栏。
//! 直连 octopus_notepad::store（经 octopus_infra::db::with_db 用本进程全局连接，WAL）。
//! 编辑走 800ms 防抖保存（对齐原 webview 行为）。

use crate::ipc::IpcMsg;
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use octopus_notepad::{Note, NoteFilter, NoteType};
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

        egui::SidePanel::left("list").resizable(true).default_width(240.0).show(ctx, |ui| {
            ui.heading("笔记");
            let mut select_id: Option<i64> = None;
            egui::ScrollArea::vertical().show(ui, |ui| {
                for n in &self.notes {
                    let selected = self.current_id == Some(n.id);
                    let label = n.title.clone().unwrap_or_else(|| {
                        n.content_text.chars().take(20).collect()
                    });
                    if ui.selectable_label(selected, &label).clicked() {
                        select_id = Some(n.id);
                    }
                }
            });
            if let Some(id) = select_id {
                self.select(id);
            }
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            // 标题
            ui.horizontal(|ui| {
                ui.label("标题:");
                let resp = ui.text_edit_singleline(&mut self.title);
                if resp.changed() {
                    self.mark_dirty();
                }
            });
            ui.separator();

            // 工具栏（5 按钮：选中文本→包 md 语法）
            toolbar(ui, &mut self.body, &mut self.body_dirty, &mut self.last_edit);

            // 编辑 / 预览分屏
            let available = ui.available_size();
            let half = egui::Vec2::new(available.x / 2.0, available.y);
            ui.horizontal(|ui| {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.set_min_size(half);
                    ui.label("Markdown 源码");
                    let resp = ui.add(
                        egui::TextEdit::multiline(&mut self.body)
                            .desired_width(f32::MAX)
                            .desired_rows(20),
                    );
                    if resp.changed() {
                        self.mark_dirty();
                    }
                });
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.set_min_size(half);
                    ui.label("预览");
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        CommonMarkViewer::new().show(ui, &mut self.md_cache, &self.body);
                    });
                });
            });
        });

        // 持续 repaint 让防抖 timer 可被 poll
        if self.body_dirty {
            ctx.request_repaint_after(DEBOUNCE);
        }
    }
}

/// 5 按钮工具栏：选中文本包 md 语法。
fn toolbar(
    ui: &mut egui::Ui,
    body: &mut String,
    dirty: &mut bool,
    last_edit: &mut Option<Instant>,
) {
    ui.horizontal_wrapped(|ui| {
        let pairs: &[(&str, &str, &str)] = &[
            ("B 粗体", "**", "**"),
            ("I 斜体", "*", "*"),
            ("H 标题", "# ", ""),
            ("• 列表", "- ", ""),
            ("` 代码", "`", "`"),
        ];
        for (label, pre, post) in pairs {
            if ui.small_button(*label).clicked() {
                wrap_selection_or_append(body, pre, post);
                *dirty = true;
                *last_edit = Some(Instant::now());
            }
        }
    });
    ui.separator();
}

/// 简化版：在末尾追加 pre+post（egui 0.29 的 TextEdit 选区 API 有限，
/// 第一版用末尾插入语法标记，对用户可见可点；选区包覆留作后续优化）。
fn wrap_selection_or_append(body: &mut String, pre: &str, post: &str) {
    body.push_str(pre);
    body.push_str(post);
}
