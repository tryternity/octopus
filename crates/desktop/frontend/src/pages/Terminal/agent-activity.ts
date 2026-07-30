/**
 * Agent 活动状态管理——监听 Rust emit 的 "agent://signal" 事件，
 * 按 ptyId 维护 working/attention/finished/idle 相位。
 *
 * 参考 Terax agentActivity.ts，去掉 zustand（octopus 无此依赖），
 * 改用模块级 state + subscribe 模式（与 octopus i18n 同构）。
 *
 * 信号源：Rust AgentDetector 解析 OSC 133/777/9 后 emit
 * { id, kind: "started"|"working"|"attention"|"finished"|"exited", agent? }。
 *
 * 相位语义：
 * - started/working → working（amber 脉冲）
 * - attention → attention（红色 bell）
 * - finished → finished（6s 后自动 idle，给用户看到完成反馈）
 * - exited → clear（agent 退出，移除该 pty 的所有状态）
 */

import { listen } from "@tauri-apps/api/event";

export type AgentPhase = "working" | "attention" | "finished" | "idle";

type AgentSignal = { id: number; kind: string; agent?: string | null };

type AgentActivityState = {
  phases: Record<number, AgentPhase>;
  /** ptyId → agent 名称（如 "claude"），从 started 信号学习，退出前保留以显示品牌图标。 */
  agents: Record<number, string>;
};

let state: AgentActivityState = { phases: {}, agents: {} };
const subscribers = new Set<() => void>();
const finishedTimers = new Map<number, ReturnType<typeof setTimeout>>();
let bound = false;

/** finished 相位自动回 idle 的延迟（ms）——给用户看到「完成」反馈。 */
const FINISHED_TTL_MS = 6000;

function emit() {
  for (const fn of subscribers) fn();
}

/** 订阅状态变化，返回取消订阅函数。组件 useEffect 里调。 */
export function subscribeAgentActivity(fn: () => void): () => void {
  subscribers.add(fn);
  return () => subscribers.delete(fn);
}

export function getAgentActivity(): AgentActivityState {
  return state;
}

/** 将原始检测器信号映射到相位（pure，可单测）。 */
export function phaseForSignal(
  kind: string,
): Exclude<AgentPhase, "idle"> | "exited" | null {
  switch (kind) {
    case "started":
    case "working":
      return "working";
    case "attention":
      return "attention";
    case "finished":
      return "finished";
    case "exited":
      return "exited";
    default:
      return null;
  }
}

function clearFinishedTimer(id: number) {
  const t = finishedTimers.get(id);
  if (t) {
    clearTimeout(t);
    finishedTimers.delete(id);
  }
}

function setPhase(id: number, phase: AgentPhase) {
  if (state.phases[id] === phase) return;
  state = {
    ...state,
    phases: { ...state.phases, [id]: phase },
  };
  emit();
}

function setAgent(id: number, agent: string) {
  if (state.agents[id] === agent) return;
  state = {
    ...state,
    agents: { ...state.agents, [id]: agent },
  };
  emit();
}

function clearPty(id: number) {
  if (!(id in state.phases) && !(id in state.agents)) return;
  const phases = { ...state.phases };
  const agents = { ...state.agents };
  delete phases[id];
  delete agents[id];
  state = { phases, agents };
  emit();
}

/**
 * 绑定全局 "agent://signal" listener（幂等，仅绑一次）。
 *
 * @param onExited agent 退出时的回调（用于 tab 状态清理）
 */
export function ensureAgentActivityListener(
  onExited?: (ptyId: number) => void,
): void {
  if (bound || typeof window === "undefined") return;
  bound = true;
  void listen<AgentSignal>("agent://signal", (e) => {
    const { id, agent } = e.payload;
    const action = phaseForSignal(e.payload.kind);
    if (action === null) return;
    clearFinishedTimer(id);
    if (action === "exited") {
      clearPty(id);
      onExited?.(id);
      return;
    }
    // agent 名称仅随 started 信号搭载（含 auto-arm）。
    if (agent) setAgent(id, agent);
    setPhase(id, action);
    if (action === "finished") {
      finishedTimers.set(
        id,
        setTimeout(() => {
          finishedTimers.delete(id);
          if (state.phases[id] === "finished") setPhase(id, "idle");
        }, FINISHED_TTL_MS),
      );
    }
  });
}
