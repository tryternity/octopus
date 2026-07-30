// ActionBar 类型定义。从 ActionBarPanel.tsx 拆出（2026-07-30）。

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
  isAsync?: boolean;
  writeOutputToClipboard?: boolean;
  shortcut?: string;
  agent?: string;
  accepts?: string;
  triggerKeyword?: string;
  globalShortcut?: string;
  needVoice?: boolean;
  /** JSON 数组字符串 ["com.apple.Safari"]，空串/undefined=全局项 */
  appBundleIds?: string;
}

export interface ImportResult {
  name: string;
  sourcePath: string;
  dirName: string;
  isAsync: boolean;
  writeOutputToClipboard: boolean;
}

export interface ScriptRun {
  id: number;
  itemId: number;
  itemTitle: string | null;
  scriptType: string;
  exitCode: number | null;
  stdout: string;
  stderr: string;
  errorMsg: string;
  startedAt: string;
  finishedAt: string | null;
  durationMs: number | null;
}
