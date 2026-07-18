//! macOS Auto-Type：用 enigo 模拟键盘输入。
//!
//! 关键：密码字段（masked input）能正常接收 CGEvent 输入，
//! 因为 enigo 发的是真实键盘事件，浏览器收到的是按键而非 DOM 填值。

use anyhow::{Context, Result};
use std::process::Command;
use std::time::Duration;

use enigo::{Direction, Enigo, Key, Keyboard, Settings};

const FOCUS_WAIT: Duration = Duration::from_millis(100);

/// 把指定 bundle_id 的 app 激活到前台。
pub fn activate_app(bundle_id: &str) -> Result<()> {
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
pub fn autotype_login(
    username: &str,
    password: &str,
    press_enter: bool,
    expected_bundle_id: Option<&str>,
) -> Result<()> {
    std::thread::sleep(FOCUS_WAIT);

    // 注入 username 前先校验焦点——若已切到第三方 app，立即放弃注入
    verify_focused(expected_bundle_id)?;

    let mut enigo = Enigo::new(&Settings::default()).context("enigo 初始化失败")?;

    // username
    enigo.text(username).context("输入 username 失败")?;

    // Tab → password 字段
    enigo
        .key(Key::Tab, Direction::Click)
        .context("Tab 输入失败")?;
    std::thread::sleep(Duration::from_millis(30));

    // 注入 password 前再校验——Tab 期间焦点可能已被抢
    verify_focused(expected_bundle_id)?;

    // password
    enigo.text(password).context("输入 password 失败")?;

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
/// - `Some(expected)`：前台必须 == expected，否则 bail（严格白名单）
/// - `None`：前台只需 ≠ octopus 自身 bundle id（最小防御，防焦点抢到 octopus 窗口）
///
/// 不一致时返回 Err——调用方应放弃按键注入，降级到剪贴板路径。
fn verify_focused(expected_bundle_id: Option<&str>) -> Result<()> {
    let actual = super::url_detect::frontmost_bundle_id()
        .unwrap_or_default()
        .trim()
        .to_string();
    match expected_bundle_id {
        Some(expected) => {
            if actual != expected.trim() {
                anyhow::bail!(
                    "焦点已切换（期望 {}, 实际 {}）——放弃按键注入防钓鱼",
                    expected,
                    actual
                );
            }
        }
        None => {
            // octopus 自身 bundle id（main.rs identifier 一致）
            // 校验失败时 bail——焦点在 octopus 自己说明 VaultPicker 没 hide 或焦点被抢回
            if actual == "com.octopus.desktop" {
                anyhow::bail!(
                    "焦点仍在 octopus 自身（VaultPicker 未 hide?）——放弃按键注入防泄露"
                );
            }
        }
    }
    Ok(())
}
