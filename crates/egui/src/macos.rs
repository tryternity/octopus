//! macOS 集成：egui 进程设 Accessory 激活策略（无 Dock 图标），窗口仍可 show/focus。
//! 兜底：若此处失败，egui 进程默认 Regular（2 个 Dock 图标，功能不阻断）。

#[cfg(target_os = "macos")]
pub fn set_accessory_policy() {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};

    let Some(mt) = MainThreadMarker::new() else {
        log::warn!("非主线程，跳过 Accessory 设置");
        return;
    };
    let app = NSApplication::sharedApplication(mt);
    let ok = app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    eprintln!("[octopus-egui] setActivationPolicy(Accessory) 返回 {}", ok);
    if !ok {
        log::warn!("setActivationPolicy(Accessory) 返回 false");
    }
    // activateIgnoringOtherApps 新版标 deprecated，但仍是拉起进程获焦的最简路径；
    // NSApp.activate 替代签名更繁，此处保留 + allow。
    #[allow(deprecated)]
    app.activateIgnoringOtherApps(true);
    log::info!("egui 进程已设 Accessory（无 Dock 图标）");
}

#[cfg(not(target_os = "macos"))]
pub fn set_accessory_policy() {}
