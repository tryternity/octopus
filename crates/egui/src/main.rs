//! octopus-egui：记事本原生进程（eframe）。单进程 + view 路由。
//! 第一阶段仅 NotepadView。Tauri 主进程经本地 TCP IPC spawn 并驱动。

mod ipc;

use ipc::IpcMsg;
use std::sync::mpsc;

fn main() -> eframe::Result {
    // IPC 接收通道：后台 server 线程收 TCP 消息 → 推给主线程
    let (tx, rx) = mpsc::channel::<IpcMsg>();
    ipc::start(tx);

    let opts = eframe::NativeOptions::default();
    eframe::run_native(
        "octopus 记事本",
        opts,
        Box::new(move |_cc| Ok(Box::new(NotepadApp::new(rx)))),
    )
}

struct NotepadApp {
    rx: mpsc::Receiver<IpcMsg>,
}

impl NotepadApp {
    fn new(rx: mpsc::Receiver<IpcMsg>) -> Self {
        Self { rx }
    }
}

impl eframe::App for NotepadApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // 排空 IPC 消息（非阻塞）
        while let Ok(msg) = self.rx.try_recv() {
            log::info!("UI 处理消息: {:?}", msg);
        }
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("octopus 记事本（骨架 + IPC）");
        });
        // 持续 request_repaint 让 IPC 消息及时被 poll（200ms）
        ctx.request_repaint_after(std::time::Duration::from_millis(200));
    }
}
