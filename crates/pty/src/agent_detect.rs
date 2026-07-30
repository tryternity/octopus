// Agent 状态感知——OSC 133/777 序列解析状态机。
// 参考 Terax agent_detect.rs 设计，macOS-only。
//
// 核心能力（相对原简化版补全）：
// - `Transition` enum 替代裸 String kind（类型安全）
// - `finish()` —— PTY 关闭时发 Exited，避免 UI 留 stale 条目
// - auto-arm —— bash 无 preexec，靠 OSC 777 marker 自我 arm（`ensure_armed`）
// - OSC 9 attention（generic desktop notification）
// - 4-field marker `777;notify;octopus;<agent>;<event>`（Codex/Gemini/Pi）
// - `OSC_MAX` 溢出防护（>2048 字节的 OSC 不会 panic）
// - `status` 字段——Working 状态不重复 emit

const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;
const OSC_INTRO: u8 = b']';
const ST_FINAL: u8 = b'\\';

const OSC_MAX: usize = 2048;

/// 已知 agent CLI 名称（匹配 OSC 133;C;<cmd> 中的命令名）。
const DEFAULT_AGENTS: &[&str] = &["claude", "codex", "gemini", "pi", "opencode", "grok"];

/// OSC 777 marker——agent hook 主动通知。
/// 兼容 3-field `notify;octopus;<event>`（Claude）和 4-field `notify;octopus;<agent>;<event>`（Codex/Gemini/Pi）。
const OCTOPUS_MARKER: &[u8] = b"notify;octopus;";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum State {
    Ground,
    Esc,
    Osc,
    OscEsc,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Status {
    Working,
    Waiting,
}

/// agent 状态转换（解析 OSC 后产生）。
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Transition {
    /// agent 启动（OSC 133;C 匹配到已知 agent，或 OSC 777 auto-arm）
    Started { agent: String },
    /// agent 开始工作（OSC 777 working）
    Working,
    /// agent 等待用户输入（OSC 777 attention / OSC 9）
    Attention,
    /// agent 完成（OSC 777 finished）
    Finished,
    /// agent 退出（OSC 133;D 或 PTY 关闭）
    Exited,
}

/// agent 状态信号（emit 到前端）。
#[derive(Clone, Debug, serde::Serialize)]
pub struct AgentSignal {
    /// PTY session id
    pub id: u32,
    /// "started" | "working" | "attention" | "finished" | "exited"
    #[serde(rename = "kind")]
    pub kind: &'static str,
    /// agent 名称（仅 started 携带）
    #[serde(rename = "agent")]
    pub agent: Option<String>,
}

impl Transition {
    pub fn into_signal(self, id: u32) -> AgentSignal {
        match self {
            Transition::Started { agent } => AgentSignal {
                id,
                kind: "started",
                agent: Some(agent),
            },
            Transition::Working => AgentSignal {
                id,
                kind: "working",
                agent: None,
            },
            Transition::Attention => AgentSignal {
                id,
                kind: "attention",
                agent: None,
            },
            Transition::Finished => AgentSignal {
                id,
                kind: "finished",
                agent: None,
            },
            Transition::Exited => AgentSignal {
                id,
                kind: "exited",
                agent: None,
            },
        }
    }
}

/// OSC 序列解析状态机。
///
/// 从 PTY 原始字节流中提取 OSC 133/777/9 序列，推断 agent 状态。
pub struct AgentDetector {
    agents: Vec<String>,
    state: State,
    osc: Vec<u8>,
    armed: bool,
    status: Status,
}

impl AgentDetector {
    pub fn new() -> Self {
        Self::with_agents(DEFAULT_AGENTS.iter().map(|s| s.to_string()).collect())
    }

    pub fn with_agents(agents: Vec<String>) -> Self {
        Self {
            agents,
            state: State::Ground,
            osc: Vec::new(),
            armed: false,
            status: Status::Working,
        }
    }

    /// arm：标记当前 session 正在运行某 agent。
    fn arm(&mut self, agent: &str) {
        self.armed = true;
        self.status = Status::Working;
        let _ = agent;
    }

    fn disarm(&mut self) {
        self.armed = false;
        self.status = Status::Working;
    }

    pub fn is_armed(&self) -> bool {
        self.armed
    }

    /// 处理 PTY 输出字节流，提取 OSC 序列。
    /// Transition 仅来自 OSC（133 prompt 边界、777 hook marker、9 desktop notify），
    /// 永不来自原始输出——TUI agent 持续重绘不会让 working/waiting 抖动。
    pub fn process<F: FnMut(Transition)>(&mut self, input: &[u8], mut emit: F) {
        // 快速路径：Ground 态且无 ESC 字节，直接放行（绝大多数 chunk 走这条）。
        if self.state == State::Ground && !input.contains(&ESC) {
            return;
        }

        for &b in input {
            match self.state {
                State::Ground => {
                    if b == ESC {
                        self.state = State::Esc;
                    }
                }
                State::Esc => match b {
                    OSC_INTRO => {
                        self.state = State::Osc;
                        self.osc.clear();
                    }
                    ESC => {} // 连续 ESC，停留在 Esc
                    _ => self.state = State::Ground,
                },
                State::Osc => match b {
                    BEL => {
                        self.finish_osc(&mut emit);
                        self.state = State::Ground;
                    }
                    ESC => self.state = State::OscEsc,
                    _ => {
                        if self.osc.len() < OSC_MAX {
                            self.osc.push(b);
                        } else {
                            // 超长 OSC 丢弃，防 TUI 恶意 payload 撑爆缓冲。
                            self.osc.clear();
                            self.state = State::Ground;
                        }
                    }
                },
                State::OscEsc => match b {
                    ST_FINAL => {
                        self.finish_osc(&mut emit);
                        self.state = State::Ground;
                    }
                    ESC => {} // OSC 内连续 ESC
                    _ => {
                        self.osc.clear();
                        self.state = State::Ground;
                    }
                },
            }
        }
    }

    /// PTY 关闭时调用。若 armed 则发 Exited，避免 UI 留 stale 条目
    /// （shell 中途死了，没来得及发 133;D）。
    pub fn finish<F: FnMut(Transition)>(&mut self, mut emit: F) {
        if self.armed {
            self.disarm();
            emit(Transition::Exited);
        }
    }

    fn finish_osc<F: FnMut(Transition)>(&mut self, emit: &mut F) {
        let body = std::mem::take(&mut self.osc);
        let (ps, pt) = match body.iter().position(|&c| c == b';') {
            Some(i) => (&body[..i], &body[i + 1..]),
            None => (&body[..], &body[0..0]),
        };
        match ps {
            b"133" => self.handle_osc133(pt, emit),
            // OSC 9;4 是 taskbar 进度，不是 attention 通知。
            b"9" if !pt.starts_with(b"4;") && pt != b"4" => self.generic_attention(emit),
            b"777" => self.handle_osc777(pt, emit),
            _ => {}
        }
    }

    fn handle_osc777<F: FnMut(Transition)>(&mut self, pt: &[u8], emit: &mut F) {
        if let Some(tail) = pt.strip_prefix(OCTOPUS_MARKER) {
            // PTY 输出不可信：仅对已知 agent 自我 arm。
            let (agent, event) = match tail.iter().position(|&c| c == b';') {
                Some(i) => {
                    let Ok(name) = std::str::from_utf8(&tail[..i]) else {
                        return;
                    };
                    if !self.agents.iter().any(|a| a == name) {
                        return;
                    }
                    (name, &tail[i + 1..])
                }
                // 3-field marker：未指定 agent，默认 claude（Terax 约定）。
                None => ("claude", tail),
            };
            // bash 无 preexec，OSC 777 来了才自我 arm（auto-arm）。
            match event {
                b"working" => {
                    self.ensure_armed(agent, emit);
                    self.set_working(emit);
                }
                b"attention" => {
                    self.ensure_armed(agent, emit);
                    self.status = Status::Waiting;
                    emit(Transition::Attention);
                }
                b"finished" => {
                    self.ensure_armed(agent, emit);
                    self.status = Status::Waiting;
                    emit(Transition::Finished);
                }
                _ => {}
            }
            return;
        }
        self.generic_attention(emit);
    }

    fn handle_osc133<F: FnMut(Transition)>(&mut self, pt: &[u8], emit: &mut F) {
        match pt.first() {
            Some(b'C') => {
                if self.armed {
                    return; // 已 arm（运行中 agent），忽略重复 started
                }
                let cmd = pt.strip_prefix(b"C;").unwrap_or(b"");
                if let Some(agent) = self.match_agent(cmd) {
                    self.arm(&agent);
                    emit(Transition::Started { agent });
                }
            }
            Some(b'D') if self.armed => {
                self.disarm();
                emit(Transition::Exited);
            }
            _ => {}
        }
    }

    fn ensure_armed<F: FnMut(Transition)>(&mut self, agent: &str, emit: &mut F) {
        if !self.armed {
            self.arm(agent);
            emit(Transition::Started {
                agent: agent.to_string(),
            });
        }
    }

    fn set_working<F: FnMut(Transition)>(&mut self, emit: &mut F) {
        if self.status != Status::Working {
            self.status = Status::Working;
            emit(Transition::Working);
        }
    }

    fn generic_attention<F: FnMut(Transition)>(&mut self, emit: &mut F) {
        if self.armed {
            self.status = Status::Waiting;
            emit(Transition::Attention);
        }
    }

    /// 匹配命令名是否为已知 agent CLI。
    /// 支持路径前缀（/usr/local/bin/claude）、npx 包装（npx claude）、
    /// 连字符后缀（claude-enigma）。
    fn match_agent(&self, cmd: &[u8]) -> Option<String> {
        let cmd = std::str::from_utf8(cmd).ok()?;
        for token in cmd.split_whitespace() {
            if token.starts_with('-') {
                continue; // flag，跳过
            }
            let base = token.rsplit(['/', '\\']).next().unwrap_or(token);
            if let Some(agent) = self.agents.iter().find(|a| {
                base.strip_prefix(a.as_str())
                    .is_some_and(|rest| rest.is_empty() || rest.starts_with('-'))
            }) {
                return Some(agent.clone());
            }
        }
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

    fn run(d: &mut AgentDetector, input: &[u8]) -> Vec<Transition> {
        let mut out = Vec::new();
        d.process(input, |t| out.push(t));
        out
    }

    fn osc(body: &str) -> Vec<u8> {
        let mut v = vec![ESC, OSC_INTRO];
        v.extend_from_slice(body.as_bytes());
        v.extend_from_slice(&[ESC, ST_FINAL]);
        v
    }

    fn started(agent: &str) -> Transition {
        Transition::Started {
            agent: agent.into(),
        }
    }

    #[test]
    fn arms_on_agent_command() {
        let mut d = AgentDetector::new();
        assert_eq!(
            run(&mut d, &osc("133;C;claude -p hello")),
            vec![started("claude")]
        );
    }

    #[test]
    fn arms_on_pi_command() {
        let mut d = AgentDetector::new();
        assert_eq!(run(&mut d, &osc("133;C;pi")), vec![started("pi")]);
    }

    #[test]
    fn arms_on_pathed_and_wrapped_command() {
        let mut d = AgentDetector::new();
        assert_eq!(
            run(&mut d, &osc("133;C;/usr/local/bin/codex exec")),
            vec![started("codex")]
        );
        let mut d2 = AgentDetector::new();
        assert_eq!(
            run(&mut d2, &osc("133;C;npx claude")),
            vec![started("claude")]
        );
    }

    #[test]
    fn arms_on_dash_suffixed_alias() {
        let mut d = AgentDetector::new();
        assert_eq!(
            run(&mut d, &osc("133;C;claude-enigma")),
            vec![started("claude")]
        );
    }

    #[test]
    fn does_not_arm_on_other_commands() {
        let mut d = AgentDetector::new();
        assert!(run(&mut d, &osc("133;C;vim src/main.rs")).is_empty());
        assert!(run(&mut d, &osc("133;C;cat claude.txt")).is_empty());
        assert!(run(&mut d, &osc("133;C;claudexyz")).is_empty());
    }

    #[test]
    fn ignores_bell_and_plain_output() {
        let mut d = AgentDetector::new();
        run(&mut d, &osc("133;C;claude"));
        assert!(run(&mut d, &[BEL]).is_empty());
        assert!(run(&mut d, b"thinking...\x07more").is_empty());
    }

    #[test]
    fn octopus_marker_drives_status() {
        let mut d = AgentDetector::new();
        run(&mut d, &osc("133;C;claude"));
        assert_eq!(
            run(&mut d, &osc("777;notify;octopus;attention")),
            vec![Transition::Attention]
        );
        assert_eq!(
            run(&mut d, &osc("777;notify;octopus;working")),
            vec![Transition::Working]
        );
        // Working 状态重复不发
        assert!(run(&mut d, &osc("777;notify;octopus;working")).is_empty());
        assert_eq!(
            run(&mut d, &osc("777;notify;octopus;finished")),
            vec![Transition::Finished]
        );
    }

    #[test]
    fn octopus_marker_auto_arms_without_preexec() {
        // bash 无 preexec，OSC 777 来了才 arm（auto-arm）。
        let mut d = AgentDetector::new();
        assert_eq!(
            run(&mut d, &osc("777;notify;octopus;attention")),
            vec![started("claude"), Transition::Attention]
        );
    }

    #[test]
    fn four_field_marker_self_arms_named_agent() {
        let mut d = AgentDetector::new();
        assert_eq!(
            run(&mut d, &osc("777;notify;octopus;codex;working")),
            vec![started("codex")]
        );
        let mut g = AgentDetector::new();
        assert_eq!(
            run(&mut g, &osc("777;notify;octopus;gemini;finished")),
            vec![started("gemini"), Transition::Finished]
        );
    }

    #[test]
    fn four_field_marker_ignores_unknown_agent() {
        let mut d = AgentDetector::new();
        assert!(run(&mut d, &osc("777;notify;octopus;evil;attention")).is_empty());
        // 同一 chunk里已知 agent 仍生效
        assert_eq!(
            run(&mut d, &osc("777;notify;octopus;codex;attention")),
            vec![started("codex"), Transition::Attention]
        );
    }

    #[test]
    fn generic_osc777_and_osc9_attention_only_when_armed() {
        let mut d = AgentDetector::new();
        assert!(run(&mut d, &osc("777;notify;Other;ready")).is_empty());
        run(&mut d, &osc("133;C;codex"));
        assert_eq!(
            run(&mut d, &osc("777;notify;Codex;ready")),
            vec![Transition::Attention]
        );
        assert_eq!(
            run(&mut d, &osc("9;needs you")),
            vec![Transition::Attention]
        );
        // OSC 9;4 是 taskbar 进度，不是 attention
        assert!(run(&mut d, &osc("9;4;1;50")).is_empty());
    }

    #[test]
    fn exits_on_133d() {
        let mut d = AgentDetector::new();
        run(&mut d, &osc("133;C;claude"));
        assert_eq!(run(&mut d, &osc("133;D;0")), vec![Transition::Exited]);
        assert!(run(&mut d, &osc("133;D;0")).is_empty());
    }

    #[test]
    fn bel_terminator_inside_osc_is_not_attention() {
        let mut d = AgentDetector::new();
        run(&mut d, &osc("133;C;claude"));
        let mut seq = vec![ESC, OSC_INTRO];
        seq.extend_from_slice(b"0;set title");
        seq.push(BEL);
        assert!(run(&mut d, &seq).is_empty());
    }

    #[test]
    fn started_split_across_chunks() {
        let mut d = AgentDetector::new();
        assert!(run(&mut d, &[ESC, OSC_INTRO]).is_empty());
        assert!(run(&mut d, b"133;C;cla").is_empty());
        let mut out = run(&mut d, b"ude");
        out.extend(run(&mut d, &[ESC, ST_FINAL]));
        assert_eq!(out, vec![started("claude")]);
    }

    #[test]
    fn finish_reports_exited_when_armed() {
        let mut d = AgentDetector::new();
        run(&mut d, &osc("133;C;claude"));
        let mut out = Vec::new();
        d.finish(|t| out.push(t));
        assert_eq!(out, vec![Transition::Exited]);
        // 二次 finish 不再发
        let mut out2 = Vec::new();
        d.finish(|t| out2.push(t));
        assert!(out2.is_empty());
    }

    #[test]
    fn oversized_osc_does_not_panic() {
        let mut d = AgentDetector::new();
        run(&mut d, &osc("133;C;claude"));
        let mut seq = vec![ESC, OSC_INTRO];
        seq.extend(std::iter::repeat_n(b'x', OSC_MAX + 100));
        seq.extend_from_slice(&[ESC, ST_FINAL]);
        assert!(run(&mut d, &seq).is_empty());
        // 状态机恢复后正常工作
        assert_eq!(
            run(&mut d, &osc("777;notify;octopus;attention")),
            vec![Transition::Attention]
        );
    }

    #[test]
    fn into_signal_serializes_correctly() {
        let sig = Transition::Started {
            agent: "claude".into(),
        }
        .into_signal(42);
        assert_eq!(sig.id, 42);
        assert_eq!(sig.kind, "started");
        assert_eq!(sig.agent.as_deref(), Some("claude"));
    }
}
