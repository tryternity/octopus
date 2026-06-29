# 窗口智能探测截图实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans.

**Goal:** 截图 idle 时自动探测鼠标所在窗口并高亮，点击直接用窗口矩形做选区 + 激活失焦应用

**Architecture:** capx 新增窗口探测 + 跨平台激活；前端 idle mousemove 防抖调后端；Canvas 绘制高亮 + 点击判定

**Spec:** `docs/superpowers/specs/2026-06-29-window-detection-design.md`

---

## 文件结构

| 文件 | 变更 | 责任 |
|---|---|---|
| `crates/capx/src/window.rs` | Create | 窗口枚举 + 探测 + 激活（跨平台 cfg） |
| `crates/capx/src/lib.rs` | Modify | pub mod window |
| `crates/capx/Cargo.toml` | Modify | macOS: objc2 + objc2-foundation + objc2-app-kit |
| `crates/desktop/src/screenshot_commands.rs` | Modify | 新增 get_window_at_point + activate_window_cmd 命令 |
| `crates/desktop/src/main.rs` | Modify | 注册命令 |
| `crates/desktop/frontend/src/pages/Screenshot/index.tsx` | Modify | 窗口高亮 + 点击判定 |

---

### Task 1: capx window.rs（探测 + 激活）

- [ ] **Step 1: 创建 window.rs**

```rust
use anyhow::{Context, Result};
use serde::Serialize;
use xcap::Window;

#[derive(Debug, Clone, Serialize)]
pub struct WindowRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub pid: u32,
    pub id: u32,
    pub app_name: String,
}

/// 找到包含逻辑坐标 (x, y) 的最顶层应用窗口。
pub fn window_at_point(x: f64, y: f64, scale_factor: f64) -> Result<Option<WindowRect>> {
    let windows = Window::all().context("Failed to list windows")?;
    let phys_x = x * scale_factor;
    let phys_y = y * scale_factor;

    // 过滤 + 按 z 降序
    let mut filtered: Vec<_> = windows.into_iter()
        .filter(|w| {
            let title = w.title().unwrap_or_default();
            let name = w.app_name().unwrap_or_default();
            // 排除无标题、桌面、Dock、截图窗口、极小窗口
            let w_val = w.width().unwrap_or(0);
            let h_val = w.height().unwrap_or(0);
            if w_val < 50 || h_val < 50 { return false; }
            if w.is_minimized().unwrap_or(false) { return false; }
            let name_lower = name.to_lowercase();
            let title_lower = title.to_lowercase();
            if title.is_empty() && name.is_empty() { return false; }
            if name_lower.contains("dock") || title_lower.contains("dock") { return false; }
            if name_lower == "finder" && (title_lower == "desktop" || title.is_empty()) { return false; }
            if name_lower.contains("octopus") || name_lower.contains("screenshot") { return false; }
            true
        })
        .collect();

    filtered.sort_by(|a, b| {
        b.z().unwrap_or(0).cmp(&a.z().unwrap_or(0))
    });

    for w in filtered {
        let wx = w.x().unwrap_or(0) as f64;
        let wy = w.y().unwrap_or(0) as f64;
        let ww = w.width().unwrap_or(0) as f64;
        let wh = w.height().unwrap_or(0) as f64;

        if phys_x >= wx && phys_x <= wx + ww && phys_y >= wy && phys_y <= wy + wh {
            return Ok(Some(WindowRect {
                x: wx / scale_factor,
                y: wy / scale_factor,
                w: ww / scale_factor,
                h: wh / scale_factor,
                pid: w.pid().unwrap_or(0),
                id: w.id().unwrap_or(0),
                app_name: w.app_name().unwrap_or_default(),
            }));
        }
    }

    Ok(None)
}

/// 激活窗口到最前面（跨平台）。
pub fn activate_window(pid: u32, window_id: u32) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        use objc2::AnyThread;
        use objc2_app_kit::{NSApplication, NSRunningApplication, NSApplicationActivateOptions};
        use objc2_foundation::NSNumber;
        unsafe {
            let apps = NSRunningApplication::runningApplicationsWithProcessIdentifier(pid as i32);
            if let Some(app) = apps.first() {
                app.activateWithOptions(NSApplicationActivateOptions::NSApplicationActivateIgnoringOtherApps);
                log::info!("Activated app pid={} name={}", pid, app.localizedName().map(|s| s.to_string()).unwrap_or_default());
            }
        }
    }
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::SetForegroundWindow;
        unsafe {
            let hwnd = HWND(window_id as isize);
            let _ = SetForegroundWindow(hwnd);
        }
    }
    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("xdotool")
            .args(["windowactivate", &window_id.to_string()])
            .spawn();
    }
    Ok(())
}
```

- [ ] **Step 2: lib.rs 加 pub mod window**
- [ ] **Step 3: Cargo.toml 加 macOS 平台依赖**
- [ ] **Step 4: 验证编译**

---

### Task 2: Tauri 命令注册

- [ ] **Step 1: screenshot_commands.rs 新增命令**

```rust
#[tauri::command]
pub fn get_window_at_point(x: f64, y: f64) -> Result<Option<serde_json::Value>, String> {
    let scale = 2.0; // TODO: 从前端传或后端获取
    let rect = octopus_capx::window::window_at_point(x, y, scale)
        .map_err(|e| e.to_string())?;
    Ok(rect.map(|r| serde_json::to_value(r).unwrap_or_default()))
}

#[tauri::command]
pub fn activate_window_cmd(pid: u32, window_id: u32) -> Result<(), String> {
    octopus_capx::window::activate_window(pid, window_id)
        .map_err(|e| e.to_string())
}
```

- [ ] **Step 2: main.rs 注册**
- [ ] **Step 3: 验证编译**

---

### Task 3: 前端窗口高亮 + 点击判定

- [ ] **Step 1: 状态 + 防抖**

新增 state: `hoverWindow: WindowRect | null`
新增 ref: `mousedownPt`, `debounceTimer`

- [ ] **Step 2: draw 中绘制高亮矩形**

idle + hoverWindow 存在时：白色描边 2px + 半透明白色填充 0.1

- [ ] **Step 3: mousemove 防抖探测**

idle + tool=none + 无 textDraft 时，200ms 防抖调 `get_window_at_point`

- [ ] **Step 4: mousedown/mouseup 点击判定**

mousedown 记录坐标 → mouseup 时移动 < 5px + 有 hoverWindow → 用窗口矩形做 sel + activate_window_cmd

- [ ] **Step 5: 构建验证**

---

### Task 4: 端到端验证

- [ ] 快捷键截图 → 鼠标移到应用窗口 → 高亮
- [ ] 点击高亮 → 选区 = 窗口矩形 + 应用激活
- [ ] 拖拽 → 手动框选（高亮消失）
- [ ] 桌面空白处 → 无高亮
