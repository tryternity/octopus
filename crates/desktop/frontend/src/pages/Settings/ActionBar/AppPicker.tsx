import { useState, useEffect, useRef, useMemo } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Search, X, Plus, Globe } from "lucide-react";
import { cn } from "@/lib/utils";
import { useT } from "@/lib/i18n";

/** list_all_apps 返回的应用简要信息 */
interface AppBrief {
  name: string;
  bundleId: string;
  icon: string; // base64 data URI，空串=无图标
}

interface AppPickerProps {
  /** JSON 数组字符串 ["com.apple.Safari"]，空串=全局项 */
  value: string;
  onChange: (v: string) => void;
}

/** 解析 app_bundle_ids JSON 数组。空串/非法 → 空数组。 */
function parseBundleIds(s: string): string[] {
  if (!s) return [];
  try {
    const arr = JSON.parse(s);
    return Array.isArray(arr) ? arr.filter((x) => typeof x === "string") : [];
  } catch {
    return [];
  }
}

/** 序列化 bundle_id 数组为 JSON 字符串。空数组 → 空串（全局项）。 */
function serializeBundleIds(ids: string[]): string {
  return ids.length === 0 ? "" : JSON.stringify(ids);
}

/**
 * App 多选器：给 action_bar 菜单项绑定 app（bundle_id）。
 *
 * 设计：已选 chips（icon + name + ×）+ 点击展开搜索浮层。
 * 空状态 = 全局命令（所有 app 显示），有选中 = 仅绑定的 app 显示。
 */
export default function AppPicker({ value, onChange }: AppPickerProps) {
  const t = useT();
  const [allApps, setAllApps] = useState<AppBrief[]>([]);
  const [query, setQuery] = useState("");
  const [open, setOpen] = useState(false);
  const containerRef = useRef<HTMLDivElement>(null);

  const selectedIds = useMemo(() => parseBundleIds(value), [value]);

  // 加载全部应用列表（组件 mount 时一次性拉取）
  useEffect(() => {
    invoke<AppBrief[]>("list_all_apps")
      .then(setAllApps)
      .catch((e) => console.error("list_all_apps failed:", e));
  }, []);

  // outside click 关闭浮层
  useEffect(() => {
    if (!open) return;
    const handler = (e: MouseEvent) => {
      if (containerRef.current && !containerRef.current.contains(e.target as Node)) {
        setOpen(false);
        setQuery("");
      }
    };
    document.addEventListener("mousedown", handler);
    return () => document.removeEventListener("mousedown", handler);
  }, [open]);

  const selectedApps = useMemo(
    () => selectedIds.map((bid) => allApps.find((a) => a.bundleId === bid)).filter(Boolean) as AppBrief[],
    [selectedIds, allApps],
  );

  const filteredApps = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return allApps;
    return allApps.filter(
      (a) => a.name.toLowerCase().includes(q) || a.bundleId.toLowerCase().includes(q),
    );
  }, [allApps, query]);

  const toggleApp = (bundleId: string) => {
    const next = selectedIds.includes(bundleId)
      ? selectedIds.filter((id) => id !== bundleId)
      : [...selectedIds, bundleId];
    onChange(serializeBundleIds(next));
  };

  const removeApp = (bundleId: string) => {
    onChange(serializeBundleIds(selectedIds.filter((id) => id !== bundleId)));
  };

  return (
    <div ref={containerRef} className="relative">
      {/* 标签 */}
      <div className="mb-1.5 flex items-center gap-1.5 text-xs font-medium text-muted-foreground">
        <span>{t("settings.actionBar.appBinding")}</span>
        {selectedIds.length === 0 && (
          <span className="inline-flex items-center gap-0.5 text-emerald-600 dark:text-emerald-400">
            <Globe className="h-3 w-3" />
            {t("settings.actionBar.globalCommand")}
          </span>
        )}
      </div>

      {/* 已选区 / 触发区 */}
      <div
        onClick={() => setOpen(!open)}
        className="flex min-h-[36px] cursor-pointer flex-wrap items-center gap-1.5 rounded-md border border-input bg-background px-2 py-1.5 text-sm transition-colors hover:border-muted-foreground/40"
      >
        {selectedApps.length === 0 ? (
          <span className="flex items-center gap-1 text-muted-foreground/70">
            <Plus className="h-3.5 w-3.5" />
            {t("settings.actionBar.appBindingEmpty")}
          </span>
        ) : (
          selectedApps.map((app) => (
            <span
              key={app.bundleId}
              onClick={(e) => {
                e.stopPropagation();
                removeApp(app.bundleId);
              }}
              className="inline-flex items-center gap-1 rounded bg-muted px-1.5 py-0.5 text-xs transition-colors hover:bg-muted/70 hover:text-destructive"
            >
              {app.icon && (
                <img src={app.icon} alt="" className="h-4 w-4 rounded-[3px]" />
              )}
              <span>{app.name}</span>
              <X className="h-3 w-3" />
            </span>
          ))
        )}
      </div>

      {/* 提示 */}
      <p className="mt-1 text-[11px] leading-relaxed text-muted-foreground/70">
        {t("settings.actionBar.appBindingHint")}
      </p>

      {/* 搜索浮层 */}
      {open && (
        <div className="absolute z-50 mt-1 w-full rounded-md border border-input bg-background p-2 shadow-lg">
          {/* 搜索框 */}
          <div className="mb-2 flex items-center gap-1.5 border-b border-border pb-1.5">
            <Search className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
            <input
              autoFocus
              autoCapitalize="off"
              autoCorrect="off"
              spellCheck={false}
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder={t("settings.actionBar.searchApps")}
              className="w-full bg-transparent text-sm outline-none placeholder:text-muted-foreground/50"
            />
          </div>

          {/* 应用列表 */}
          <div className="max-h-48 overflow-y-auto">
            {filteredApps.length === 0 ? (
              <p className="py-3 text-center text-xs text-muted-foreground/60">
                {t("settings.actionBar.noAppsFound")}
              </p>
            ) : (
              filteredApps.map((app) => {
                const isSelected = selectedIds.includes(app.bundleId);
                return (
                  <button
                    key={app.bundleId}
                    onClick={() => toggleApp(app.bundleId)}
                    className={cn(
                      "flex w-full items-center gap-2 rounded px-1.5 py-1 text-left text-sm transition-colors",
                      isSelected
                        ? "bg-emerald-500/10 text-emerald-700 dark:text-emerald-300"
                        : "hover:bg-muted",
                    )}
                  >
                    {app.icon ? (
                      <img src={app.icon} alt="" className="h-5 w-5 shrink-0 rounded-[4px]" />
                    ) : (
                      <div className="h-5 w-5 shrink-0 rounded-[4px] bg-muted" />
                    )}
                    <span className="flex-1 truncate">{app.name}</span>
                    {isSelected && (
                      <span className="text-xs text-emerald-600 dark:text-emerald-400">✓</span>
                    )}
                  </button>
                );
              })
            )}
          </div>
        </div>
      )}
    </div>
  );
}
