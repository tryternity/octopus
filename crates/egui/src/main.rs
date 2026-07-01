//! octopus-egui：记事本原生进程（eframe）。单进程 + view 路由。
//! 第一阶段仅 NotepadView。Tauri 主进程经本地 TCP IPC spawn 并驱动。
//!
//! Task 4 骨架：空 eframe 窗口，验证依赖版本可编。NotepadView/IPC 见 Task 5/6。

fn main() -> eframe::Result {
    let opts = eframe::NativeOptions::default();
    eframe::run_native(
        "octopus 记事本",
        opts,
        Box::new(|_cc| Ok(Box::new(NotepadApp::default()))),
    )
}

#[derive(Default)]
struct NotepadApp;

impl eframe::App for NotepadApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("octopus 记事本（骨架）");
        });
    }
}
