// ActionBar 浮窗类型定义。从 index.tsx 拆出（2026-07-30）。

export type ContextKind = "text" | "files";

export type AppKind = 'editor' | 'terminal' | 'browser' | 'chat' | 'unknown';

export interface AppSource {
  bundleId?: string;
  name: string;
  kind: AppKind;
}

export interface SurroundingText {
  before?: string;
  after?: string;
  windowTitle?: string;
}

export interface Context {
  kind: ContextKind;
  text: string;
  files: string[];
  /** Cmd+C 同窗口读到的 HTML flavor（浏览器复制才有）；与后端 ActionBarContext.html camelCase 对应 */
  html?: string | null;
  source?: AppSource;
  surrounding?: SurroundingText;
}

export interface ActionBarItem {
  id: number;
  parentId: number | null;
  title: string;
  icon: string;
  actionType: string;
  actionData: string;
  sortOrder: number;
  isSystem: boolean;
  isEnabled: boolean;
  agent?: string;
  accepts?: string;
  needVoice?: boolean;
  /** JSON 数组字符串 ["com.apple.Safari"]，空串/undefined=全局项（所有 app 显示） */
  appBundleIds?: string;
}
