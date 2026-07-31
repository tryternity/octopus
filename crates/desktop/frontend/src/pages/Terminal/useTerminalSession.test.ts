/**
 * attachWebgl 降级路径 + context loss 恢复单测。
 *
 * WebGL addon 依赖真实 DOM + GPU，无法测实际渲染。通过 attachWebgl 的
 * factory 参数注入 mock WebglAddon，验证关键不变量：
 * - 构造抛错 → 返回 null（降级 Canvas），不抛
 * - 构造成功 → 返回 addon 实例 + onContextLoss 回调注册
 * - context loss 触发 → dispose + 250ms 后重连
 */
import { describe, it, expect, vi, beforeEach } from "vitest";
import { createRef } from "react";
import type { WebglAddon } from "@xterm/addon-webgl";

import { attachWebgl } from "./useTerminalSession";

/** 最小 term mock（满足 attachWebgl 需要的 loadAddon/refresh/rows）。 */
function makeTermMock() {
  return {
    loadAddon: vi.fn(),
    refresh: vi.fn(),
    rows: 24,
  } as unknown as Parameters<typeof attachWebgl>[0];
}

/** WebglAddon 实例 mock（onContextLoss 捕获回调 + dispose + triggerLoss 测试触发器）。 */
type WebglMock = WebglAddon & { triggerLoss: () => void };
function makeWebglMock(): WebglMock {
  let lossCb: (() => void) | null = null;
  return {
    onContextLoss: vi.fn((cb: () => void) => {
      lossCb = cb;
    }),
    dispose: vi.fn(),
    triggerLoss: () => lossCb?.(),
  } as unknown as WebglMock;
}

describe("attachWebgl", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.clearAllTimers();
  });

  it("构造成功 → 返回 addon 实例 + loadAddon + onContextLoss 注册", () => {
    const instance = makeWebglMock();
    const term = makeTermMock();
    const ref = createRef<WebglAddon | null>();

    const result = attachWebgl(term, ref, () => instance);

    expect(result).toBe(instance);
    expect(term.loadAddon).toHaveBeenCalledWith(instance);
    expect(instance.onContextLoss).toHaveBeenCalled();
  });

  it("构造抛错 → 返回 null（降级 Canvas），不抛", () => {
    const term = makeTermMock();
    const ref = createRef<WebglAddon | null>();

    // factory 抛错模拟 GPU 不可用——不应传播，降级返回 null
    const result = attachWebgl(term, ref, () => {
      throw new Error("WebGL not supported");
    });

    expect(result).toBeNull();
    expect(term.loadAddon).not.toHaveBeenCalled();
  });

  it("context loss → dispose + ref 清空 + 250ms 后重连 refresh", () => {
    vi.useFakeTimers();
    const first = makeWebglMock();
    const reattached = makeWebglMock();
    let callCount = 0;
    const factory = vi.fn(() => {
      callCount += 1;
      return callCount === 1 ? first : reattached;
    });
    const term = makeTermMock();
    const ref = createRef<WebglAddon | null>();

    attachWebgl(term, ref, factory);
    ref.current = first;

    // 触发 context loss（fake timer 下注册 setTimeout）
    first.triggerLoss();

    // 立即：dispose 被调 + ref 清空（释放丢的 context）
    expect(first.dispose).toHaveBeenCalled();
    expect(ref.current).toBeNull();
    // 250ms 前：factory 只被调 1 次（首次 attach）
    expect(factory).toHaveBeenCalledTimes(1);

    // 推进 250ms：重连发生（第 2 次 factory 调用）
    vi.advanceTimersByTime(250);

    expect(factory).toHaveBeenCalledTimes(2);
    expect(ref.current).toBe(reattached);
    // 重连后 refresh 重绘（0 到 rows-1）
    expect(term.refresh).toHaveBeenCalledWith(0, 23);
    vi.useRealTimers();
  });

  it("context loss 重连时若已有 addon（被其他路径 attach）→ 不重复 attach", () => {
    vi.useFakeTimers();
    const first = makeWebglMock();
    const factory = vi.fn(() => first);
    const term = makeTermMock();
    const ref = createRef<WebglAddon | null>();

    attachWebgl(term, ref, factory);
    ref.current = first;

    first.triggerLoss();
    // 在 250ms 延迟期间，外部（如 active 切换）已 attach 了新 addon
    const external = makeWebglMock();
    ref.current = external;

    vi.advanceTimersByTime(250);

    // 不应重连（ref 已有 addon）
    expect(factory).toHaveBeenCalledTimes(1);
    vi.useRealTimers();
  });
});
