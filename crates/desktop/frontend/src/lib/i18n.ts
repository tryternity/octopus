import { useState, useEffect, useCallback } from "react";
import { invoke, listen } from "@/lib/tauri";
import zhCN from "@/locales/zh-CN.yaml";
import en from "@/locales/en.yaml";

type Locale = "zh-CN" | "en";

/** 递归拍平嵌套对象为 flat dotted keys */
function flatten(obj: Record<string, unknown>, prefix = ""): Record<string, string> {
  const result: Record<string, string> = {};
  for (const [key, val] of Object.entries(obj)) {
    const fullKey = prefix ? `${prefix}.${key}` : key;
    if (typeof val === "string") {
      result[fullKey] = val;
    } else if (typeof val === "object" && val !== null) {
      Object.assign(result, flatten(val as Record<string, unknown>, fullKey));
    }
  }
  return result;
}

const DICTS: Record<Locale, Record<string, string>> = {
  "zh-CN": flatten(zhCN as Record<string, unknown>),
  "en": flatten(en as Record<string, unknown>),
};

const LOCALE_CACHE_KEY = "octopus-locale";

/**
 * 模块加载时同步从 localStorage 恢复 locale（零 IPC）。
 *
 * 与 lib/theme.ts 的 restoreCachedTheme 同范式：避免 main.tsx 等 get_config IPC
 * resolve 才 render 导致截图窗口白屏 ~10-50ms。后台 initI18n 的 IPC 仅做 DB
 * 校正——与缓存不一致时才触发 setLocale 重渲染。
 */
let currentLocale: Locale = (() => {
  try {
    const cached = localStorage.getItem(LOCALE_CACHE_KEY);
    if (cached === "en" || cached === "zh-CN") return cached;
  } catch {
    // localStorage 不可用时用默认值
  }
  return "zh-CN";
})();
const listeners = new Set<() => void>();

function translate(key: string, params?: Record<string, string | number>): string {
  const dict = DICTS[currentLocale];
  let str = dict[key] ?? key;
  if (params) {
    for (const [k, v] of Object.entries(params)) {
      str = str.replace(new RegExp(`\\$\\{${k}\\}`, "g"), String(v));
    }
  }
  return str;
}

function localeFromConfig(v?: string): Locale {
  return v === "en" ? "en" : "zh-CN";
}

/**
 * 从后端 config 读 ui_language 校正 locale（DB 改了语言时同步到前端）+ 监听跨窗口事件。
 *
 * 不阻塞渲染：main.tsx 应先 render，再后台调用本函数。locale 已在模块加载时从
 * localStorage 同步恢复，此函数仅做 DB→前端校正。
 */
export async function initI18n(): Promise<void> {
  try {
    const resp = await invoke<{ config: Record<string, unknown> }>("get_config");
    const uiLang = resp.config?.ui_language as string | undefined;
    setLocale(localeFromConfig(uiLang));
  } catch {
    // 后端未就绪时保持 localStorage 缓存值
  }
  // 监听跨窗口语言切换：Settings 改语言后 emit("locale-changed")，
  // 每个窗口的 initI18n 独立监听并同步本地 locale
  listen("locale-changed", (payload) => {
    if (typeof payload === "string") {
      setLocale(localeFromConfig(payload));
    }
  });
}

/** 语义对齐 restoreCachedTheme：实际恢复已在模块加载时完成，此处供 main.tsx 显式调用 */
export function restoreCachedLocale(): void {
  // currentLocale 已在模块加载时从 localStorage 同步读取
}

export function setLocale(locale: Locale): void {
  if (locale === currentLocale) return;
  currentLocale = locale;
  try {
    localStorage.setItem(LOCALE_CACHE_KEY, locale);
  } catch {
    // localStorage 不可用时仅更新内存
  }
  listeners.forEach((fn) => fn());
}

export function getLocale(): Locale {
  return currentLocale;
}

/** React hook：订阅 locale 变化，返回 t 函数 */
export function useT(): (key: string, params?: Record<string, string | number>) => string {
  const [, forceUpdate] = useState({});
  useEffect(() => {
    const fn = () => forceUpdate({});
    listeners.add(fn);
    return () => {
      listeners.delete(fn);
    };
  }, []);
  return useCallback(translate, []);
}

// 非 React 上下文使用（如 decorateCodeBlocks 内部）
export const t = translate;
