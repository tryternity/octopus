/** ActionBar 搜索功能类型定义 */

/** 搜索结果（与 Rust octopus_search::SearchResult 对齐，camelCase 序列化） */
export interface SearchResult {
  /** 结果来源："app" | "file" | "menu" | "bookmark" | "quicklink" | "shell" */
  source: string;
  /** 标题（应用名 / 文件名 / 菜单标题） */
  title: string;
  /** 副标题（路径 / action_type 标签） */
  subtitle: string;
  /** 自定义图标（base64 data URI，如应用 icon），空=用 source 默认图标 */
  icon?: string | null;
  /** 动作类型："launch_app" | "open_file" | "menu" | "url" | "shell" */
  actionType: string;
  /** 动作数据（JSON 字符串） */
  actionData: string;
  /** 匹配得分（排序用，越高越优先） */
  score: number;
}

/** Tab 标识 */
export type TabId = "all" | "apps" | "files" | "shell" | "bookmarks";

/** Tab 定义 */
export interface TabDef {
  id: TabId;
  label: string;
  /** 快捷键字符 */
  key: string;
}

/**
 * Tab 栏定义（顺序即 Tab 循环顺序）。
 * Tab 键在 Tab 页之间循环；Cmd+字母 快捷定位。
 */
export const TABS: readonly TabDef[] = [
  { id: "all", label: "全部", key: "a" },
  { id: "apps", label: "应用", key: "d" },
  { id: "files", label: "文件", key: "f" },
  { id: "shell", label: "Shell", key: "s" },
  { id: "bookmarks", label: "书签", key: "b" },
] as const;

/** 展开方向 */
export type ExpandDirection = "up" | "down";

/** 焦点区域 */
export type FocusZone = "input" | "results";

/** 搜索结果行高（px） */
export const RESULT_ROW_HEIGHT = 36;

/** Tab 栏高度（px） */
export const TAB_BAR_HEIGHT = 30;

/** 搜索输入框高度（px） */
export const INPUT_HEIGHT = 36;

/** 菜单模式各视图的菜单条高度（px）—— resize effect 据此算窗口总高 */
export const MENU_HEIGHT_MAIN = 40;
export const MENU_HEIGHT_SUBMENU = 78;
export const MENU_HEIGHT_LOADING = 48;

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
