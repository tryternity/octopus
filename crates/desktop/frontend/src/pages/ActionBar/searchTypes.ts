/** ActionBar 搜索功能类型定义 */

/** 搜索结果（与 Rust octopus_search::SearchResult 对齐，camelCase 序列化） */
export interface SearchResult {
  /** 结果来源："app" | "file" | "menu" | "bookmark" | "quicklink" | "calculator" | "url" | "command" */
  source: string;
  /** 标题（应用名 / 文件名 / 菜单标题） */
  title: string;
  /** 副标题（路径 / action_type 标签） */
  subtitle: string;
  /** 自定义图标（base64 data URI，如应用 icon），空=用 source 默认图标 */
  icon?: string | null;
  /** 动作类型："launch_app" | "open_file" | "menu" | "url" | "copy" | "copy_and_reveal" */
  actionType: string;
  /** 动作数据（JSON 字符串） */
  actionData: string;
  /** 匹配得分（排序用，越高越优先） */
  score: number;
}

/** 流式批次事件 payload（search_stream emit 的 `search://batch` 事件体）。
 *  后端每完成一个 Provider 即 emit 一次全局累积 top-10 的完整快照（已加权+排序+截断）；
 *  前端按 runId 校验本次会话后整体替换（非累加合并）。 */
export interface SearchBatch {
  /** 本次 search_stream 会话 ID（前端生成 crypto.randomUUID 传入） */
  runId: string;
  /** 全局累积 top-N 结果快照（已排序截断），整体替换上次结果 */
  results: SearchResult[];
}

/** Tab 标识 */
export type TabId = "all" | "apps" | "files" | "bookmarks" | "actions" | "commands";

/** Tab 定义 */
export interface TabDef {
  id: TabId;
  label: string;
  /** 快捷键字符 */
  key: string;
}

/**
 * Tab 栏定义（顺序即 Tab 循环顺序）。
 * Tab 键在 Tab 页之间循环（Tab/Shift+Tab/←→）。
 */
export const TABS: readonly TabDef[] = [
  { id: "all", label: "全部", key: "a" },
  { id: "apps", label: "应用", key: "d" },
  { id: "files", label: "文件", key: "f" },
  { id: "bookmarks", label: "书签", key: "b" },
  { id: "actions", label: "动作", key: "z" },
  { id: "commands", label: "命令", key: "c" },
] as const;

/** 展开方向 */
export type ExpandDirection = "up" | "down";

/** 焦点区域 */
export type FocusZone = "input" | "results";

/** 搜索结果行高（px）—— 实测每行渲染含 padding/baseline/border 约 49px，
 *  用于 resize effect 算窗口总高。比 CSS 理论值（py-2+badge=38）大 11px 余量，
 *  确保 resize 算的窗口高度 ≥ 列表实际高度 + 底部圆角空间，防最后一条被裁剪。
 *
 *  为何比理论值大这么多：ResultRow 内 SourceBadge(h-22) + title(text-13) 在 flex
 *  baseline 对齐时有额外行盒高度（line-height 1.5 → 文字行盒 ~20px，但 flex align-items
 *  center 取最高子项 badge 22px，再加 py-2(16px) = 38px 理论值）。实际渲染受：
 *  1. WKWebView 默认 line-height 比桌面 Chrome 高（macOS 系统字体渲染）
 *  2. flex 容器 border-box 与 content-box 计算差异
 *  3. 窗口 setSize 的逻辑像素与 webview devicePixelRatio 取整误差累积
 *  4. 外壳 rounded-[10px] 圆角占底部空间
 *  实测需 49px 才完整容纳 + 底部圆角。 */
export const RESULT_ROW_HEIGHT = 49;

/** Tab 栏高度（px） */
export const TAB_BAR_HEIGHT = 30;

/** 搜索输入框高度（px）—— resize effect 据此算窗口总高 */
export const INPUT_HEIGHT = 44;

/** 窗口宽度（px）—— resize effect 的 setSize 用此值。
 *  必须与 Rust action_bar_window.rs 的 inner_size + action_bar_commands.rs 的 WIN_W 一致，
 *  否则前端 setSize 会覆盖 Rust 创建时的宽度。 */
export const WINDOW_WIDTH = 480;

/** 菜单模式各视图的菜单条高度（px）—— resize effect 据此算窗口总高。
 *  CSS 理论值 ScrollRow(6) + IconBtn(34) = 40，但实测受 baseline 对齐 + 底部圆角
 *  影响需 +2px 余量（与 RESULT_ROW_HEIGHT 同理）。
 *  MAIN = 42（CSS 理论 40 + 2 余量）
 *  SUBMENU = MAIN(42) + 子菜单(42) + border-t(1) = 85
 *  LOADING = 50（spinner 区理论 40 + 余量） */
export const MENU_HEIGHT_MAIN = 42;
export const MENU_HEIGHT_SUBMENU = 85;
export const MENU_HEIGHT_LOADING = 50;

/** ActionBar 视图状态（菜单模式内部切换） */
export type View = "main" | "submenu" | "loading";

/** 最多可见结果数 */
export const MAX_VISIBLE_RESULTS = 10;

/** 延迟搜索防抖时间（ms） */
export const DELAYED_SEARCH_DEBOUNCE_MS = 150;

/** 延迟搜索最小查询长度 */
export const DELAYED_SEARCH_MIN_LENGTH = 2;

/** 展开方向阈值（px）——下方空间超过此值则向下展开 */
export const EXPAND_THRESHOLD_PX = 400;
