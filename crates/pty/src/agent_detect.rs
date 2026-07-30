// Agent 状态感知——OSC 133/777 序列解析状态机。
// 参考 Terax agent_detect.rs 设计。

/// agent 状态信号（emit 到前端）。
#[derive(Clone, Debug)]
pub struct AgentSignal {
    /// PTY session id
    pub id: u32,
    /// "started" | "working" | "attention" | "finished" | "exited"
    pub kind: String,
    /// agent 名称（仅 started 携带）
    pub agent: Option<String>,
}

/// OSC 序列解析状态机。
/// 从 PTY 原始字节流中提取 OSC 133/777 序列，推断 agent 状态。
pub struct AgentDetector {
    state: ParseState,
    osc_buf: Vec<u8>,
    armed: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum ParseState {
    Ground,
    Esc,    // 收到 \x1b
    Osc,    // 收到 \x1b] 进入 OSC
    OscEsc, // OSC 内收到 \x1b（可能是 ST 结束）
}

/// 已知 agent CLI 名称（匹配 OSC 133;C;<cmd> 中的命令名）。
const DEFAULT_AGENTS: &[&str] = &["claude", "codex", "gemini", "pi", "opencode"];

impl AgentDetector {
    pub fn new() -> Self {
        Self {
            state: ParseState::Ground,
            osc_buf: Vec::with_capacity(256),
            armed: false,
        }
    }

    /// arm：标记当前 session 正在运行某 agent（后续 OSC 777 才生效）。
    pub fn arm(&mut self, agent: &str) {
        self.armed = true;
        let _ = agent; // agent 名称存在 emit 时携带
    }

    /// disarm：agent 退出，取消 arm。
    pub fn disarm(&mut self) {
        self.armed = false;
    }

    pub fn is_armed(&self) -> bool {
        self.armed
    }

    /// 处理 PTY 输出字节流，提取 OSC 序列。
    /// 返回解析出的 AgentSignal 列表（可能为空）。
    pub fn process(&mut self, data: &[u8], session_id: u32) -> Vec<AgentSignal> {
        let mut signals = Vec::new();
        for &byte in data {
            match self.state {
                ParseState::Ground => {
                    if byte == 0x1b {
                        self.state = ParseState::Esc;
                    }
                }
                ParseState::Esc => {
                    if byte == b']' {
                        self.state = ParseState::Osc;
                        self.osc_buf.clear();
                    } else {
                        self.state = ParseState::Ground;
                    }
                }
                ParseState::Osc => {
                    if byte == 0x07 {
                        // BEL → OSC 结束
                        let payload = self.osc_buf.clone();
                        if let Some(sig) = self.parse_osc(&payload, session_id) {
                            signals.push(sig);
                        }
                        self.state = ParseState::Ground;
                    } else if byte == 0x1b {
                        self.state = ParseState::OscEsc;
                    } else {
                        self.osc_buf.push(byte);
                    }
                }
                ParseState::OscEsc => {
                    if byte == b'\\' {
                        // ST (\x1b\\) → OSC 结束
                        let payload = self.osc_buf.clone();
                        if let Some(sig) = self.parse_osc(&payload, session_id) {
                            signals.push(sig);
                        }
                        self.state = ParseState::Ground;
                    } else {
                        // \x1b 后非 \，回到 Osc 继续
                        self.osc_buf.push(0x1b);
                        self.osc_buf.push(byte);
                        self.state = ParseState::Osc;
                    }
                }
            }
        }
        signals
    }

    /// 解析 OSC payload。
    fn parse_osc(&mut self, payload: &[u8], session_id: u32) -> Option<AgentSignal> {
        let s = std::str::from_utf8(payload).ok()?;
        // OSC 133;C;<cmd> → agent 启动
        if let Some(cmd) = s.strip_prefix("133;C;") {
            let agent = match_agent(cmd);
            if let Some(ref name) = agent {
                self.arm(name);
            }
            return Some(AgentSignal {
                id: session_id,
                kind: "started".into(),
                agent,
            });
        }
        // OSC 133;D → 命令/agent 退出
        if s.starts_with("133;D") {
            self.disarm();
            return Some(AgentSignal {
                id: session_id,
                kind: "exited".into(),
                agent: None,
            });
        }
        // OSC 777;notify;octopus;<event> → agent hook 主动通知
        if self.armed {
            if let Some(event) = s.strip_prefix("777;notify;octopus;") {
                let kind = match event {
                    "working" => "working",
                    "attention" => "attention",
                    "finished" => "finished",
                    _ => return None,
                };
                return Some(AgentSignal {
                    id: session_id,
                    kind: kind.into(),
                    agent: None,
                });
            }
        }
        None
    }
}

/// 匹配命令名是否为已知 agent CLI。
fn match_agent(cmd: &str) -> Option<String> {
    // cmd 可能是完整路径 /usr/local/bin/claude 或 claude-alias
    let basename = cmd.split_whitespace().next()?.rsplit('/').next().unwrap_or(cmd);
    let base = basename.split('-').next().unwrap_or(basename);
    if DEFAULT_AGENTS.contains(&base) {
        Some(base.to_string())
    } else {
        None
    }
}

impl Default for AgentDetector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_osc133_command_started() {
        let mut det = AgentDetector::new();
        let sigs = det.process(b"\x1b]133;C;claude\x07", 1);
        assert_eq!(sigs.len(), 1);
        assert_eq!(sigs[0].kind, "started");
        assert_eq!(sigs[0].agent.as_deref(), Some("claude"));
        assert!(det.is_armed());
    }

    #[test]
    fn test_osc133_command_exit() {
        let mut det = AgentDetector::new();
        det.arm("claude");
        let sigs = det.process(b"\x1b]133;D\x07", 1);
        assert_eq!(sigs.len(), 1);
        assert_eq!(sigs[0].kind, "exited");
        assert!(!det.is_armed());
    }

    #[test]
    fn test_osc777_working() {
        let mut det = AgentDetector::new();
        det.arm("claude");
        let sigs = det.process(b"\x1b]777;notify;octopus;working\x07", 1);
        assert_eq!(sigs.len(), 1);
        assert_eq!(sigs[0].kind, "working");
    }

    #[test]
    fn test_osc777_not_armed_ignored() {
        let mut det = AgentDetector::new();
        // 未 arm → OSC 777 被忽略
        let sigs = det.process(b"\x1b]777;notify;octopus;working\x07", 1);
        assert!(sigs.is_empty());
    }

    #[test]
    fn test_st_terminator() {
        let mut det = AgentDetector::new();
        // ST 结束符 \x1b\\ 而非 BEL \x07
        let sigs = det.process(b"\x1b]133;C;codex\x1b\\", 2);
        assert_eq!(sigs.len(), 1);
        assert_eq!(sigs[0].agent.as_deref(), Some("codex"));
    }

    #[test]
    fn test_match_agent_path() {
        assert_eq!(match_agent("/usr/local/bin/claude"), Some("claude".into()));
        assert_eq!(match_agent("claude-alias"), Some("claude".into()));
        assert_eq!(match_agent("pi"), Some("pi".into()));
        assert_eq!(match_agent("ls -la"), None);
    }

    #[test]
    fn test_non_osc_passthrough() {
        let mut det = AgentDetector::new();
        let sigs = det.process(b"hello world\n\x1b[31mred text\x1b[0m", 1);
        assert!(sigs.is_empty());
    }
}
