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
const CUSTOM_CSS_KEY = "octopus-custom-theme-css";

/** 内置主题 id——颜色值已在 index.css [data-theme="xxx"] 预编译。 */
const BUILTIN_IDS = new Set(["light", "glass-dark", "nord"]);

/** 主题列表前端缓存——避免每次 applyThemeById 都跨进程 IPC。 */
let themeCache: ThemeInfo[] | null = null;

/**
 * 应用主题：内置主题只需设 <html data-theme="xxx">（CSS 预编译，零 var() 开销）。
 * 自定义主题（~/.octopus/themes/*.json）需 JS 注入 CSS 变量作为 fallback。
 */
export async function applyThemeById(themeId: string) {
  // 脏检查：值相同直接拦截，避免触发浏览器全局 style recalc
  const current = document.documentElement.getAttribute("data-theme");
  if (current === themeId) return;

  if (BUILTIN_IDS.has(themeId)) {
    // 切回内置主题时清除自定义主题 style 标签
    const existing = document.getElementById("octopus-custom-theme");
    if (existing) existing.remove();
    document.documentElement.setAttribute("data-theme", themeId);
  } else {
    // 自定义主题：从缓存查颜色，注入 <style> 标签
    if (!themeCache) {
      themeCache = await invoke<ThemeInfo[]>("list_themes");
    }
    const theme = themeCache.find((t) => t.id === themeId);
    if (theme) {
      const css = buildCustomThemeCss(theme);
      injectCustomThemeCss(css);
      document.documentElement.setAttribute("data-theme", themeId);
    }
  }
  try {
    localStorage.setItem(CACHE_KEY, themeId);
  } catch {}
}

/** 构造自定义主题的 CSS 规则字符串。 */
function buildCustomThemeCss(theme: ThemeInfo): string {
  const rules: string[] = [];
  (Object.entries(theme.colors) as [string, string][]).forEach(([key, value]) => {
    const cssVar = key === "icon-filter" ? "--icon-filter" : `--color-${key}`;
    rules.push(`${cssVar}: ${value};`);
  });
  return `[data-theme="${theme.id}"] {\n  ${rules.join("\n  ")}\n}`;
}

/** 注入自定义主题 CSS 到 <style> 标签 + 缓存到 localStorage（供 index.html 同步恢复）。 */
function injectCustomThemeCss(css: string) {
  let styleEl = document.getElementById("octopus-custom-theme") as HTMLStyleElement | null;
  if (styleEl) {
    // 脏检查：CSS 内容相同则跳过，避免重复解析
    if (styleEl.textContent === css) return;
  } else {
    styleEl = document.createElement("style");
    styleEl.id = "octopus-custom-theme";
    document.head.appendChild(styleEl);
  }
  styleEl.textContent = css;
  try {
    localStorage.setItem(CUSTOM_CSS_KEY, css);
  } catch {}
}

/**
 * 从 localStorage 同步恢复主题 id——零 IPC 调用。
 * index.html 的阻断脚本已做同样的事，此函数作为 main.tsx 的兜底（index.html 脚本失败时）。
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
 * 用于校正 localStorage 缓存（用户可能改了主题）。
 * App.tsx mount 时不再无条件调用——只靠 config-changed 事件驱动。
 */
export async function applyThemeFromConfig() {
  try {
    // config-changed 可能是用户增删了自定义主题文件——清除前端缓存让 list_themes 重新拉取
    themeCache = null;
    const themeId = await invoke<string>("get_theme_id");
    await applyThemeById(themeId);
  } catch (e) {
    console.error("applyThemeFromConfig failed:", e);
  }
}
