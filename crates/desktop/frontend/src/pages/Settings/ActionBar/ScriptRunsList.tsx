// 脚本执行记录列表。从 ActionBarPanel.tsx 拆出（2026-07-30）。

import { useState, useCallback, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { cn } from "@/lib/utils";
import { useT } from "@/lib/i18n";
import { Button } from "@/components/ui/button";
import type { ScriptRun } from "./types";

export default function ScriptRunsList({ showToast }: { showToast: (msg: string) => void }) {
  const t = useT();
  const [runs, setRuns] = useState<ScriptRun[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [expandedId, setExpandedId] = useState<number | null>(null);
  const [selectedIds, setSelectedIds] = useState<Set<number>>(new Set());

  const refresh = useCallback(async () => {
    try {
      const list = await invoke<ScriptRun[]>("list_script_runs", { limit: 100 });
      setRuns(list);
    } catch {
      // 静默——脚本记录列表加载失败不应阻塞设置页
    }
    setLoaded(true);
  }, []);

  useEffect(() => { refresh(); }, [refresh]);

  const handleClear = useCallback(async () => {
    try {
      await invoke("clear_script_runs", { keepRecent: 100 });
      showToast(t("settings.actionBar.cleanedOldRecords"));
      refresh();
    } catch (e) {
      showToast(t("settings.actionBar.cleanFailed") + e);
    }
  }, [showToast, refresh]);

  const toggleSelect = useCallback((id: number) => {
    setSelectedIds((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id); else next.add(id);
      return next;
    });
  }, []);

  const allSelected = runs.length > 0 && selectedIds.size === runs.length;
  const toggleSelectAll = useCallback(() => {
    setSelectedIds(allSelected ? new Set() : new Set(runs.map((r) => r.id)));
  }, [allSelected, runs]);

  const handleDeleteSelected = useCallback(async () => {
    if (selectedIds.size === 0) return;
    try {
      await invoke("delete_script_runs", { ids: Array.from(selectedIds) });
      showToast(t("settings.actionBar.deleted"));
      setSelectedIds(new Set());
      refresh();
    } catch (e) {
      showToast(t("settings.actionBar.deleteFailed") + e);
    }
  }, [selectedIds, showToast, refresh]);

  if (!loaded) {
    return <p className="py-12 text-center text-sm text-muted-foreground">{t("settings.actionBar.loadingRecords")}</p>;
  }

  if (runs.length === 0) {
    return (
      <div className="flex flex-col items-center justify-center gap-2 py-16 text-center">
        <p className="text-sm font-medium">{t("settings.actionBar.noRecords")}</p>
        <p className="text-xs text-muted-foreground">{t("settings.actionBar.recordsHint")}</p>
      </div>
    );
  }

  const statusColor = (r: ScriptRun) => {
    if (r.exitCode === null) return "bg-warning";
    return r.exitCode === 0 ? "bg-success" : "bg-destructive";
  };
  const statusLabel = (r: ScriptRun) => {
    if (r.exitCode === null) return r.errorMsg || t("settings.actionBar.statusError");
    return r.exitCode === 0 ? t("settings.actionBar.statusSuccess") : t("settings.actionBar.statusFailed", { n: r.exitCode });
  };

  return (
    <div>
      {/* 顶部工具栏：全选 + 删除选中 + 清理旧记录 */}
      <div className="mb-3 flex items-center gap-3">
        <label className="flex items-center gap-1.5 text-xs text-muted-foreground cursor-pointer">
          <input
            type="checkbox"
            checked={allSelected}
            onChange={toggleSelectAll}
            className="h-3.5 w-3.5 accent-voice"
          />
          {t("settings.actionBar.selectAll")}
        </label>
        <Button
          variant="outline"
          size="sm"
          disabled={selectedIds.size === 0}
          onClick={handleDeleteSelected}
        >
          {t("settings.actionBar.deleteSelected")} ({selectedIds.size})
        </Button>
        <div className="ml-auto">
          <Button variant="outline" size="sm" onClick={handleClear}>
            {t("settings.actionBar.cleanOldRecords")}
          </Button>
        </div>
      </div>
      <div className="space-y-1.5">
        {runs.map((r) => (
          <div key={r.id} className={cn(
            "rounded-lg border bg-muted/15 overflow-hidden transition-colors",
            selectedIds.has(r.id) ? "border-voice/40" : "border-border",
          )}>
            <div className="flex items-center gap-3 px-3 py-2">
              <input
                type="checkbox"
                checked={selectedIds.has(r.id)}
                onChange={() => toggleSelect(r.id)}
                className="h-3.5 w-3.5 shrink-0 accent-voice"
              />
              <button
                onClick={() => setExpandedId(expandedId === r.id ? null : r.id)}
                className="flex flex-1 items-center gap-3 text-left"
              >
                <span className={cn("h-1.5 w-1.5 shrink-0 rounded-full", statusColor(r))} />
                <span className="shrink-0 text-xs font-medium">{r.itemTitle || t("settings.actionBar.untitled")}</span>
                <span className="shrink-0 font-mono text-[10px] uppercase tracking-wider text-muted-foreground">{r.scriptType}</span>
                <span className="shrink-0 text-[11px] text-muted-foreground">{statusLabel(r)}</span>
                <span className="ml-auto shrink-0 text-[11px] text-muted-foreground">
                  {r.durationMs != null ? `${r.durationMs}ms` : "—"}
                </span>
              </button>
            </div>
            {expandedId === r.id && (
              <div className="space-y-2 border-t border-border/50 px-3.5 py-2.5">
                {r.stdout && (
                  <div>
                    <p className="mb-1 font-mono text-[10px] uppercase tracking-wider text-muted-foreground">stdout</p>
                    <textarea
                      readOnly
                      className="w-full min-h-[60px] resize-y bg-background border border-border rounded px-2 py-1.5 font-mono text-xs leading-relaxed"
                      value={r.stdout.slice(0, 8000)}
                    />
                  </div>
                )}
                {r.stderr && (
                  <div>
                    <p className="mb-1 font-mono text-[10px] uppercase tracking-wider text-destructive/70">stderr</p>
                    <textarea
                      readOnly
                      className="w-full min-h-[40px] resize-y bg-background border border-border rounded px-2 py-1.5 font-mono text-xs leading-relaxed text-destructive/80"
                      value={r.stderr.slice(0, 8000)}
                    />
                  </div>
                )}
                {r.errorMsg && <p className="text-xs text-orange-600">{r.errorMsg}</p>}
              </div>
            )}
          </div>
        ))}
      </div>
    </div>
  );
}
