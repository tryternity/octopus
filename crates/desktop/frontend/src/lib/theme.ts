import { invoke } from "@/lib/tauri";

export interface ThemeColors {
  background: string;
  foreground: string;
  primary: string;
  "primary-foreground": string;
  muted: string;
  "muted-foreground": string;
  accent: string;
  "accent-foreground": string;
  border: string;
  voice: string;
  /** 不透明表面色——result_window 等需要实色背景的组件用（暗色主题的 background 可能半透明）。 */
  surface: string;
  /** 工具栏图标色——result_window 工具栏按钮（暗色主题需浅色）。 */
  "tool-icon": string;
  /** 截图工具栏图标 CSS filter——暗色主题反色让黑色 SVG 可见。 */
  "icon-filter": string;
}

export interface ThemeInfo {
  id: string;
  name: string;
  description: string;
  blur: boolean;
  colors: ThemeColors;
}

const CACHE_KEY = "octopus-theme-css-vars";

/**
 * 应用主题：将主题颜色写入 :root 的 CSS 变量（--color-xxx），
 * Tailwind v4 的 bg-background / text-foreground / border-border 等类自动跟随。
 * 同时把完整 CSS 变量集缓存到 localStorage，下次窗口启动时同步恢复（零 IPC 延迟）。
 */
export function applyTheme(theme: ThemeInfo) {
  const root = document.documentElement;
  const vars: Record<string, string> = {};
  (Object.entries(theme.colors) as [string, string][]).forEach(([key, value]) => {
    if (key === "icon-filter") return; // 不是颜色，单独处理
    const cssVar = `--color-${key}`;
    root.style.setProperty(cssVar, value);
    vars[cssVar] = value;
  });
  // icon-filter 不是颜色（CSS filter 函数），设为顶层 --icon-filter 变量。
  root.style.setProperty("--icon-filter", theme.colors["icon-filter"] ?? "none");
  vars["--icon-filter"] = theme.colors["icon-filter"] ?? "none";
  // 缓存快照，供下次窗口启动时同步恢复
  try {
    localStorage.setItem(CACHE_KEY, JSON.stringify(vars));
  } catch {}
}

/**
 * 从 localStorage 快照同步恢复 CSS 变量——零 IPC 调用，窗口启动时立即执行，
 * 消除"先默认 light 再闪到暗色"的延迟。在 main.tsx render 前调用。
 */
export function restoreCachedTheme() {
  try {
    const cached = localStorage.getItem(CACHE_KEY);
    if (!cached) return;
    const vars = JSON.parse(cached) as Record<string, string>;
    const root = document.documentElement;
    for (const [key, value] of Object.entries(vars)) {
      root.style.setProperty(key, value);
    }
  } catch {}
}

/**
 * 异步从后端读取配置并应用主题。用于校正 localStorage 缓存（用户可能改了主题或加了新主题文件）。
 * 与 restoreCachedTheme 配合：启动时先同步恢复，再异步校正。
 */
export async function applyThemeFromConfig() {
  try {
    const [themes, configResp] = await Promise.all([
      invoke<ThemeInfo[]>("list_themes"),
      invoke<{ config: Record<string, string | number | boolean> }>("get_config"),
    ]);
    const themeId = (configResp.config.clipboard_theme as string) || "light";
    const theme = themes.find((t) => t.id === themeId) ?? themes.find((t) => t.id === "light");
    if (theme) applyTheme(theme);
  } catch (e) {
    console.error("applyThemeFromConfig failed:", e);
  }
}
