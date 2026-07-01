//! octopus-egui：记事本原生进程（eframe）。单进程 + view 路由。
//! 第一阶段仅 NotepadView。Tauri 主进程经本地 TCP IPC spawn 并驱动。

mod ipc;
mod macos;
mod notepad_view;
mod theme;

use ipc::IpcMsg;
use notepad_view::NotepadView;
use std::sync::mpsc;

fn main() -> eframe::Result {
    // 日志（RUST_LOG 控制，默认 info）。stderr 继承到 desktop 终端，便于诊断 egui 进程。
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .try_init();

    // 单例锁：已有活实例 → 直接退出（desktop 端会连上已有实例，不重复 spawn）。
    if !ipc::acquire_singleton() {
        log::info!("已有 octopus-egui 实例运行，本进程退出");
        return Ok(());
    }

    // IPC 接收通道：后台 server 线程收 TCP 消息 → 推给主线程
    let (tx, rx) = mpsc::channel::<IpcMsg>();
    ipc::start(tx);

    // macOS Accessory（无 Dock 图标）。run_native 前设一次。
    macos::set_accessory_policy();

    // 显式窗口尺寸 + 最小尺寸：default() 给的初始窗口偏小/不确定，叠加 SidePanel 260 默认宽
    // 会让 CentralPanel 余量为 0（编辑区整个消失）。固定后中央永远有显示空间。
    let viewport = egui::ViewportBuilder::default()
        .with_inner_size([1024.0, 720.0])
        .with_min_inner_size([760.0, 480.0]);
    let opts = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native(
        "octopus 记事本",
        opts,
        Box::new(move |cc| Ok(Box::new(NotepadApp::new(cc, rx)))),
    )
}

struct NotepadApp {
    rx: mpsc::Receiver<IpcMsg>,
    view: NotepadView,
}

impl NotepadApp {
    fn new(cc: &eframe::CreationContext<'_>, rx: mpsc::Receiver<IpcMsg>) -> Self {
        // 主题：深色 + indigo 强调色 + spacing（快速美化）。
        theme::setup(&cc.egui_ctx);
        // CJK 字体（egui 默认无中文，不加载则中文显方块）。
        setup_fonts(&cc.egui_ctx);
        // Accessory 再设一次（事件循环已起，确保 Dock 图标隐藏生效）。
        macos::set_accessory_policy();
        Self { rx, view: NotepadView::default() }
    }
}

/// 加载系统 CJK 字体注入 egui。失败保留默认（中文显方块但能跑）。
fn setup_fonts(ctx: &egui::Context) {
    // 候选：单 ttf 优先（ab_glyph 必支持），ttc 次之（PingFang 漂亮但需 collection 支持）。
    let candidates: &[&str] = &[
        "/System/Library/Fonts/Supplemental/Arial Unicode.ttf", // macOS 单 ttf（含全 CJK）
        "/System/Library/Fonts/PingFang.ttc",                     // macOS ttc
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc", // Linux
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
        "C:\\Windows\\Fonts\\msyh.ttc",                           // Windows 微软雅黑
        "C:\\Windows\\Fonts\\simsun.ttc",
    ];
    for path in candidates {
        if let Ok(bytes) = std::fs::read(path) {
            let mut fonts = egui::FontDefinitions::default();
            fonts.font_data.insert(
                "cjk".to_owned(),
                egui::FontData::from_owned(bytes),
            );
            // Proportional / Monospace 都把 cjk 加到末尾作 fallback（拉丁优先，缺字回退 cjk）
            for fam in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                if let Some(list) = fonts.families.get_mut(&fam) {
                    list.push("cjk".to_owned());
                }
            }
            ctx.set_fonts(fonts);
            log::info!("已加载 CJK 字体: {}", path);
            return;
        }
    }
    log::warn!("未找到 CJK 字体，中文将显示为方块");
}

impl eframe::App for NotepadApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // 排空 IPC 消息（非阻塞）。Show 在此直接唤起窗口（viewport 命令），
        // 其余分发到 view。egui 关窗后窗口常被隐藏而非进程退出，Show 必须显式唤起。
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                IpcMsg::Show => {
                    log::info!("IPC Show: 唤起窗口");
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                other => self.view.handle_ipc(other),
            }
        }
        self.view.show(ctx);
    }
}

impl Drop for NotepadApp {
    fn drop(&mut self) {
        // 退出清理：删 singleton 锁 + port 文件，避免残留让 desktop 误判实例还在
        ipc::cleanup();
    }
}
