//! macOS Auto-Type：用 enigo 模拟键盘输入。
//!
//! 关键：密码字段（masked input）能正常接收 CGEvent 输入，
//! 因为 enigo 发的是真实键盘事件，浏览器收到的是按键而非 DOM 填值。

use anyhow::{Context, Result};
use std::process::Command;
use std::time::Duration;

use enigo::{Direction, Enigo, Key, Keyboard, Settings};

const FOCUS_WAIT: Duration = Duration::from_millis(100);

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
/// **fail-closed**（修复 D）：osascript 失败（权限缺失/被回收）时 bail，不静默放行。
/// 避免安全校验在权限缺失时失效。
///
/// 不一致时返回 Err——调用方应放弃按键注入，降级到剪贴板路径。
fn verify_focused(expected_bundle_id: Option<&str>) -> Result<()> {
    // osascript 失败 → 直接 bail（fail-closed，修复 D）
    let actual = super::url_detect::frontmost_bundle_id()
        .context("无法读取前台 bundle id（osascript 权限缺失?），fail-closed")?
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
            // 修复 E：用 OCTOPUS_BUNDLE_ID 常量，避免硬编码散落
            if actual == OCTOPUS_BUNDLE_ID {
                anyhow::bail!(
                    "焦点仍在 octopus 自身（VaultPicker 未 hide?）——放弃按键注入防泄露"
                );
            }
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
}
