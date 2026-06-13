// src/result_window.rs

use log::{debug, info};
use std::path::PathBuf;
use tauri::{Emitter, Listener, Manager};

const RESULT_WIDTH: f64 = 520.0;
const RESULT_HEIGHT: f64 = 100.0;
const WINDOW_LABEL: &str = "result_window";

/// history.txt 最多保留的历史记录条数
const MAX_HISTORY_ENTRIES: usize = 20;

// ── 文件路径 ──

fn record_file_path() -> PathBuf {
    octopus_asr::config::handy_home().join("record.txt")
}

fn history_file_path() -> PathBuf {
    octopus_asr::config::handy_home().join("history.txt")
}

// ── record.txt 操作 ──

/// 将当前展示文本写入 ~/.octopus/record.txt
pub fn save_record(text: &str) {
    if text.is_empty() {
        return;
    }
    let path = record_file_path();
    if let Err(e) = std::fs::write(&path, text) {
        debug!("Failed to save record.txt: {}", e);
    }
}

/// 清空 record.txt（归档到 history 后调用）
fn clear_record_file() {
    let path = record_file_path();
    let _ = std::fs::remove_file(&path);
}

// ── history.txt 操作 ──

/// 将当前 record.txt 内容归档到 history.txt，保留最多 MAX_HISTORY_ENTRIES 条
pub fn archive_to_history() {
    let record_path = record_file_path();
    let history_path = history_file_path();

    // 读取当前 record.txt
    let content = match std::fs::read_to_string(&record_path) {
        Ok(c) if !c.trim().is_empty() => c.trim().to_string(),
        _ => return, // 无内容，跳过
    };

    // 构建新条目
    let timestamp = chrono_now_string();
    let new_entry = format!("--- {} ---\n{}\n", timestamp, content);

    // 读取已有 history
    let existing = if history_path.exists() {
        std::fs::read_to_string(&history_path).unwrap_or_default()
    } else {
        String::new()
    };

    // 解析已有条目
    let mut entries: Vec<String> = parse_history_entries(&existing);

    // 追加新条目
    entries.push(new_entry);

    // 保留最近 MAX_HISTORY_ENTRIES 条
    if entries.len() > MAX_HISTORY_ENTRIES {
        let drain_count = entries.len() - MAX_HISTORY_ENTRIES;
        entries.drain(0..drain_count);
    }

    // 写回 history.txt
    let history_content = entries.join("\n");
    if let Err(e) = std::fs::write(&history_path, history_content) {
        debug!("Failed to write history.txt: {}", e);
    } else {
        info!("Archived to history.txt ({} entries)", entries.len());
    }

    // 清空 record.txt
    clear_record_file();
}

/// 解析 history.txt 中的条目（以 `--- ` 开头的行作为分隔符）
fn parse_history_entries(content: &str) -> Vec<String> {
    let mut entries: Vec<String> = Vec::new();
    let mut current = String::new();

    for line in content.lines() {
        if line.starts_with("--- ") && line.ends_with(" ---") {
            // 遇到新条目分隔符
            if !current.trim().is_empty() {
                entries.push(current.trim_end().to_string());
            }
            current = line.to_string();
            current.push('\n');
        } else {
            current.push_str(line);
            current.push('\n');
        }
    }
    if !current.trim().is_empty() {
        entries.push(current.trim_end().to_string());
    }

    entries
}

/// 获取当前时间的格式化字符串（不依赖 chrono，使用 std）
fn chrono_now_string() -> String {
    // 使用 SystemTime → 大致格式化为 YYYY-MM-DD HH:MM:SS
    let duration = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = duration.as_secs();
    // 简单计算日期时间（无需 chrono 依赖）
    let days = secs / 86400;
    let time_of_day = secs % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    // 从 1970-01-01 开始计算年月日
    let (year, month, day) = days_to_ymd(days);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        year, month, day, hours, minutes, seconds
    )
}

/// Unix epoch 天数 → (年, 月, 日)
fn days_to_ymd(mut days: u64) -> (u64, u64, u64) {
    let mut year = 1970u64;
    loop {
        let days_in_year = if is_leap(year) { 366 } else { 365 };
        if days < days_in_year {
            break;
        }
        days -= days_in_year;
        year += 1;
    }
    let leap = is_leap(year);
    let month_days: [u64; 12] = if leap {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    let mut month = 0u64;
    for (i, &md) in month_days.iter().enumerate() {
        if days < md {
            month = i as u64 + 1;
            break;
        }
        days -= md;
    }
    if month == 0 {
        month = 12;
    }
    (year, month, days + 1)
}

fn is_leap(year: u64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

// ── 窗口管理 ──

/// 创建结果展示窗口（默认隐藏）。
pub fn create_result_window(app: &tauri::AppHandle) {
    if app.get_webview_window(WINDOW_LABEL).is_some() {
        return;
    }

    let builder = tauri::WebviewWindowBuilder::new(
        app,
        WINDOW_LABEL,
        tauri::WebviewUrl::App("result/index.html".into()),
    )
    .title("Result")
    .inner_size(RESULT_WIDTH, RESULT_HEIGHT)
    .resizable(false)
    .decorations(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .transparent(true)
    .focused(false)
    .visible(false)
    .shadow(false);

    match builder.build() {
        Ok(window) => {
            // 首次创建时定位到屏幕顶部居中
            if let Ok(monitor) = window.primary_monitor() {
                if let Some(m) = monitor {
                    let x = (m.size().width as f64 / m.scale_factor() - RESULT_WIDTH) / 2.0;
                    let y = 80.0;
                    let _ = window.set_position(tauri::Position::Logical(
                        tauri::LogicalPosition::new(x, y),
                    ));
                }
            }

            // 监听来自 JS 的编辑同步事件
            let app_handle = app.clone();
            let _ = window.listen("result-edited", move |event| {
                let text = event.payload();
                if !text.is_empty() {
                    save_record(text);
                }
            });

            debug!("Result window created");
        }
        Err(e) => debug!("Failed to create result window: {}", e),
    }
}

/// 显示结果窗口并展示识别文本。
pub fn show_result(app: &tauri::AppHandle, text: &str) {
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        let _ = window.emit("show-result", text);
        let _ = window.show();
    }
}

/// 更新结果窗口文本（流式更新时使用）。
pub fn update_result(app: &tauri::AppHandle, text: &str) {
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        let _ = window.emit("update-result", text);
    }
}

/// 清空结果窗口内容并隐藏（粘贴完成后调用）。
/// 同时将当前 record.txt 归档到 history.txt。
pub fn clear_result(app: &tauri::AppHandle) {
    // 先归档到 history
    archive_to_history();

    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        let _ = window.emit("clear-result", ());
        let window_clone = window.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(200));
            let _ = window_clone.hide();
        });
    }
}

/// 隐藏结果窗口（不清空内容，不归档）。
pub fn hide_result(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        let _ = window.emit("hide-result", ());
        let window_clone = window.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(200));
            let _ = window_clone.hide();
        });
    }
}
