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
  surface: string;
  "tool-icon": string;
  "icon-filter": string;
}

export interface ThemeInfo {
  id: string;
  name: string;
  description: string;
  blur: boolean;
  colors: ThemeColors;
}

const CACHE_KEY = "octopus-theme-id";

/** 内置主题 id——颜色值已在 index.css [data-theme="xxx"] 预编译。 */
const BUILTIN_IDS = new Set(["light", "glass-dark", "nord"]);

/**
 * 应用主题：内置主题只需设 <html data-theme="xxx">（CSS 预编译，零 var() 开销）。
 * 自定义主题（~/.octopus/themes/*.json）需 JS 注入 CSS 变量作为 fallback。
 */
export async function applyThemeById(themeId: string) {
  if (BUILTIN_IDS.has(themeId)) {
    document.documentElement.setAttribute("data-theme", themeId);
  } else {
    // 自定义主题：从 list_themes 查颜色，注入 <style> 标签
    const themes = await invoke<ThemeInfo[]>("list_themes");
    const theme = themes.find((t) => t.id === themeId);
    if (theme) {
      injectCustomTheme(theme);
      document.documentElement.setAttribute("data-theme", themeId);
    }
  }
  try {
    localStorage.setItem(CACHE_KEY, themeId);
  } catch {}
}

/** 自定义主题 fallback：注入 <style> 标签覆盖 CSS 变量。 */
function injectCustomTheme(theme: ThemeInfo) {
  const rules: string[] = [];
  (Object.entries(theme.colors) as [string, string][]).forEach(([key, value]) => {
    const cssVar = key === "icon-filter" ? "--icon-filter" : `--color-${key}`;
    rules.push(`${cssVar}: ${value};`);
  });
  let styleEl = document.getElementById("octopus-custom-theme") as HTMLStyleElement | null;
  if (!styleEl) {
    styleEl = document.createElement("style");
    styleEl.id = "octopus-custom-theme";
    document.head.appendChild(styleEl);
  }
  styleEl.textContent = `[data-theme="${theme.id}"] {\n  ${rules.join("\n  ")}\n}`;
}

/**
 * 从 localStorage 同步恢复主题 id——零 IPC 调用。
 * 只需读一个字符串 + 设一个属性，微秒级。
 */
export function restoreCachedTheme() {
  try {
    const themeId = localStorage.getItem(CACHE_KEY);
    if (themeId) {
      document.documentElement.setAttribute("data-theme", themeId);
    }
  } catch {}
}

/**
 * 异步从后端读取当前主题 id 并应用。
 * list_themes 进程内缓存（OnceLock），get_theme_id 只读 DB 单键。
 */
export async function applyThemeFromConfig() {
  try {
    const themeId = await invoke<string>("get_theme_id");
    await applyThemeById(themeId);
  } catch (e) {
    console.error("applyThemeFromConfig failed:", e);
  }
}
