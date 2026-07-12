import { useState, useEffect, useCallback } from "react";
import { invoke } from "@/lib/tauri";
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
