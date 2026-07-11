import { useState, useEffect, useCallback } from "react";
import { invoke } from "@/lib/tauri";
import zhCN from "@/locales/zh-CN.json";
import en from "@/locales/en.json";

type Locale = "zh-CN" | "en";

const DICTS: Record<Locale, Record<string, string>> = {
  "zh-CN": zhCN as Record<string, string>,
  "en": en as Record<string, string>,
};

let currentLocale: Locale = "zh-CN";
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

/** 从后端 config 读 ui_language，初始化 locale（main.tsx 启动时调用） */
export async function initI18n(): Promise<void> {
  try {
    const resp = await invoke<{ config: Record<string, unknown> }>("get_config");
    const uiLang = resp.config?.ui_language as string | undefined;
    setLocale(localeFromConfig(uiLang));
  } catch {
    // 后端未就绪时用默认 zh-CN
  }
}

export function setLocale(locale: Locale): void {
  if (locale === currentLocale) return;
  currentLocale = locale;
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
