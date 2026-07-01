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
        // 强制 dark：eframe 默认 theme_preference=System，macOS 浅色模式会用 light visuals
        //（panel_fill=248 近白）覆盖 set_visuals，且 clear_color 读 light panel_fill → 白底。
        // 显式锁 Dark，自定义深色主题才稳定生效（不必每帧重设）。
        cc.egui_ctx.options_mut(|o| o.theme_preference = egui::ThemePreference::Dark);
        // 主题：深色 + indigo 强调色 + spacing（快速美化）。
        theme::setup(&cc.egui_ctx);
        // CJK 字体（egui 默认无中文，不加载则中文显方块）。
        setup_fonts(&cc.egui_ctx);
        // Accessory 再设一次（事件循环已起，确保 Dock 图标隐藏生效）。
        macos::set_accessory_policy();
        // 强制首帧后再画一帧：eframe 首帧窗口 size 可能未定（macOS 窗口刚创建），
        // CentralPanel 内容若按 0 尺寸布局会画空，且无交互不重绘 → 默认空白。
        cc.egui_ctx.request_repaint();
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
                std::sync::Arc::new(egui::FontData::from_owned(bytes)),
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
    /// 清屏色：与 theme panel_fill 一致的深色（硬编码）。
    /// 默认 epi::clear_color 是 (12,12,12,180) 半透明黑，在 macOS 上显灰；
    /// 而 clear_color 在 update 前用上一帧 visuals，读 visuals.panel_fill 首帧可能尚未应用 theme。
    /// 硬编码深色保证首帧即深，且与面板同色，掩盖 SidePanel↔CentralPanel 的 sub-pixel 间隙。
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        egui::Color32::from_rgb(24, 24, 27).to_normalized_gamma_f32()
    }

    /// 逻辑层（每帧 ui 前调用，不可 paint）：排空 IPC、唤起窗口、分发消息。
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Show 在此唤起窗口（viewport 命令），其余分发到 view。
        // egui 关窗后窗口常被隐藏而非进程退出，Show 必须显式唤起。
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
    }

    /// UI 层（每帧重绘）。eframe 0.34 起 App 主入口为 ui（非 update）。
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.view.show(ui);
    }
}

impl Drop for NotepadApp {
    fn drop(&mut self) {
        // 退出清理：删 singleton 锁 + port 文件，避免残留让 desktop 误判实例还在
        ipc::cleanup();
    }
}

#[cfg(test)]
mod tests {
    /// 回归保护：侧栏 Panel::left().exact_size(260).resizable(false) 在 show_inside（根 ui 内）
    /// 必须精确产出 260 宽、且与 CentralPanel 无缝（gap≈0）。
    /// 背景：曾怀疑 egui 0.34 的 exact_size 不生效（线上诊断出 324.28），后用 __run_test_ctx
    /// 确认是旧二进制——egui 行为正确。本测试固化该结论，防回归。
    #[test]
    #[allow(deprecated)]
    fn left_panel_exact_size_is_honored() {
        egui::__run_test_ctx(|ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let side = egui::Panel::left("list")
                    .resizable(false)
                    .exact_size(260.0)
                    .show_separator_line(false)
                    .show_inside(ui, |_ui| {});
                let central = egui::CentralPanel::default().show_inside(ui, |_ui| {});
                let gap = central.response.rect.min.x - side.response.rect.max.x;
                assert!(
                    (side.response.rect.width() - 260.0).abs() < 1.0,
                    "侧栏宽度 = {}, 期望 260",
                    side.response.rect.width()
                );
                assert!(gap.abs() < 2.0, "panel 间隙 = {gap}, 期望 ≈0 无缝");
            });
        });
    }
}
