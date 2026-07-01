//! octopus-egui：记事本原生进程（eframe）。单进程 + view 路由。
//! 第一阶段仅 NotepadView。Tauri 主进程经本地 TCP IPC spawn 并驱动。

mod ipc;
mod notepad_view;

use ipc::IpcMsg;
use notepad_view::NotepadView;
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
    view: NotepadView,
}

impl NotepadApp {
    fn new(rx: mpsc::Receiver<IpcMsg>) -> Self {
        Self { rx, view: NotepadView::default() }
    }
}

impl eframe::App for NotepadApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // 排空 IPC 消息（非阻塞），分发到 view
        while let Ok(msg) = self.rx.try_recv() {
            self.view.handle_ipc(msg);
        }
        self.view.show(ctx);
    }
}
