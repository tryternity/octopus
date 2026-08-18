//! 脚本执行 + 统一执行入口（从 action_bar_commands/mod.rs 提取，Task 1.6）。
//!
//! 脚本运行时探测（node/bun/deno/...）+ spawn/等待（同步阻塞 / 异步后台）+ 落库；
//! `execute_action_bar_inner` 是 ai/url/script/agent/copy_path 五种动作类型的统一执行核心，
//! `execute_action_bar` 是对外 `#[tauri::command]` 包装。

use std::sync::OnceLock;
use tauri::{AppHandle, Manager};
use crate::core::error_util::{e2s, e2s_ctx};
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
        let _ = octopus_infra::db::insert_script_run(&octopus_infra::db::ScriptRunRecord {
            item_id,
            script_type: &script_type,
            exit_code: result.exit_code,
            stdout: &result.stdout,
            stderr: &result.stderr,
            error_msg: &error_msg,
            started_at: &started_at,
            finished_at: Some(&finished_at),
            duration_ms: Some(duration_ms),
        });
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
    let _ = octopus_infra::db::insert_script_run(&octopus_infra::db::ScriptRunRecord {
        item_id,
        script_type: &script_type,
        exit_code: result.exit_code,
        stdout: &result.stdout,
        stderr: &result.stderr,
        error_msg: &error_msg,
        started_at: &started_at,
        finished_at: Some(&finished_at),
        duration_ms: Some(duration_ms),
    });
    // 标记已落库（ScriptResult 原样返回给上层）
    let _ = &mut result; // 消费 mut borrow
    Ok(result)
}

/// AI（LLM 润色/摘要/解释）操作超时秒数。auto_translate 不受此限（长文本本地翻译）。
/// spec 2026-07-31-actionbar-execute-paths-unification：超时从前端移到后端，
/// 避免 LLM 线程泄漏（前端超时只丢 UI 结果，后端线程仍跑）。
const AI_TIMEOUT_SECS: u64 = 10;

/// 执行菜单项动作核心逻辑（不含收口）。
/// Ok(true) = ai 已自行收口；Ok(false) = 成功需外层统一收口；Err = 异常需外层 finalize。
/// Quick Execute（全局快捷键）和 ActionBar 路径共用——action_bar_show_result_internal
/// 用 is_visible 检查自动适配 ActionBar 可见/不可见。
pub(crate) async fn execute_action_bar_inner(
    item_id: i64,
    text: String,
    html: Option<String>,
    files: Option<Vec<String>>,
    app: &AppHandle,
) -> Result<bool, String> {
    let item = octopus_infra::db::load_action_bar_item(item_id)
        .map_err(e2s)?
        .ok_or("菜单项不存在")?;

    // 从 PENDING_CONTEXT 取 files + html（Quick Execute 路径数据源，spec §3 优先级）
    let (app_state_files, pending_html) = {
        let guard = PENDING_CONTEXT.lock();
        guard
            .as_ref()
            .map(|c| (c.files.clone(), c.html.clone()))
            .unwrap_or_default()
    };

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
                        // 本地 / 云端模型都走流式翻译，输出到 translate_window 只读浮窗
                        //（与截图翻译一致，行为统一）。
                        // 隐藏 ActionBar 浮窗（仅可见时）+ show 译文浮窗。
                        if action_bar_visible {
                            if let Some(win) = app.get_webview_window(crate::action_bar::action_bar_window::WINDOW_LABEL) {
                                let _ = win.hide();
                            }
                            #[cfg(target_os = "macos")]
                            { crate::platform::activation::after_floating_window_hide_keep_active(app); }
                            finalize_action_bar(app);
                        }

                        let original_text = text.clone();
                        let ah = app.clone();
                        let _ = app.run_on_main_thread(move || {
                            crate::ui::translate_window::show_at_mouse(&ah);
                        });

                        let app_clone = app.clone();
                        let target = TranslateEmitTarget::Float;
                        std::thread::spawn(move || {
                            do_translate_streaming(&original_text, &app_clone, target);
                        });
                        return Ok(true);
                    }
                    TranslateStrategy::FallbackLlm => {
                        let llm_config = crate::core::config::llm_config_ignore_mode()
                            .ok_or("润色模型未配置，请在设置中配置 LLM")?;
                        // 翻译用纯 text（不含 enriched 上下文标签）：
                        // 1. auto_translate_prompt 检测 CJK 判断方向——标签会干扰检测
                        // 2. 翻译结果不应包含【来源】等上下文标签
                        let prompt = auto_translate_prompt(&text).to_string();
                        let text_clone = text.clone();
                        let config_clone = llm_config.clone();
                        // LLM 调用是同步阻塞 HTTP——必须 spawn_blocking 防卡 tokio runtime
                        // 第四十一轮 P2-低：补 timeout 包裹——同函数 AI 路径（:445-456）有
                        // timeout(AI_TIMEOUT_SECS)，FallbackLlm 路径漏了。LLM client 自带 120s
                        // timeout 兜底非永久卡死，但与同函数不对称 + UX 卡顿。超时后 spawn_blocking
                        // 线程继续跑到结束才回收（同步 reqwest 无法中断），前端立即收到 Err 释放 UI。
                        let llm_future = tokio::task::spawn_blocking(move || {
                            octopus_llm::chat_text_with_prompt(&prompt, &text_clone, &config_clone, None)
                        });
                        let result = match tokio::time::timeout(std::time::Duration::from_secs(AI_TIMEOUT_SECS), llm_future).await {
                            Ok(Ok(res)) => res.map_err(e2s)?,
                            Ok(Err(e)) => return Err(e2s_ctx("LLM 线程异常: {}", e)),
                            Err(_elapsed) => return Err(format!("自动翻译超时（{}秒）", AI_TIMEOUT_SECS)),
                        };
                        // FallbackLlm 非流式：show 浮窗后一次性 emit done
                        //（与流式分支 + 截图翻译行为统一，不再开 CompactEditor contrast tab）
                        let result_text = result.clone();
                        let ah = app.clone();
                        let _ = app.run_on_main_thread(move || {
                            crate::ui::translate_window::show_at_mouse(&ah);
                            crate::ui::translate_window::emit_float_done(&ah, &result_text);
                        });
                        return Ok(true);
                    }
                }
            }

            // 非 auto_translate 的 AI 操作（润色/摘要/解释），仍走 LLM
            let llm_config = crate::core::config::llm_config_ignore_mode()
                .ok_or("润色模型未配置，请在设置中配置 LLM")?;
            let enriched_text = build_enriched_text(&text);
            let prompt = resolve_prompt_reference(&item.action_data);
            let config_clone = llm_config.clone();
            // LLM 调用是同步阻塞 HTTP——必须 spawn_blocking 防卡 tokio runtime。
            // tokio::time::timeout 包裹：超时返回 Err（auto_translate 路径上方已 return，不进这里）。
            // 注意：超时后 spawn_blocking 线程无法立即中断（同步 reqwest），会继续跑到结束才回收——
            // 但前端立即收到 Err 释放 UI，比原「前端超时 + 后端永远跑」改进。spec 风险 #3。
            let llm_future = tokio::task::spawn_blocking(move || {
                octopus_llm::chat_text_with_prompt(&prompt, &enriched_text, &config_clone, None)
            });
            match tokio::time::timeout(std::time::Duration::from_secs(AI_TIMEOUT_SECS), llm_future).await {
                Ok(Ok(res)) => {
                    let result = res.map_err(e2s)?;
                    action_bar_show_result(result, String::new(), item.title, app.clone(), true);
                    Ok(true)
                }
                Ok(Err(e)) => Err(e2s_ctx("LLM 线程异常: {}", e)),
                Err(_elapsed) => Err(format!("AI 操作超时（{}秒）", AI_TIMEOUT_SECS)),
            }
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
            crate::platform::sys_open::open_with_default(&url).map(|_| false)
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
                // 第四十二轮 P2-3：exit_code=None（信号终止 SIGSEGV/SIGKILL）也当失败——
                // 原 if let Some(code) 漏了 None 分支（落到「成功」路径静默成功）。
                // 对称 script_error_msg :35 的 exit_code != Some(0) 写法。
                if result.exit_code != Some(0) {
                    let detail = if result.stderr.is_empty() { String::new() } else { format!("\n{}", result.stderr) };
                    match result.exit_code {
                        Some(code) => return Err(format!("脚本退出码 {}{}", code, detail)),
                        None => return Err(format!("脚本异常退出（被信号终止）{}", detail)),
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
            // agent 桥接：渲染命令 → 内嵌终端启动（Task 8）。
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
            let command = crate::action_bar::agent_adapter::render_command(
                &adapter.command_template, &prompt, &app_state_files, &cwd,
            );
            // 优先内嵌终端窗口（Task 6-7）；失败 fallback 到 Terminal.app（Task 8）。
            // open_terminal_with_command 内部用 run_on_main_thread 调度 AppKit，
            // 可在 async worker 线程安全调用（无需 spawn_blocking）。
            match crate::ui::terminal_window::open_terminal_with_command(app, Some(&cwd), &command) {
                Ok(_) => {
                    // command 可能是长 prompt（几百字），只打前 60 字符 + 省略号，避免刷屏
                    let cmd_preview: String = command.chars().take(60).collect();
                    let ellipsis = if command.chars().count() > 60 { "…" } else { "" };
                    log::info!("[action-bar] agent 已启动到内嵌终端（cwd={}, cmd={}{}）", cwd, cmd_preview, ellipsis);
                }
                Err(e) => {
                    log::warn!("[action-bar] 内嵌终端失败，fallback 到 Terminal.app: {}", e);
                    let launcher = crate::action_bar::terminal_launcher::TerminalAppLauncher;
                    use crate::action_bar::terminal_launcher::TerminalLauncher;
                    let cwd_path = std::path::Path::new(&cwd);
                    let cwd_buf = cwd_path.to_path_buf();
                    tokio::task::spawn_blocking(move || launcher.spawn(&command, &cwd_buf))
                        .await
                        .map_err(|e| format!("Terminal 启动任务异常: {e}"))??;
                }
            }
            Ok(false)
        }
        "copy_path" => {
            let formatted = format_paths(&app_state_files, &item.action_data);
            write_clipboard_text(app, &formatted);
            Ok(false)
        }
        "markdown" => {
            // 输入优先级（spec §3）：显式 files > PENDING files > html（显式 > PENDING）> text。
            // 异步执行（spec §5.2 修订 2026-08-18）：立即返回 Ok(false)（外层统一收口隐藏
            // ActionBar），后台转换 → 写文件 ~/Documents/octopus/markitdown/ → CompactEditor
            // file tab 打开（编辑保存可写回）；失败开错误 temp tab（agent-task://error 只有
            // Result 浮窗监听、不一定可见，编辑器反馈更可靠）。
            let files_in = files.filter(|f| !f.is_empty()).unwrap_or(app_state_files);
            let html_in = html.or(pending_html);
            let text_in = text.clone();
            let write_clipboard = item.write_output_to_clipboard;
            let ah = app.clone();
            tokio::spawn(async move {
                let inputs = (files_in, html_in, text_in);
                let ah_sb = ah.clone();
                let result = tokio::task::spawn_blocking(move || {
                    let (f, h, t) = inputs;
                    crate::action_bar::action_bar_commands::markdown::convert_and_save(&ah_sb, f, h, t)
                })
                .await
                .map_err(|e| format!("转换线程异常: {}", e));
                match result {
                    Ok(Ok((path, md))) => {
                        if write_clipboard {
                            write_clipboard_text(&ah, &md);
                        }
                        let path_str = path.to_string_lossy().to_string();
                        let ah2 = ah.clone();
                        // create_compact_editor_window 内含 set_dock_icon 需主线程
                        //（同 action_bar_show_result_internal 的投递模式）
                        let _ = ah.run_on_main_thread(move || {
                            if let Err(e) = crate::commands::compact_editor_commands
                                ::open_disk_file_in_compact_editor(&ah2, &path_str)
                            {
                                log::warn!("[action-bar] 打开转换结果失败: {}", e);
                            }
                        });
                    }
                    Ok(Err(e)) | Err(e) => {
                        log::warn!("[action-bar] 转 Markdown 异步失败: {}", e);
                        let payload = crate::commands::compact_editor_commands::TempTabPayload {
                            text: format!("【转 Markdown 失败】\n{}", e),
                            ..Default::default()
                        };
                        let ah_recv = ah.clone();
                        let ah_cap = ah_recv.clone();
                        let _ = ah_recv.run_on_main_thread(move || {
                            crate::commands::compact_editor_commands::open_temp_compact_editor(
                                &ah_cap, &payload,
                            );
                        });
                    }
                }
            });
            Ok(false)
        }
        // "copy" 类型已从 Settings UI 删除（2026-07-19）——用户改用 Cmd+C。
        // 旧 DB 残留的 actionType="copy" 菜单走 _ 分支返回错误，提示用户去 Settings 改类型。
        _ => Err(format!("未知动作类型: {}", item.action_type)),
    }
}

/// 统一执行菜单项动作。html/files 为前端透传的可选上下文（markdown 命令用，spec §5.2）；
/// Quick Execute 路径传 None，由 inner 回退读 PENDING_CONTEXT。
#[tauri::command]
pub async fn execute_action_bar(
    item_id: i64,
    text: String,
    html: Option<String>,
    files: Option<Vec<String>>,
    app: AppHandle,
) -> Result<(), String> {
    match execute_action_bar_inner(item_id, text, html, files, &app).await {
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
