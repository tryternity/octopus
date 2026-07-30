import { describe, it, expect } from "vitest";
import {
  determineExpandDirection,
  getNextTab,
  getVisibleTabs,
  getTabIndex,
  shouldTriggerDelayedSearch,
  filterByTab,
  parseActionData,
  calcResultsHeight,
  calcPanelHeight,
  clampSelectedIndex,
  navigateResults,
  hasQuery,
  nextFocusLayerAfterExecute,
  calcMenuHeight,
  calcTotalHeight,
  isMoveKey,
  moveDirection,
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

// ── getVisibleTabs ──

describe("getVisibleTabs", () => {
  it("有选中（hasContext=true）→ 全部 7 个 Tab 含 actions", () => {
    const tabs = getVisibleTabs(true);
    expect(tabs).toHaveLength(7);
    expect(tabs.find((t) => t.id === "actions")).toBeDefined();
    expect(tabs.find((t) => t.id === "commands")).toBeDefined();
    expect(tabs.find((t) => t.id === "slash")).toBeDefined();
  });

  it("无选中（hasContext=false，launch 模式）→ 6 个 Tab，无 actions（commands/slash 不依赖 context）", () => {
    const tabs = getVisibleTabs(false);
    expect(tabs).toHaveLength(6);
    expect(tabs.find((t) => t.id === "actions")).toBeUndefined();
    expect(tabs.find((t) => t.id === "commands")).toBeDefined();
    expect(tabs.find((t) => t.id === "slash")).toBeDefined();
  });
});

// ── getNextTab ──

describe("getNextTab", () => {
  it("正向循环 all → apps → files → bookmarks → actions → commands → slash → all", () => {
    expect(getNextTab("all", 1)).toBe("apps");
    expect(getNextTab("apps", 1)).toBe("files");
    expect(getNextTab("files", 1)).toBe("bookmarks");
    expect(getNextTab("bookmarks", 1)).toBe("actions");
    expect(getNextTab("actions", 1)).toBe("commands");
    expect(getNextTab("commands", 1)).toBe("slash");
    expect(getNextTab("slash", 1)).toBe("all");
  });

  it("反向循环 all → slash → commands → actions → bookmarks → files → apps → all", () => {
    expect(getNextTab("all", -1)).toBe("slash");
    expect(getNextTab("slash", -1)).toBe("commands");
    expect(getNextTab("commands", -1)).toBe("actions");
    expect(getNextTab("actions", -1)).toBe("bookmarks");
    expect(getNextTab("bookmarks", -1)).toBe("files");
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
    expect(getTabIndex("bookmarks")).toBe(3);
    expect(getTabIndex("actions")).toBe(4);
    expect(getTabIndex("commands")).toBe(5);
    expect(getTabIndex("slash")).toBe(6);
  });

  it("无效 Tab 返回 -1", () => {
    // @ts-expect-error — 测试无效输入
    expect(getTabIndex("invalid")).toBe(-1);
  });

  it("TABS 长度为 7", () => {
    expect(TABS.length).toBe(7);
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

// ── mergeResults ──
// (已删除) mergeResults 函数已随 delayedResults 死状态一并移除——流式后端 emit
// 累积 top-N 快照，前端整体替换，不再需要即时/延迟两路合并。

// ── filterByTab ──

describe("filterByTab", () => {
  const results: SearchResult[] = [
    makeResult("app", "Chrome"),
    makeResult("file", "doc.pdf"),
    makeResult("bookmark", "GitHub"),
    makeResult("menu", "翻译"),
    makeResult("command", "git status"),
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

  it("bookmarks → 仅 bookmark 来源", () => {
    const filtered = filterByTab(results, "bookmarks");
    expect(filtered).toHaveLength(1);
    expect(filtered[0].source).toBe("bookmark");
  });

  it("actions → 仅 menu 来源（不含 quicklink/app/file 等）", () => {
    const filtered = filterByTab(results, "actions");
    expect(filtered).toHaveLength(1);
    expect(filtered[0].source).toBe("menu");
  });

  it("commands → 仅 command 来源", () => {
    const filtered = filterByTab(results, "commands");
    expect(filtered).toHaveLength(1);
    expect(filtered[0].source).toBe("command");
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
  it("0 结果 → 1 行兜底（RESULT_ROW_HEIGHT）", () => {
    expect(calcResultsHeight(0)).toBe(49);
  });

  it("1-10 结果 → 按行高 49px 计算", () => {
    expect(calcResultsHeight(1)).toBe(49);
    expect(calcResultsHeight(5)).toBe(245);
    expect(calcResultsHeight(10)).toBe(490);
  });

  it("> 10 结果 → 截断到 10 行（490px）", () => {
    expect(calcResultsHeight(15)).toBe(490);
    expect(calcResultsHeight(100)).toBe(490);
  });

  it("负数 → 1 行兜底", () => {
    expect(calcResultsHeight(-5)).toBe(49);
  });
});

// ── calcPanelHeight ──

describe("calcPanelHeight", () => {
  it("0 结果 → 输入框 + Tab 栏 + 1 行（calcResultsHeight(0)=RESULT_ROW_HEIGHT）", () => {
    // INPUT_HEIGHT(44) + TAB_BAR_HEIGHT(30) + calcResultsHeight(0)=49 = 123
    expect(calcPanelHeight(0)).toBe(123);
  });

  it("5 结果 → 44 + 30 + 245 = 319", () => {
    expect(calcPanelHeight(5)).toBe(319);
  });

  it("10 结果 → 44 + 30 + 490 = 564", () => {
    expect(calcPanelHeight(10)).toBe(564);
  });

  it("> 10 结果 → 截断到 10 行", () => {
    expect(calcPanelHeight(20)).toBe(564);
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

// ── nextFocusLayerAfterExecute ──

describe("nextFocusLayerAfterExecute", () => {
  it("submenu 类型 → sub（终结性展开，Enter 应执行子项）", () => {
    expect(nextFocusLayerAfterExecute("submenu", "main")).toBe("sub");
    expect(nextFocusLayerAfterExecute("submenu", "sub")).toBe("sub");
  });

  it("非 submenu 类型 → 保持当前层（executeItem 不改变焦点层）", () => {
    expect(nextFocusLayerAfterExecute("ai", "main")).toBe("main");
    expect(nextFocusLayerAfterExecute("script", "main")).toBe("main");
    expect(nextFocusLayerAfterExecute("url", "main")).toBe("main");
    expect(nextFocusLayerAfterExecute("copy", "sub")).toBe("sub");
  });

  it("回归 S3：executeItem 展开 submenu 后 focusLayer 必须为 sub", () => {
    // 这是 S3 bug 的核心不变量——executeItem(submenu) 后 Enter 走 sub 分支执行子项
    const layer = nextFocusLayerAfterExecute("submenu", "main");
    expect(layer).toBe("sub");
  });
});

// ── calcMenuHeight / calcTotalHeight ──

describe("calcMenuHeight", () => {
  it("无选中（hasContext=false）→ 0（仅搜索框）", () => {
    expect(calcMenuHeight(false, "main")).toBe(0);
    expect(calcMenuHeight(false, "submenu")).toBe(0);
    expect(calcMenuHeight(false, "loading")).toBe(0);
  });

  it("有选中 + view=main → MENU_HEIGHT_MAIN (42)", () => {
    expect(calcMenuHeight(true, "main")).toBe(42);
  });

  it("有选中 + view=submenu → MENU_HEIGHT_SUBMENU (85)", () => {
    expect(calcMenuHeight(true, "submenu")).toBe(85);
  });

  it("有选中 + view=loading → MENU_HEIGHT_LOADING (50)", () => {
    expect(calcMenuHeight(true, "loading")).toBe(50);
  });

  it("回归：有选中时菜单条高度必须 > 0（防 resize 裁剪菜单）", () => {
    // 核心不变量——context 非 null 时菜单条必须有高度，
    // 否则窗口裁剪菜单条（"首次有选中只显示搜索框"回归）
    for (const view of ["main", "submenu", "loading"] as const) {
      expect(calcMenuHeight(true, view)).toBeGreaterThan(0);
    }
  });
});

describe("calcTotalHeight", () => {
  it("无选中菜单模式 = 输入框 + 0", () => {
    expect(calcTotalHeight(false, false, "main", 0)).toBe(44);
  });

  it("有选中菜单模式 main = 输入框 + MENU_HEIGHT_MAIN", () => {
    expect(calcTotalHeight(false, true, "main", 0)).toBe(44 + 42);
  });

  it("有选中菜单模式 submenu = 输入框 + MENU_HEIGHT_SUBMENU", () => {
    expect(calcTotalHeight(false, true, "submenu", 0)).toBe(44 + 85);
  });

  it("搜索模式 = 输入框 + Tab栏 + 结果区（不受 context/view 影响）", () => {
    // 搜索模式高度只依赖结果数，与 context 无关
    const withContext = calcTotalHeight(true, true, "main", 5);
    const noContext = calcTotalHeight(true, false, "main", 5);
    expect(withContext).toBe(noContext);
    expect(withContext).toBe(44 + 30 + calcResultsHeight(5));
  });
});

// ── isMoveKey / moveDirection ──

describe("isMoveKey", () => {
  it("Tab 始终是移动键（不受 ARROW_AS_TAB 影响）", () => {
    expect(isMoveKey("Tab", true)).toBe(true);
    expect(isMoveKey("Tab", false)).toBe(true);
  });

  it("ARROW_AS_TAB=true 时 ←/→ 是移动键", () => {
    expect(isMoveKey("ArrowLeft", true)).toBe(true);
    expect(isMoveKey("ArrowRight", true)).toBe(true);
  });

  it("ARROW_AS_TAB=false 时 ←/→ 不是移动键", () => {
    expect(isMoveKey("ArrowLeft", false)).toBe(false);
    expect(isMoveKey("ArrowRight", false)).toBe(false);
  });

  it("其他键（字母/数字/↑↓/Enter）不是移动键", () => {
    expect(isMoveKey("a", true)).toBe(false);
    expect(isMoveKey("ArrowUp", true)).toBe(false);
    expect(isMoveKey("ArrowDown", true)).toBe(false);
    expect(isMoveKey("Enter", true)).toBe(false);
    expect(isMoveKey(" ", true)).toBe(false);
  });
});

describe("moveDirection", () => {
  it("Tab → 正向（shiftKey=false）", () => {
    expect(moveDirection("Tab", false, true)).toBe(true);
    expect(moveDirection("Tab", false, false)).toBe(true);
  });

  it("Shift+Tab → 反向（shiftKey=true）", () => {
    expect(moveDirection("Tab", true, true)).toBe(false);
    expect(moveDirection("Tab", true, false)).toBe(false);
  });

  it("ARROW_AS_TAB=true：→ 正向，← 反向", () => {
    expect(moveDirection("ArrowRight", false, true)).toBe(true);
    expect(moveDirection("ArrowLeft", false, true)).toBe(false);
    // shiftKey 不影响左右键方向（← 永远反向，→ 永远正向）
    expect(moveDirection("ArrowRight", true, true)).toBe(true);
    expect(moveDirection("ArrowLeft", true, true)).toBe(false);
  });

  it("ARROW_AS_TAB=false：←/→ 不是移动键 → null", () => {
    expect(moveDirection("ArrowRight", false, false)).toBeNull();
    expect(moveDirection("ArrowLeft", false, false)).toBeNull();
  });

  it("非移动键 → null", () => {
    expect(moveDirection("a", false, true)).toBeNull();
    expect(moveDirection("Enter", false, true)).toBeNull();
    expect(moveDirection("ArrowUp", false, true)).toBeNull();
  });
});

// ── slash tab ──

describe("slash tab", () => {
  const slashResult: SearchResult = {
    source: "slash", title: "/google", subtitle: "Google",
    icon: null, actionType: "url", actionData: "{}", score: 100,
  };
  const appResult: SearchResult = {
    source: "app", title: "Chrome", subtitle: "",
    icon: null, actionType: "launch_app", actionData: "{}", score: 100,
  };

  it("filterByTab slash 只留 source=slash", () => {
    const filtered = filterByTab([slashResult, appResult], "slash");
    expect(filtered).toEqual([slashResult]);
  });

  it("getVisibleTabs 含 slash（无 context 也含）", () => {
    const tabs = getVisibleTabs(false);
    expect(tabs.find((t) => t.id === "slash")).toBeTruthy();
  });

  it("getNextTab 循环含 slash", () => {
    // 从某 tab 循环应能经过 slash（具体起点取决于 TABS 顺序，此处验证不报错 + 能到达）
    const tabs = getVisibleTabs(true);
    const slashIdx = tabs.findIndex((t) => t.id === "slash");
    expect(slashIdx).toBeGreaterThanOrEqual(0);
  });
});
