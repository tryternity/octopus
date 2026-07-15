import { describe, it, expect } from "vitest";
import {
  determineExpandDirection,
  getTabByKey,
  getNextTab,
  getTabIndex,
  shouldTriggerDelayedSearch,
  isShellMode,
  extractShellCommand,
  mergeResults,
  filterByTab,
  parseActionData,
  calcResultsHeight,
  calcPanelHeight,
  clampSelectedIndex,
  navigateResults,
  hasQuery,
} from "./searchLogic";
import type { SearchResult } from "./searchTypes";
import { TABS } from "./searchTypes";

// ── 工厂辅助 ──

function makeResult(
  source: string,
  title: string,
  score = 0,
  actionType = "menu",
): SearchResult {
  return {
    source,
    title,
    subtitle: "",
    actionType,
    actionData: "{}",
    score,
  };
}

// ── determineExpandDirection ──

describe("determineExpandDirection", () => {
  it("下方空间 > 阈值 → 向下展开", () => {
    expect(determineExpandDirection(100, 1000, 400)).toBe("down");
    expect(determineExpandDirection(500, 1000, 400)).toBe("down"); // 500px below
  });

  it("下方空间 = 阈值 → 向上展开（严格大于才向下）", () => {
    expect(determineExpandDirection(600, 1000, 400)).toBe("up"); // exactly 400
  });

  it("下方空间 < 阈值 → 向上展开", () => {
    expect(determineExpandDirection(700, 1000, 400)).toBe("up");
    expect(determineExpandDirection(950, 1000, 400)).toBe("up");
  });

  it("使用默认阈值 400", () => {
    expect(determineExpandDirection(500, 1000)).toBe("down"); // 500 > 400
    expect(determineExpandDirection(700, 1000)).toBe("up"); // 300 < 400
  });

  it("输入框在屏幕顶部 → 向下展开", () => {
    expect(determineExpandDirection(0, 900)).toBe("down");
  });

  it("输入框在屏幕底部 → 向上展开", () => {
    expect(determineExpandDirection(890, 900)).toBe("up");
  });
});

// ── getTabByKey ──

describe("getTabByKey", () => {
  it("正确映射快捷键字符 → TabId", () => {
    expect(getTabByKey("?")).toBe("all");
    expect(getTabByKey("a")).toBe("apps");
    expect(getTabByKey("f")).toBe("files");
    expect(getTabByKey(">")).toBe("shell");
    expect(getTabByKey("b")).toBe("bookmarks");
  });

  it("无匹配返回 null", () => {
    expect(getTabByKey("x")).toBeNull();
    expect(getTabByKey("")).toBeNull();
    expect(getTabByKey("A")).toBeNull(); // 大写不匹配
    expect(getTabByKey(" ")).toBeNull();
  });
});

// ── getNextTab ──

describe("getNextTab", () => {
  it("正向循环 all → apps → files → shell → bookmarks → all", () => {
    expect(getNextTab("all", 1)).toBe("apps");
    expect(getNextTab("apps", 1)).toBe("files");
    expect(getNextTab("files", 1)).toBe("shell");
    expect(getNextTab("shell", 1)).toBe("bookmarks");
    expect(getNextTab("bookmarks", 1)).toBe("all");
  });

  it("反向循环 all → bookmarks → shell → files → apps → all", () => {
    expect(getNextTab("all", -1)).toBe("bookmarks");
    expect(getNextTab("bookmarks", -1)).toBe("shell");
    expect(getNextTab("shell", -1)).toBe("files");
    expect(getNextTab("files", -1)).toBe("apps");
    expect(getNextTab("apps", -1)).toBe("all");
  });

  it("无效 TabId 回退到 all", () => {
    // @ts-expect-error — 测试无效输入
    expect(getNextTab("invalid", 1)).toBe("all");
  });
});

// ── getTabIndex ──

describe("getTabIndex", () => {
  it("返回 Tab 在 TABS 中的位置", () => {
    expect(getTabIndex("all")).toBe(0);
    expect(getTabIndex("apps")).toBe(1);
    expect(getTabIndex("files")).toBe(2);
    expect(getTabIndex("shell")).toBe(3);
    expect(getTabIndex("bookmarks")).toBe(4);
  });

  it("无效 Tab 返回 -1", () => {
    // @ts-expect-error — 测试无效输入
    expect(getTabIndex("invalid")).toBe(-1);
  });

  it("TABS 长度为 5", () => {
    expect(TABS.length).toBe(5);
  });
});

// ── shouldTriggerDelayedSearch ──

describe("shouldTriggerDelayedSearch", () => {
  it("≥ 2 字符 → true", () => {
    expect(shouldTriggerDelayedSearch("ab")).toBe(true);
    expect(shouldTriggerDelayedSearch("abc")).toBe(true);
    expect(shouldTriggerDelayedSearch("  ab  ")).toBe(true); // trim 后 2 字符
    expect(shouldTriggerDelayedSearch("翻译")).toBe(true); // 2 个中文字符
  });

  it("< 2 字符 → false", () => {
    expect(shouldTriggerDelayedSearch("")).toBe(false);
    expect(shouldTriggerDelayedSearch("a")).toBe(false);
    expect(shouldTriggerDelayedSearch("  ")).toBe(false);
  });
});

// ── isShellMode / extractShellCommand ──

describe("isShellMode", () => {
  it("以 > 开头 → true", () => {
    expect(isShellMode(">ls")).toBe(true);
    expect(isShellMode("> ls -la")).toBe(true);
    expect(isShellMode("  > echo hi")).toBe(true); // 前导空格
  });

  it("不以 > 开头 → false", () => {
    expect(isShellMode("ls")).toBe(false);
    expect(isShellMode("")).toBe(false);
    expect(isShellMode("echo > file")).toBe(false);
  });
});

describe("extractShellCommand", () => {
  it("正确提取命令", () => {
    expect(extractShellCommand(">ls")).toBe("ls");
    expect(extractShellCommand("> ls -la")).toBe("ls -la");
    expect(extractShellCommand("  > echo hi")).toBe("echo hi");
  });

  it("只有 > 无命令 → 空串", () => {
    expect(extractShellCommand(">")).toBe("");
    expect(extractShellCommand(">   ")).toBe("");
  });

  it("非 shell 模式也能提取（去掉首字符）", () => {
    // 非 > 开头时 slice(1) 仍然工作，但通常在 isShellMode 判定后才调用
    expect(extractShellCommand("abc")).toBe("bc");
  });
});

// ── mergeResults ──

describe("mergeResults", () => {
  it("合并不重复的结果", () => {
    const instant = [makeResult("app", "Chrome", 500)];
    const delayed = [makeResult("file", "doc.pdf", 200)];
    const merged = mergeResults(instant, delayed);
    expect(merged).toHaveLength(2);
    expect(merged[0].title).toBe("Chrome");
    expect(merged[1].title).toBe("doc.pdf");
  });

  it("即时结果优先于延迟结果（去重保留即时 score）", () => {
    const instant = [makeResult("app", "Chrome", 500)];
    const delayed = [makeResult("app", "Chrome", 300)];
    const merged = mergeResults(instant, delayed);
    expect(merged).toHaveLength(1);
    expect(merged[0].score).toBe(500); // 即时结果 score 保留
  });

  it("同 title 不同 source 不去重", () => {
    const instant = [makeResult("app", "Chrome", 500)];
    const delayed = [makeResult("file", "Chrome", 200)];
    const merged = mergeResults(instant, delayed);
    expect(merged).toHaveLength(2);
  });

  it("空输入 → 空输出", () => {
    expect(mergeResults([], [])).toEqual([]);
  });

  it("即时为空，延迟有结果", () => {
    const delayed = [makeResult("file", "doc.pdf", 200)];
    const merged = mergeResults([], delayed);
    expect(merged).toHaveLength(1);
  });

  it("合并后全局按 score 降序排序（跨来源）", () => {
    // 即时 score 低、延迟 score 高——合并后延迟应在前
    const instant = [
      makeResult("app", "Chrome", 200),
      makeResult("menu", "翻译", 150),
    ];
    const delayed = [
      makeResult("file", "doc.pdf", 8000),
      makeResult("bookmark", "GitHub", 500),
    ];
    const merged = mergeResults(instant, delayed);
    expect(merged).toHaveLength(4);
    // 8000 > 500 > 200 > 150
    expect(merged[0].title).toBe("doc.pdf");
    expect(merged[1].title).toBe("GitHub");
    expect(merged[2].title).toBe("Chrome");
    expect(merged[3].title).toBe("翻译");
  });
});

// ── filterByTab ──

describe("filterByTab", () => {
  const results: SearchResult[] = [
    makeResult("app", "Chrome"),
    makeResult("file", "doc.pdf"),
    makeResult("shell", "ls"),
    makeResult("bookmark", "GitHub"),
    makeResult("menu", "翻译"),
  ];

  it("all → 全部", () => {
    expect(filterByTab(results, "all")).toHaveLength(5);
  });

  it("apps → 仅 app 来源", () => {
    const filtered = filterByTab(results, "apps");
    expect(filtered).toHaveLength(1);
    expect(filtered[0].source).toBe("app");
  });

  it("files → 仅 file 来源", () => {
    const filtered = filterByTab(results, "files");
    expect(filtered).toHaveLength(1);
    expect(filtered[0].source).toBe("file");
  });

  it("shell → 仅 shell 来源", () => {
    const filtered = filterByTab(results, "shell");
    expect(filtered).toHaveLength(1);
    expect(filtered[0].source).toBe("shell");
  });

  it("bookmarks → 仅 bookmark 来源", () => {
    const filtered = filterByTab(results, "bookmarks");
    expect(filtered).toHaveLength(1);
    expect(filtered[0].source).toBe("bookmark");
  });

  it("空列表 → 空列表", () => {
    expect(filterByTab([], "all")).toEqual([]);
  });
});

// ── parseActionData ──

describe("parseActionData", () => {
  it("合法 JSON → 解析对象", () => {
    const data = parseActionData('{"path":"/Applications/Chrome.app"}');
    expect(data.path).toBe("/Applications/Chrome.app");
  });

  it("非法 JSON → 空对象", () => {
    expect(parseActionData("not json")).toEqual({});
    expect(parseActionData("")).toEqual({});
  });

  it("嵌套 JSON → 正确解析", () => {
    const data = parseActionData('{"command":"ls","args":["-la"]}');
    expect(data.command).toBe("ls");
    expect(data.args).toEqual(["-la"]);
  });
});

// ── calcResultsHeight ──

describe("calcResultsHeight", () => {
  it("0 结果 → 0px", () => {
    expect(calcResultsHeight(0)).toBe(0);
  });

  it("1-10 结果 → 按行高 36px 计算", () => {
    expect(calcResultsHeight(1)).toBe(36);
    expect(calcResultsHeight(5)).toBe(180);
    expect(calcResultsHeight(10)).toBe(360);
  });

  it("> 10 结果 → 截断到 10 行（360px）", () => {
    expect(calcResultsHeight(15)).toBe(360);
    expect(calcResultsHeight(100)).toBe(360);
  });

  it("负数 → 0px", () => {
    expect(calcResultsHeight(-5)).toBe(0);
  });
});

// ── calcPanelHeight ──

describe("calcPanelHeight", () => {
  it("0 结果 → 输入框 + Tab 栏高度", () => {
    // INPUT_HEIGHT(36) + TAB_BAR_HEIGHT(30) + 0 = 66
    expect(calcPanelHeight(0)).toBe(66);
  });

  it("5 结果 → 66 + 180 = 246", () => {
    expect(calcPanelHeight(5)).toBe(246);
  });

  it("10 结果 → 66 + 360 = 426", () => {
    expect(calcPanelHeight(10)).toBe(426);
  });

  it("> 10 结果 → 截断到 10 行", () => {
    expect(calcPanelHeight(20)).toBe(426);
  });
});

// ── clampSelectedIndex ──

describe("clampSelectedIndex", () => {
  it("空列表 → -1", () => {
    expect(clampSelectedIndex(0, 0)).toBe(-1);
    expect(clampSelectedIndex(5, 0)).toBe(-1);
  });

  it("有效范围内 → 原值", () => {
    expect(clampSelectedIndex(0, 5)).toBe(0);
    expect(clampSelectedIndex(3, 5)).toBe(3);
    expect(clampSelectedIndex(4, 5)).toBe(4);
  });

  it("超出上界 → clamp 到 length-1", () => {
    expect(clampSelectedIndex(5, 5)).toBe(4);
    expect(clampSelectedIndex(100, 5)).toBe(4);
  });

  it("负值 → clamp 到 0", () => {
    expect(clampSelectedIndex(-1, 5)).toBe(0);
    expect(clampSelectedIndex(-100, 5)).toBe(0);
  });
});

// ── navigateResults ──

describe("navigateResults", () => {
  it("正向导航（非末尾）", () => {
    expect(navigateResults(0, 1, 5)).toBe(1);
    expect(navigateResults(3, 1, 5)).toBe(4);
  });

  it("正向导航（末尾循环到开头）", () => {
    expect(navigateResults(4, 1, 5)).toBe(0);
  });

  it("反向导航（非开头）", () => {
    expect(navigateResults(4, -1, 5)).toBe(3);
    expect(navigateResults(1, -1, 5)).toBe(0);
  });

  it("反向导航（开头循环到末尾）", () => {
    expect(navigateResults(0, -1, 5)).toBe(4);
  });

  it("空列表 → -1", () => {
    expect(navigateResults(0, 1, 0)).toBe(-1);
  });

  it("单元素列表 → 始终 0", () => {
    expect(navigateResults(0, 1, 1)).toBe(0);
    expect(navigateResults(0, -1, 1)).toBe(0);
  });
});

// ── hasQuery ──

describe("hasQuery", () => {
  it("非空 → true", () => {
    expect(hasQuery("a")).toBe(true);
    expect(hasQuery("hello")).toBe(true);
    expect(hasQuery(" 翻译 ")).toBe(true);
  });

  it("空/纯空格 → false", () => {
    expect(hasQuery("")).toBe(false);
    expect(hasQuery("   ")).toBe(false);
    expect(hasQuery("\t\n")).toBe(false);
  });
});
