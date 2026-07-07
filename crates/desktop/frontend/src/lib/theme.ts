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
const STYLE_ID = "octopus-theme-vars";

/**
 * 应用主题：将主题颜色写入 <style> 标签的 :root 规则（非 inline style）。
 * Tailwind v4 的 bg-background / text-foreground / border-border 等类自动跟随。
 * 同时把完整 CSS 变量集缓存到 localStorage，下次窗口启动时同步恢复（零 IPC 延迟）。
 *
 * 为何用 <style> 标签而非 document.documentElement.style（inline style）：
 * 浏览器对 inline-style 自定义属性无法像 stylesheet 规则那样缓存优化，
 * 每次 style recalculation 都要重新 resolve var()，频繁滚动/拖动时掉帧。
 */
export function applyTheme(theme: ThemeInfo) {
  const rules: string[] = [];
  const cache: Record<string, string> = {};
  (Object.entries(theme.colors) as [string, string][]).forEach(([key, value]) => {
    if (key === "icon-filter") return;
    const cssVar = `--color-${key}`;
    rules.push(`${cssVar}: ${value};`);
    cache[cssVar] = value;
  });
  // icon-filter 不是颜色（CSS filter 函数），单独处理。
  rules.push(`--icon-filter: ${theme.colors["icon-filter"] ?? "none"};`);
  cache["--icon-filter"] = theme.colors["icon-filter"] ?? "none";

  // 注入 <style> 标签（替换已存在的）
  let styleEl = document.getElementById(STYLE_ID) as HTMLStyleElement | null;
  if (!styleEl) {
    styleEl = document.createElement("style");
    styleEl.id = STYLE_ID;
    document.head.appendChild(styleEl);
  }
  styleEl.textContent = `:root {\n  ${rules.join("\n  ")}\n}`;

  // 缓存快照，供下次窗口启动时同步恢复
  try {
    localStorage.setItem(CACHE_KEY, JSON.stringify(cache));
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
    const rules: string[] = [];
    for (const [key, value] of Object.entries(vars)) {
      rules.push(`${key}: ${value};`);
    }
    const styleEl = document.createElement("style");
    styleEl.id = STYLE_ID;
    styleEl.textContent = `:root {\n  ${rules.join("\n  ")}\n}`;
    document.head.appendChild(styleEl);
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
