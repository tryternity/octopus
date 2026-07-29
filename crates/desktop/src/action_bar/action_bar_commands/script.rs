//! 脚本执行 + 统一执行入口（从 action_bar_commands/mod.rs 提取，Task 1.6）。
//!
//! 脚本运行时探测（node/bun/deno/...）+ spawn/等待（同步阻塞 / 异步后台）+ 落库；
//! `execute_action_bar_inner` 是 ai/url/script/agent/copy_path 五种动作类型的统一执行核心，
//! `execute_action_bar` 是对外 `#[tauri::command]` 包装。

use std::sync::OnceLock;
use tauri::{AppHandle, Manager};
use crate::error_util::{e2s, e2s_ctx};
use crate::action_bar::action_bar_window::hide_action_bar_window;
// 父模块共享状态 + 共享类型 + 各兄弟子模块经 glob re-export 暴露的 helper。
use super::{
    PENDING_CONTEXT,
    write_clipboard_text,
    action_bar_show_result, build_enriched_text,
    finalize_action_bar,
    resolve_prompt_reference, render_agent_prompt, derive_cwd, format_paths,
    resolve_translate_strategy, TranslateStrategy, TranslateEmitTarget,
    do_translate_streaming, auto_translate_prompt, url_encode_param,
};

struct ScriptResult {
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
    timed_out: bool,
}

/// 从 ScriptResult 生成 error_msg——超时/异常退出/非零退出码都有描述
fn script_error_msg(result: &ScriptResult) -> String {
    if result.timed_out {
        "执行超时（60秒）".to_string()
    } else if result.exit_code.is_none() {
        "进程异常退出".to_string()
    } else if result.exit_code != Some(0) {
        format!("进程以错误码 {} 退出", result.exit_code.unwrap())
    } else {
        String::new()
    }
}

/// 探测 JS 运行时——优先级 node → bun → deno（结果缓存，仅首次探测）
fn detect_js_runtime() -> Option<(&'static str, &'static str)> {
    static CACHE: OnceLock<Option<(&'static str, &'static str)>> = OnceLock::new();
    *CACHE.get_or_init(|| {
        for (bin, flag) in [("node", "-e"), ("bun", "eval"), ("deno", "eval")] {
            if std::process::Command::new(bin).arg("--version")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status().is_ok()
            {
                return Some((bin, flag));
            }
        }
        None
    })
}

/// 探测 TS 运行时——优先级 bun → deno → npx tsx（结果缓存，仅首次探测）
fn detect_ts_runtime() -> Option<(&'static str, Vec<&'static str>)> {
    static CACHE: OnceLock<Option<(&'static str, Vec<&'static str>)>> = OnceLock::new();
    CACHE.get_or_init(|| {
        // bun/deno 原生支持 TS，探测仅本地进程，毫秒级
        if std::process::Command::new("bun").arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status().is_ok()
        {
            return Some(("bun", vec!["eval"]));
        }
        if std::process::Command::new("deno").arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status().is_ok()
        {
            return Some(("deno", vec!["eval"]));
        }
        // npx tsx 作为 fallback（可能触发联网下载，最慢）
        if std::process::Command::new("npx").args(["--yes", "tsx", "--version"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status().is_ok()
        {
            return Some(("npx", vec!["--yes", "tsx", "-e"]));
        }
        None
    }).clone()
}

/// 按 magic comment 分发运行时，spawn 子进程。
/// capture_output=true 时 stdout/stderr 用 pipe（同步模式），false 时用 null（异步模式）。
fn spawn_script(source: &str, text: &str, capture_output: bool, pkg_dir: &Option<String>) -> Result<(std::process::Child, String, Option<std::path::PathBuf>), String> {
    use std::process::Stdio;
    let first_line = source.lines().next().unwrap_or("").trim();
    let script: String = source.lines().skip(1).collect::<Vec<_>>().join("\n");

    let stdout_cfg = if capture_output { Stdio::piped() } else { Stdio::null() };
    let stderr_cfg = if capture_output { Stdio::piped() } else { Stdio::null() };

    let cmd_result: Result<std::process::Command, String> = match first_line {
        "#shell" => {
            let mut c = std::process::Command::new("sh");
            c.arg("-c").arg(&script);
            Ok(c)
        }
        "#osascript" => {
            #[cfg(target_os = "macos")]
            { let mut c = std::process::Command::new("osascript"); c.arg("-e").arg(&script); Ok(c) }
            #[cfg(not(target_os = "macos"))]
            { Err("osascript 仅 macOS 支持".into()) }
        }
        "#powershell" => {
            #[cfg(target_os = "windows")]
            { let mut c = std::process::Command::new("powershell"); c.arg("-Command").arg(&script); Ok(c) }
            #[cfg(not(target_os = "windows"))]
            { Err("powershell 仅 Windows 支持".into()) }
        }
        "#python" => {
            #[cfg(target_os = "windows")]
            let py = "python";
            #[cfg(not(target_os = "windows"))]
            let py = "python3";
            let mut c = std::process::Command::new(py);
            c.arg("-c").arg(&script);
            Ok(c)
        }
        "#node" => {
            let mut c = std::process::Command::new("node");
            c.arg("-e").arg(&script);
            Ok(c)
        }
        "#deno" => {
            let mut c = std::process::Command::new("deno");
            c.arg("eval").arg(&script);
            Ok(c)
        }
        "#bun" => {
            let mut c = std::process::Command::new("bun");
            c.arg("eval").arg(&script);
            Ok(c)
        }
        "#javascript" => {
            let (bin, flag) = detect_js_runtime()
                .ok_or_else(|| "未检测到 JS 运行时，请安装 Node.js / Bun / Deno 之一".to_string())?;
            let mut c = std::process::Command::new(bin);
            c.arg(flag).arg(&script);
            Ok(c)
        }
        "#typescript" => {
            let (bin, args) = detect_ts_runtime()
                .ok_or_else(|| "未检测到 TS 运行时，请安装 tsx（npm i -g tsx）/ Bun / Deno 之一".to_string())?;
            let mut c = std::process::Command::new(bin);
            for a in &args { c.arg(a); }
            c.arg(&script);
            Ok(c)
        }
        _ => return Err(format!(
            "未知脚本类型: {}（第一行须为 #shell/#osascript/#powershell/#python/#node/#deno/#bun/#javascript/#typescript）",
            first_line
        )),
    };

    let mut cmd = cmd_result?;
    // 选中文本传递：≤200KB 用环境变量 OCTOPUS_TEXT；超出写临时文件，
    // OCTOPUS_TEXT 设为 "_____ULTRA_LONG_TEXT_____:/tmp/octopus-text-xxx" 供消费方读取
    let mut text_tmp: Option<std::path::PathBuf> = None;
    const TEXT_LIMIT: usize = 200_000;
    if text.len() > TEXT_LIMIT {
        let tmp_path = std::env::temp_dir().join(format!(
            "octopus-text-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
        ));
        if let Err(e) = {
            #[cfg(unix)]
            {
                use std::io::Write;
                use std::os::unix::fs::OpenOptionsExt;
                std::fs::OpenOptions::new()
                    .write(true).create(true).truncate(true)
                    .mode(0o600)  // 仅 owner 可读——选中文本可能含敏感信息，防其他本地用户读取
                    .open(&tmp_path)
                    .and_then(|mut f| f.write_all(text.as_bytes()))
            }
            #[cfg(not(unix))]
            { std::fs::write(&tmp_path, text) }
        } {
            log::warn!("[script] 临时文件写入失败，回退截断: {}", e);
            // 按字节截断（非字符），确保 UTF-8 边界安全 + 严格 < TEXT_LIMIT 字节
            let mut end = TEXT_LIMIT;
            while !text.is_char_boundary(end) { end -= 1; }
            cmd.env("OCTOPUS_TEXT", &text[..end]);
        } else {
            let marker = format!(
                "_____ULTRA_LONG_TEXT_____:{}",
                tmp_path.to_string_lossy()
            );
            cmd.env("OCTOPUS_TEXT", &marker);
            text_tmp = Some(tmp_path);
        }
    } else {
        cmd.env("OCTOPUS_TEXT", text);
    }
    if let Some(dir) = pkg_dir {
        cmd.env("OCTOPUS_PACKAGE_DIR", dir);
    }
    cmd.stdout(stdout_cfg);
    cmd.stderr(stderr_cfg);
    let child = cmd.spawn().map_err(|e| {
        // spawn 失败——清理临时文件防泄露
        if let Some(ref p) = text_tmp { let _ = std::fs::remove_file(p); }
        format!("脚本执行失败: {}", e)
    })?;
    Ok((child, first_line.to_string(), text_tmp))
}

/// 轮询等待子进程退出，60 秒超时强杀。并发读取 stdout/stderr 防管道死锁。
fn wait_with_timeout(child: std::process::Child) -> ScriptResult {
    wait_with_timeout_secs(child, 60)
}

/// 异步脚本等待——不超时，阻塞等待自然退出（0 CPU 占用）。
fn wait_forever(mut child: std::process::Child) -> ScriptResult {
    use std::io::Read;

    let mut stdout_handle = child.stdout.take();
    let mut stderr_handle = child.stderr.take();

    let stdout_thread = std::thread::spawn(move || {
        let mut buf = String::new();
        if let Some(ref mut stdout) = stdout_handle { let _ = stdout.read_to_string(&mut buf); }
        buf
    });
    let stderr_thread = std::thread::spawn(move || {
        let mut buf = String::new();
        if let Some(ref mut stderr) = stderr_handle { let _ = stderr.read_to_string(&mut buf); }
        buf
    });

    // 阻塞等待子进程退出——无轮询，CPU 0%
    let code = child.wait().ok().and_then(|s| s.code());

    let stdout_buf = stdout_thread.join().unwrap_or_default();
    let stderr_buf = stderr_thread.join().unwrap_or_default();
    ScriptResult { exit_code: code, stdout: stdout_buf, stderr: stderr_buf, timed_out: false }
}

fn wait_with_timeout_secs(mut child: std::process::Child, timeout_secs: u32) -> ScriptResult {
    use std::io::Read;

    let mut stdout_handle = child.stdout.take();
    let mut stderr_handle = child.stderr.take();

    let stdout_thread = std::thread::spawn(move || {
        let mut buf = String::new();
        if let Some(ref mut stdout) = stdout_handle { let _ = stdout.read_to_string(&mut buf); }
        buf
    });
    let stderr_thread = std::thread::spawn(move || {
        let mut buf = String::new();
        if let Some(ref mut stderr) = stderr_handle { let _ = stderr.read_to_string(&mut buf); }
        buf
    });

    let mut timed_out = false;
    let polls = timeout_secs.saturating_mul(2); // 500ms × 2 = 1s
    for _ in 0..polls {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(500)),
            Err(_) => break,
        }
    }
    if child.try_wait().map(|s| s.is_none()).unwrap_or(true) {
        let _ = child.kill();
        let _ = child.wait();
        timed_out = true;
    }

    let stdout_buf = stdout_thread.join().unwrap_or_default();
    let stderr_buf = stderr_thread.join().unwrap_or_default();
    let code = child.wait().ok().and_then(|s| s.code());
    ScriptResult { exit_code: code, stdout: stdout_buf, stderr: stderr_buf, timed_out }
}

/// 当前 Unix epoch 秒数（字符串），落 script_runs.started_at/finished_at——命名反映实际返回值（非 ISO 8601）
fn now_epoch_secs() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    format!("{}", secs)
}

/// 异步执行脚本——spawn 后立即返回，后台线程收割并落库
fn run_script_async(source: &str, text: &str, item_id: i64, pkg_dir: Option<String>) -> Result<(), String> {
    let (child, script_type, text_tmp) = spawn_script(source, text, false, &pkg_dir)?;
    let started = std::time::Instant::now();
    let started_at = now_epoch_secs();
    std::thread::spawn(move || {
        let result = wait_forever(child);
        // 清理超长文本临时文件
        if let Some(ref p) = text_tmp { let _ = std::fs::remove_file(p); }
        let duration_ms = started.elapsed().as_millis() as i64;
        let finished_at = now_epoch_secs();
        let error_msg = script_error_msg(&result);
        let _ = octopus_infra::db::insert_script_run(
            item_id, &script_type, result.exit_code,
            &result.stdout, &result.stderr, &error_msg,
            &started_at, Some(&finished_at), Some(duration_ms),
        );
    });
    Ok(())
}

/// 同步执行脚本（阻塞）——等待完成，返回结果并落库
fn run_script_sync_blocking(source: &str, text: &str, item_id: i64, pkg_dir: Option<String>) -> Result<ScriptResult, String> {
    let (child, script_type, text_tmp) = spawn_script(source, text, true, &pkg_dir)?;
    let started = std::time::Instant::now();
    let started_at = now_epoch_secs();
    let mut result = wait_with_timeout(child);
    // 清理超长文本临时文件
    if let Some(ref p) = text_tmp { let _ = std::fs::remove_file(p); }
    let duration_ms = started.elapsed().as_millis() as i64;
    let finished_at = now_epoch_secs();
    let error_msg = script_error_msg(&result);
    let _ = octopus_infra::db::insert_script_run(
        item_id, &script_type, result.exit_code,
        &result.stdout, &result.stderr, &error_msg,
        &started_at, Some(&finished_at), Some(duration_ms),
    );
    // 标记已落库（ScriptResult 原样返回给上层）
    let _ = &mut result; // 消费 mut borrow
    Ok(result)
}

/// 执行菜单项动作核心逻辑（不含收口）。
/// Ok(true) = ai 已自行收口；Ok(false) = 成功需外层统一收口；Err = 异常需外层 finalize。
/// Quick Execute（全局快捷键）和 ActionBar 路径共用——action_bar_show_result_internal
/// 用 is_visible 检查自动适配 ActionBar 可见/不可见。
pub(crate) async fn execute_action_bar_inner(item_id: i64, text: String, app: &AppHandle) -> Result<bool, String> {
    let item = octopus_infra::db::load_action_bar_item(item_id)
        .map_err(e2s)?
        .ok_or("菜单项不存在")?;

    // 从 PENDING_CONTEXT 取 files（Files 场景）
    let app_state_files: Vec<String> = PENDING_CONTEXT.lock()
        .as_ref().map(|c| c.files.clone()).unwrap_or_default();

    match item.action_type.as_str() {
        "ai" => {
            let config = octopus_infra::config::load_config().map_err(e2s)?;

            // 翻译特殊处理：优先本地引擎。
            // 翻译分支入口只看 action_data，不依赖 ActionBar 可见性——
            // Quick Execute（ActionBar 不可见）也必须正确翻译。
            // action_bar_visible 只 gate Local 流式路径里的 hide/depth 操作。
            if item.action_data == "auto_translate" {
                let action_bar_visible = app.get_webview_window(crate::action_bar::action_bar_window::WINDOW_LABEL)
                    .and_then(|w| w.is_visible().ok())
                    .unwrap_or(false);
                match resolve_translate_strategy(&config).await {
                    TranslateStrategy::LocalModel { .. } | TranslateStrategy::CloudModel { .. } => {
                        // 本地 / 云端模型都走流式翻译（CompactEditor contrast tab，体验更好）。
                        // 隐藏浮窗（仅 ActionBar 可见时）+ 打开 contrast tab。
                        if action_bar_visible {
                            if let Some(win) = app.get_webview_window(crate::action_bar::action_bar_window::WINDOW_LABEL) {
                                let _ = win.hide();
                            }
                            #[cfg(target_os = "macos")]
                            { crate::activation::after_floating_window_hide_keep_active(app); }
                            finalize_action_bar(app);
                        }

                        let original_text = text.clone();
                        // 生成 sessionId——写入 TempTabPayload 让前端 open-tab 时知道这次翻译的 session，
                        // 同时传给 do_translate_streaming 的 CompactEditor target。
                        // 解决发现 1（竞态）：payload 带 sessionId，前端按 sessionId 路由而非依赖
                        // translatingTabKeyRef 时序，spawn emit 早于 open-tab emit 也能正确路由。
                        let session_id = uuid::Uuid::new_v4().to_string();
                        let payload = crate::compact_editor_commands::TempTabPayload {
                            text: "【翻译】\n⏳ 正在翻译…".into(),
                            mode: Some("contrast".into()),
                            original_text: Some(original_text.clone()),
                            translated_text: Some("⏳ 正在翻译…".into()),
                            translate_session_id: Some(session_id.clone()),
                            ..Default::default()
                        };
                        // 投递主线程——create_compact_editor_window 内含 set_dock_icon
                        // 需主线程的 MainThreadMarker，worker 线程直接调会被跳过
                        let app_for_editor = app.clone();
                        let _ = app.run_on_main_thread(move || {
                            crate::compact_editor_commands::open_temp_compact_editor(&app_for_editor, &payload);
                        });

                        let app_clone = app.clone();
                        let target = TranslateEmitTarget::CompactEditor { session_id };
                        std::thread::spawn(move || {
                            do_translate_streaming(&original_text, &app_clone, target);
                        });
                        return Ok(true);
                    }
                    TranslateStrategy::FallbackLlm => {
                        let llm_config = crate::config::llm_config_ignore_mode()
                            .ok_or("润色模型未配置，请在设置中配置 LLM")?;
                        // 翻译用纯 text（不含 enriched 上下文标签）：
                        // 1. auto_translate_prompt 检测 CJK 判断方向——标签会干扰检测
                        // 2. 翻译结果不应包含【来源】等上下文标签
                        let prompt = auto_translate_prompt(&text).to_string();
                        let text_clone = text.clone();
                        let config_clone = llm_config.clone();
                        // LLM 调用是同步阻塞 HTTP——必须 spawn_blocking 防卡 tokio runtime
                        let result = tokio::task::spawn_blocking(move || {
                            octopus_llm::chat_text_with_prompt(&prompt, &text_clone, &config_clone, None)
                        }).await
                            .map_err(|e| e2s_ctx("LLM 线程异常: {}", e))?
                            .map_err(e2s)?;
                        action_bar_show_result(result, text, "translate".into(), app.clone(), true);
                        return Ok(true);
                    }
                }
            }

            // 非 auto_translate 的 AI 操作（润色/摘要/解释），仍走 LLM
            let llm_config = crate::config::llm_config_ignore_mode()
                .ok_or("润色模型未配置，请在设置中配置 LLM")?;
            let enriched_text = build_enriched_text(&text);
            let prompt = resolve_prompt_reference(&item.action_data);
            let config_clone = llm_config.clone();
            // LLM 调用是同步阻塞 HTTP——必须 spawn_blocking 防卡 tokio runtime
            let result = tokio::task::spawn_blocking(move || {
                octopus_llm::chat_text_with_prompt(&prompt, &enriched_text, &config_clone, None)
            }).await
                .map_err(|e| e2s_ctx("LLM 线程异常: {}", e))?
                .map_err(e2s)?;
            action_bar_show_result(result, String::new(), item.title, app.clone(), true);
            Ok(true)
        }
        "url" => {
            let url = if item.action_data.is_empty() {
                // 选中文本即 URL——仅放行 http/https，其余 scheme 统一补 https://
                // 防止 smb:// / file:/// / vnc:// 等通过选中不可信文本触发系统级操作
                let raw = text.trim();
                if raw.starts_with("http://") || raw.starts_with("https://") {
                    raw.to_string()
                } else {
                    format!("https://{}", raw)
                }
            } else {
                // URL 模板替换 {query} 和 {text} 两个占位符（与前端搜索/关键词路径对齐）
                item.action_data
                    .replace("{query}", &url_encode_param(&text))
                    .replace("{text}", &url_encode_param(&text))
            };
            // 用系统默认浏览器打开（检查退出码——无默认处理器/URL 无效时返回错误）
            crate::sys_open::open_with_default(&url).map(|_| false)
        }
        "script" => {
            let is_async = item.is_async;
            let write_output = item.write_output_to_clipboard;
            let item_title = item.title.clone();
            let item_id = item.id;

            // Package 脚本（action_data 是绝对路径）vs 内联脚本
            let is_pkg = std::path::Path::new(&item.action_data).is_absolute();
            let source = if is_pkg {
                std::fs::read_to_string(&item.action_data)
                    .map_err(|e| e2s_ctx("脚本文件不存在或无法读取: {}", e))?
            } else {
                item.action_data.clone()
            };
            let pkg_dir = if is_pkg {
                std::path::Path::new(&item.action_data).parent()
                    .map(|p| p.to_string_lossy().to_string())
            } else { None };

            if is_async {
                run_script_async(&source, &text, item_id, pkg_dir)?;
                Ok(false)
            } else {
                let text_clone = text.clone();
                let result = tokio::task::spawn_blocking(move || {
                    run_script_sync_blocking(&source, &text_clone, item_id, pkg_dir)
                }).await.map_err(|e| e2s_ctx("脚本执行线程异常: {}", e))??;

                if result.timed_out {
                    return Err("脚本执行超时（60秒），已强制终止".into());
                }
                if let Some(code) = result.exit_code {
                    if code != 0 {
                        let detail = if result.stderr.is_empty() { String::new() } else { format!("\n{}", result.stderr) };
                        return Err(format!("脚本退出码 {}{}", code, detail));
                    }
                }
                // 成功
                if !result.stdout.is_empty() {
                    if write_output {
                        write_clipboard_text(app, &result.stdout);
                    }
                    action_bar_show_result(result.stdout, text, item_title, app.clone(), false);
                    return Ok(true);
                }
                // 成功无输出 → 正常关闭
                Ok(false)
            }
        }
        "agent" => {
            // agent 桥接：渲染命令 → Terminal.app 启动。
            // 三层 fallback（v42）：菜单指定 → 系统默认 → 第一个可用。
            let (adapter, source) = crate::action_bar::agent_adapter::resolve_effective_adapter(&item.agent)?;
            if source != "menu" {
                log::info!(
                    "[action-bar] agent 菜单 '{}' 走 fallback（source={}，命中 '{}'）",
                    item.title, source, adapter.key
                );
            }
            // 非语音路径：voice 为空（无用户指令输入），text 是选中文本
            let resolved = resolve_prompt_reference(&item.action_data);
            let prompt = render_agent_prompt(&resolved, "", &text, &app_state_files);
            let cwd = derive_cwd(&app_state_files);
            let cwd_path = std::path::Path::new(&cwd);
            let command = crate::action_bar::agent_adapter::render_command(
                &adapter.command_template, &prompt, &app_state_files, &cwd,
            );
            let launcher = crate::action_bar::terminal_launcher::TerminalAppLauncher;
            use crate::action_bar::terminal_launcher::TerminalLauncher;
            // osascript 启动 Terminal.app 会 wait 子进程（200ms-2s），包 spawn_blocking
            // 避免阻塞 Tokio worker（与 spawn_script 范式一致）。
            let cwd_buf = cwd_path.to_path_buf();
            tokio::task::spawn_blocking(move || launcher.spawn(&command, &cwd_buf))
                .await
                .map_err(|e| format!("Terminal 启动任务异常: {e}"))??;
            Ok(false)
        }
        "copy_path" => {
            let formatted = format_paths(&app_state_files, &item.action_data);
            write_clipboard_text(app, &formatted);
            Ok(false)
        }
        // "copy" 类型已从 Settings UI 删除（2026-07-19）——用户改用 Cmd+C。
        // 旧 DB 残留的 actionType="copy" 菜单走 _ 分支返回错误，提示用户去 Settings 改类型。
        _ => Err(format!("未知动作类型: {}", item.action_type)),
    }
}

/// 统一执行菜单项动作。
#[tauri::command]
pub async fn execute_action_bar(item_id: i64, text: String, app: AppHandle) -> Result<(), String> {
    match execute_action_bar_inner(item_id, text, &app).await {
        Ok(true) => Ok(()),
        Ok(false) => {
            // url/script/copy 成功 → 统一收口：标准隐藏 + 焦点交还 + 重入锁复位
            // hide_action_bar_window 含 after_floating_window_hide（NSApplication::deactivate），
            // 本 command 是 async → 跑在 tokio worker 线程，MainThreadMarker::new() 返回 None
            // 导致 deactivate 静默跳过。投递到主线程执行（与 trigger_action_bar 的 show 同模式）。
            let app_for_hide = app.clone();
            let _ = app.run_on_main_thread(move || {
                hide_action_bar_window(&app_for_hide);
            });
            finalize_action_bar(&app);
            Ok(())
        }
        Err(e) => {
            // 异常路径：仅重置重入锁（不 hide——前端切 error 视图需窗口可见，
            // error 视图关闭时 action_bar_dismiss 走 hide + after_hide 递减 depth）
            finalize_action_bar(&app);
            Err(e)
        }
    }
}
