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
pub fn autotype_login(username: &str, password: &str, press_enter: bool) -> Result<()> {
    std::thread::sleep(FOCUS_WAIT);

    let mut enigo = Enigo::new(&Settings::default()).context("enigo 初始化失败")?;

    // username
    enigo.text(username).context("输入 username 失败")?;

    // Tab → password 字段
    enigo
        .key(Key::Tab, Direction::Click)
        .context("Tab 输入失败")?;
    std::thread::sleep(Duration::from_millis(30));

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
