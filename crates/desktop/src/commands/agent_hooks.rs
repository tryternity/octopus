//! Agent CLI hook 安装——把 OSC 777 marker 注入 Claude/Codex/Gemini/Pi 的配置文件。
//!
//! 参考 Terax agent.rs。原理：agent CLI 的 hook 机制在特定事件
//! （UserPromptSubmit / Stop / PermissionRequest 等）执行 shell 命令，
//! 我们注入的命令在 `$OCTOPUS_TERMINAL=1` 时 emit `777;notify;octopus;<agent>;<event>`，
//! PTY reader 的 AgentDetector 解析后 emit "agent://signal"，前端更新状态徽章。
//!
//! 安全保证：
//! - `write_atomic`：tmp + rename，绝不写半个文件
//! - `OWNED_MARKERS`：prune 我们自己历史发过的所有 marker 变体，保证幂等
//! - merge 不覆盖用户已有 hook（只 append 我们的 group）
//! - existing_config 遇到无效 JSON 拒绝覆写（不破坏用户配置）

use serde_json::{json, Value};

/// hook 命令如何把 OSC 777 marker 送进终端。
#[derive(Clone, Copy)]
enum Delivery {
    /// Claude v2.1.139+ 丢了 /dev/tty 访问，通过 `terminalSequence` JSON 字段返回序列，
    /// 由 host 在终端 in-band 发射。跨平台。
    TerminalSequence,
    /// Codex/Gemini hook 不能写终端，hook 命令自己 emit marker：Unix 写 /dev/tty。
    Osc,
}

struct AgentSpec {
    agent: &'static str,
    dir: &'static str,
    file: &'static str,
    events: &'static [(&'static str, &'static str)],
    matcher: bool,
    delivery: Delivery,
}

const AGENTS: &[AgentSpec] = &[
    AgentSpec {
        agent: "claude",
        dir: ".claude",
        file: "settings.json",
        events: &[
            ("UserPromptSubmit", "working"),
            ("Notification", "attention"),
            ("Stop", "finished"),
        ],
        matcher: false,
        delivery: Delivery::TerminalSequence,
    },
    AgentSpec {
        agent: "codex",
        dir: ".codex",
        file: "hooks.json",
        events: &[
            ("UserPromptSubmit", "working"),
            ("PermissionRequest", "attention"),
            ("Stop", "finished"),
        ],
        matcher: false,
        delivery: Delivery::Osc,
    },
    AgentSpec {
        agent: "gemini",
        dir: ".gemini",
        file: "settings.json",
        events: &[
            ("BeforeAgent", "working"),
            ("Notification", "attention"),
            ("AfterAgent", "finished"),
        ],
        matcher: true,
        delivery: Delivery::Osc,
    },
];

// Pi 走扩展机制（TS 文件），不是 JSON hook。
const PI_EXTENSION_DIR: &str = ".pi/agent/extensions";
const PI_EXTENSION_FILE: &str = "octopus-notifications.ts";
const PI_EXTENSION_MARKER: &str = "octopus-pi-notifications-v1";
const PI_STATUS_NEEDLES: [&str; 6] = [
    PI_EXTENSION_MARKER,
    "agent_start",
    "agent_settled",
    "notify;octopus;pi;${event}",
    "emit(\"working\")",
    "emit(\"finished\")",
];
const PI_EXTENSION: &str = r#"// octopus-pi-notifications-v1
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

export default function (pi: ExtensionAPI) {
  const emit = (event: "working" | "finished") => {
    if (process.env.OCTOPUS_TERMINAL) {
      process.stdout.write(`\u001b]777;notify;octopus;pi;${event}\u0007`);
    }
  };

  pi.on("agent_start", () => emit("working"));
  pi.on("agent_settled", () => emit("finished"));
}
"#;

// 识别某 hook 命令是我们的标记——覆盖历史发过的所有变体。
// 用于 prune 旧条目，保证安装幂等 + 迁移老 marker。
const OWNED_MARKERS: [&str; 3] = ["notify;octopus;", "octopus;notify", "__octopus_notify"];

fn find(agent: &str) -> Result<&'static AgentSpec, String> {
    AGENTS
        .iter()
        .find(|s| s.agent == agent)
        .ok_or_else(|| format!("unknown agent {agent}"))
}

fn hook_command(spec: &AgentSpec, event: &str) -> String {
    match spec.delivery {
        Delivery::TerminalSequence => format!(
            r#"[ -n "$OCTOPUS_TERMINAL" ] && printf '{{"terminalSequence":"\\u001b]777;notify;octopus;{event}\\u0007"}}' || true"#
        ),
        Delivery::Osc => osc_command(spec.agent, event),
    }
}

// marker 写 /dev/tty，然后 stdout 输出 `{}`：Codex/Gemini 要求 JSON no-op。
fn osc_command(agent: &str, event: &str) -> String {
    format!(
        r#"[ -n "$OCTOPUS_TERMINAL" ] && printf '\033]777;notify;octopus;{agent};{event}\007' > /dev/tty; printf '{{}}'"#
    )
}

// 证明某 (agent, event) hook 已安装的稳定子串。与 hook_command 同步，
// 这样 status 反映 enable 实际写入的内容。
fn status_needle(spec: &AgentSpec, event: &str) -> String {
    match spec.delivery {
        Delivery::TerminalSequence => format!("notify;octopus;{event}"),
        Delivery::Osc => format!("notify;octopus;{};{event}", spec.agent),
    }
}

fn is_ours(group: &Value) -> bool {
    group
        .get("hooks")
        .and_then(Value::as_array)
        .is_some_and(|hs| {
            hs.iter().any(|h| {
                h.get("command")
                    .and_then(Value::as_str)
                    .is_some_and(|c| OWNED_MARKERS.iter().any(|m| c.contains(m)))
            })
        })
}

// 空 hooks 的 group 是惰性残留（删了命令但没删 wrapper）。丢掉保持文件干净。
fn is_empty_group(group: &Value) -> bool {
    group
        .get("hooks")
        .and_then(Value::as_array)
        .is_none_or(|hs| hs.is_empty())
}

fn merge_hooks(mut root: Value, spec: &AgentSpec) -> Value {
    if !root.is_object() {
        root = json!({});
    }
    let obj = root.as_object_mut().unwrap();
    let hooks = obj.entry("hooks").or_insert_with(|| json!({}));
    if !hooks.is_object() {
        *hooks = json!({});
    }
    let hooks = hooks.as_object_mut().unwrap();

    for (event, marker) in spec.events {
        let arr = hooks.entry(*event).or_insert_with(|| json!([]));
        if !arr.is_array() {
            *arr = json!([]);
        }
        let arr = arr.as_array_mut().unwrap();
        arr.retain(|group| !is_ours(group) && !is_empty_group(group));
        let mut group = json!({
            "hooks": [ { "type": "command", "command": hook_command(spec, marker) } ]
        });
        if spec.matcher {
            group["matcher"] = json!("*");
        }
        arr.push(group);
    }
    root
}

fn existing_config(contents: Option<&str>, path: &std::path::Path) -> Result<Value, String> {
    match contents {
        Some(s) if !s.trim().is_empty() => serde_json::from_str::<Value>(s).map_err(|e| {
            format!("{} is not valid JSON ({e}); refusing to overwrite", path.display())
        }),
        _ => Ok(json!({})),
    }
}

fn home_path(dir: &str, file: &str) -> Result<std::path::PathBuf, String> {
    Ok(dirs::home_dir()
        .ok_or_else(|| "could not resolve home dir".to_string())?
        .join(dir)
        .join(file))
}

fn settings_path(spec: &AgentSpec) -> Result<std::path::PathBuf, String> {
    home_path(spec.dir, spec.file)
}

fn pi_extension_path() -> Result<std::path::PathBuf, String> {
    home_path(PI_EXTENSION_DIR, PI_EXTENSION_FILE)
}

fn pi_extension_contents(
    existing: Option<&str>,
    path: &std::path::Path,
) -> Result<&'static str, String> {
    if existing.is_some_and(|s| !s.trim().is_empty() && !s.contains(PI_EXTENSION_MARKER)) {
        return Err(format!(
            "{} is not managed by octopus; refusing to overwrite",
            path.display()
        ));
    }
    Ok(PI_EXTENSION)
}

fn write_atomic(path: &std::path::Path, contents: &str) -> Result<(), String> {
    let tmp = path.with_extension("octopus-tmp");
    std::fs::write(&tmp, contents).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("rename into {}: {e}", path.display())
    })
}

fn pi_extension_write_path(path: &std::path::Path) -> Result<std::path::PathBuf, String> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            std::fs::canonicalize(path).map_err(|e| format!("resolve {}: {e}", path.display()))
        }
        Ok(_) => Ok(path.to_path_buf()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(path.to_path_buf()),
        Err(e) => Err(format!("inspect {}: {e}", path.display())),
    }
}

fn enable_pi_extension_at(path: &std::path::Path) -> Result<(), String> {
    let dir = path.parent().unwrap();
    std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let existing = match std::fs::read_to_string(path) {
        Ok(s) if s == PI_EXTENSION => return Ok(()),
        Ok(s) => Some(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(format!("read {}: {e}", path.display())),
    };
    let contents = pi_extension_contents(existing.as_deref(), path)?;
    write_atomic(&pi_extension_write_path(path)?, contents)
}

fn enable_pi_extension() -> Result<(), String> {
    enable_pi_extension_at(&pi_extension_path()?)
}

/// 安装某 agent 的 hook（写配置文件，幂等）。
///
/// 前端在用户选 agent 时调，把 OSC 777 marker 注入 agent 配置。
/// 已安装则 no-op（prune + 重写相同内容）。
#[tauri::command]
pub fn agent_enable_hooks(agent: String) -> Result<(), String> {
    if agent == "pi" {
        return enable_pi_extension();
    }
    let spec = find(&agent)?;
    let path = settings_path(spec)?;
    let dir = path.parent().unwrap();
    std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;

    let existing = match std::fs::read_to_string(&path) {
        Ok(s) => existing_config(Some(&s), &path)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => json!({}),
        Err(e) => return Err(format!("read {}: {e}", path.display())),
    };

    let merged = merge_hooks(existing, spec);
    let out = serde_json::to_string_pretty(&merged).map_err(|e| e.to_string())?;
    write_atomic(&path, &out)
}

/// 查询某 agent 的 hook 是否已安装（前端显示开关状态）。
#[tauri::command]
pub fn agent_hooks_status(agent: String) -> bool {
    if agent == "pi" {
        return pi_extension_path()
            .ok()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .is_some_and(|content| {
                PI_STATUS_NEEDLES
                    .iter()
                    .all(|needle| content.contains(needle))
            });
    }
    let Ok(spec) = find(&agent) else {
        return false;
    };
    let Some(content) = settings_path(spec)
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
    else {
        return false;
    };
    spec.events
        .iter()
        .all(|(_, m)| content.contains(&status_needle(spec, m)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(agent: &str) -> &'static AgentSpec {
        find(agent).unwrap()
    }

    fn hook_count(root: &Value, event: &str) -> usize {
        root["hooks"][event].as_array().map_or(0, Vec::len)
    }

    fn command(root: &Value, event: &str, idx: usize) -> String {
        root["hooks"][event][idx]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .to_string()
    }

    #[test]
    fn claude_adds_all_event_hooks_to_empty_config() {
        let out = merge_hooks(json!({}), spec("claude"));
        assert_eq!(hook_count(&out, "UserPromptSubmit"), 1);
        assert_eq!(hook_count(&out, "Notification"), 1);
        assert_eq!(hook_count(&out, "Stop"), 1);
        assert!(command(&out, "Notification", 0).contains("notify;octopus;attention"));
        assert!(command(&out, "Stop", 0).contains("notify;octopus;finished"));
        assert!(command(&out, "UserPromptSubmit", 0).contains("notify;octopus;working"));
        assert!(command(&out, "Stop", 0).contains("terminalSequence"));
        assert!(!command(&out, "Stop", 0).contains("/dev/tty"));
    }

    #[test]
    fn is_idempotent_per_agent() {
        for agent in ["claude", "codex", "gemini"] {
            let s = spec(agent);
            let once = merge_hooks(json!({}), s);
            let twice = merge_hooks(once.clone(), s);
            assert_eq!(once, twice, "{agent} not idempotent");
        }
    }

    #[test]
    fn codex_emits_four_field_dev_tty_marker() {
        let out = merge_hooks(json!({}), spec("codex"));
        assert_eq!(hook_count(&out, "UserPromptSubmit"), 1);
        assert_eq!(hook_count(&out, "PermissionRequest"), 1);
        assert_eq!(hook_count(&out, "Stop"), 1);
        let stop = command(&out, "Stop", 0);
        assert!(stop.contains("notify;octopus;codex;finished"));
        assert!(stop.contains("> /dev/tty"));
        // Codex Stop 拒绝空/非 JSON stdout；hook 必须 emit no-op。
        assert!(stop.contains("printf '{}'"));
        assert!(!stop.contains("terminalSequence"));
    }

    #[test]
    fn gemini_uses_matcher_and_named_marker() {
        let out = merge_hooks(json!({}), spec("gemini"));
        assert_eq!(out["hooks"]["BeforeAgent"][0]["matcher"], "*");
        assert!(command(&out, "AfterAgent", 0).contains("notify;octopus;gemini;finished"));
        assert!(command(&out, "Notification", 0).contains("notify;octopus;gemini;attention"));
    }

    #[test]
    fn pi_extension_emits_named_working_and_finished_markers() {
        let path = std::path::Path::new("/x/octopus-notifications.ts");
        let extension = pi_extension_contents(None, path).unwrap();
        for needle in PI_STATUS_NEEDLES {
            assert!(extension.contains(needle), "missing {needle}");
        }
        assert!(extension.contains("process.env.OCTOPUS_TERMINAL"));
        assert!(extension.contains("process.stdout.write"));
    }

    #[test]
    fn pi_extension_only_replaces_octopus_owned_file() {
        let path = std::path::Path::new("/x/octopus-notifications.ts");
        assert!(pi_extension_contents(Some("export const mine = true;"), path).is_err());
        assert!(pi_extension_contents(Some(PI_EXTENSION), path).is_ok());
        assert!(pi_extension_contents(Some("  \n"), path).is_ok());
    }

    #[test]
    fn pi_extension_install_is_atomic_idempotent_and_preserves_foreign_files() {
        let dir = std::env::temp_dir().join(format!("octopus-pi-extension-{}", std::process::id()));
        let path = dir.join(PI_EXTENSION_FILE);
        let _ = std::fs::remove_dir_all(&dir);

        enable_pi_extension_at(&path).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), PI_EXTENSION);
        enable_pi_extension_at(&path).unwrap();

        std::fs::write(&path, "export const mine = true;").unwrap();
        assert!(enable_pi_extension_at(&path).is_err());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "export const mine = true;"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn preserves_unrelated_settings_and_foreign_hooks() {
        let input = json!({
            "permissions": { "allow": ["Bash"] },
            "hooks": {
                "Notification": [
                    { "hooks": [ { "type": "command", "command": "say hi" } ] }
                ]
            }
        });
        let out = merge_hooks(input, spec("claude"));
        assert_eq!(out["permissions"]["allow"][0], "Bash");
        assert_eq!(hook_count(&out, "Notification"), 2);
        assert_eq!(command(&out, "Notification", 0), "say hi");
    }

    #[test]
    fn replaces_non_object_root() {
        let out = merge_hooks(json!("garbage"), spec("codex"));
        assert_eq!(hook_count(&out, "Stop"), 1);
    }

    #[test]
    fn prunes_empty_groups_and_collapses_duplicates() {
        let input = json!({
            "hooks": {
                "Notification": [
                    { "hooks": [] },
                    { "hooks": [ { "type": "command", "command": hook_command(spec("claude"), "attention") } ] }
                ]
            }
        });
        let out = merge_hooks(input, spec("claude"));
        assert_eq!(hook_count(&out, "Notification"), 1);
        assert!(command(&out, "Notification", 0).contains("notify;octopus;attention"));
    }

    #[test]
    fn existing_config_absent_or_empty_starts_fresh() {
        let p = std::path::Path::new("/x/settings.json");
        assert_eq!(existing_config(None, p).unwrap(), json!({}));
        assert_eq!(existing_config(Some("   \n"), p).unwrap(), json!({}));
    }

    #[test]
    fn existing_config_refuses_to_clobber_invalid_json() {
        let p = std::path::Path::new("/x/settings.json");
        assert!(existing_config(Some("{ not json,"), p).is_err());
        assert_eq!(
            existing_config(Some(r#"{"permissions":{}}"#), p).unwrap(),
            json!({ "permissions": {} })
        );
    }
}
