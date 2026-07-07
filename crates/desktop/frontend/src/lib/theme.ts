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
 * blur=true 时给 body 加 theme-blur 类（配合透明窗口实现毛玻璃）。
 */
export function applyTheme(theme: ThemeInfo) {
  const root = document.documentElement;
  (Object.entries(theme.colors) as [string, string][]).forEach(([key, value]) => {
    root.style.setProperty(`--color-${key}`, value);
  });
  document.body.classList.toggle("theme-blur", theme.blur);
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
