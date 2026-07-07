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

/**
 * 应用主题：将主题颜色写入 :root 的 CSS 变量（--color-xxx），
 * Tailwind v4 的 bg-background / text-foreground / border-border 等类自动跟随。
 * blur 字段保留供未来原生窗口模糊集成使用，当前 3 套内置主题均为 false。
 */
export function applyTheme(theme: ThemeInfo) {
  const root = document.documentElement;
  (Object.entries(theme.colors) as [string, string][]).forEach(([key, value]) => {
    if (key === "icon-filter") return; // 不是颜色，单独处理
    root.style.setProperty(`--color-${key}`, value);
  });
  // icon-filter 不是颜色（CSS filter 函数），设为顶层 --icon-filter 变量。
  root.style.setProperty("--icon-filter", theme.colors["icon-filter"] ?? "none");
}

/** 读配置中的 clipboard_theme id，找到匹配的主题并应用。 */
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
