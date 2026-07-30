// 扩展导入拖拽区。从 ActionBarPanel.tsx 拆出（2026-07-30）。

import { useState, useCallback, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { X } from "lucide-react";
import { cn } from "@/lib/utils";
import { t as ti18n } from "@/lib/i18n";
import type { ActionBarItem, ImportResult } from "./types";

export default function ExtensionDropZone({
  form, onChange,
}: {
  form: Partial<ActionBarItem>;
  onChange: (form: Partial<ActionBarItem>) => void;
}) {
  const [dragging, setDragging] = useState(false);
  const [importing, setImporting] = useState(false);
  const [error, setError] = useState("");

  const doImport = useCallback(
    async (sourcePath: string) => {
      setImporting(true);
      setError("");
      try {
        const result = await invoke<ImportResult>("import_extension", { sourcePath });
        onChange({
          ...form,
          title: result.name || form.title,
          actionData: `${result.sourcePath}|${result.dirName}`,
          isAsync: result.isAsync,
          writeOutputToClipboard: result.writeOutputToClipboard,
        });
      } catch (e) {
        setError(String(e));
      }
      setImporting(false);
    },
    [form, onChange],
  );

  const handleOpenFile = useCallback(async () => {
    try {
      const selected = await openDialog({
        multiple: false,
        filters: [{ name: ti18n("settings.actionBar.extensionFilter"), extensions: ["zip"] }],
      });
      if (typeof selected === "string") doImport(selected);
    } catch { /* cancelled */ }
  }, [doImport]);

  const handleOpenDir = useCallback(async () => {
    try {
      const selected = await openDialog({ directory: true, multiple: false });
      if (typeof selected === "string") doImport(selected);
    } catch { /* cancelled */ }
  }, [doImport]);

  useEffect(() => {
    const win = getCurrentWebview();
    const unlisten = win.onDragDropEvent((event) => {
      const { type } = event.payload;
      if (type === "enter" || type === "over") setDragging(true);
      else if (type === "drop") {
        setDragging(false);
        const paths = (event.payload as { paths: string[] }).paths;
        if (paths.length > 0) doImport(paths[0]);
      } else if (type === "leave") setDragging(false);
    });
    return () => { unlisten.then((fn: () => void) => fn()); };
  }, [doImport]);

  const hasPackage = form.actionData && form.actionData.startsWith("/");

  return (
    <div
      className={cn(
        "rounded-lg border-2 border-dashed transition-colors min-h-[80px] flex flex-col items-center justify-center gap-1.5 p-3",
        dragging ? "border-voice bg-voice/5" : "border-border",
        hasPackage && "border-solid",
      )}
    >
      {importing ? (
        <p className="text-xs text-muted-foreground">{ti18n("settings.actionBar.importing")}</p>
      ) : hasPackage ? (
        <div className="flex items-center gap-2 w-full">
          <div className="flex-1 min-w-0">
            <p className="text-xs font-medium text-foreground truncate">{form.title}</p>
            <p className="font-mono text-[10px] text-muted-foreground/70 truncate">
              {form.actionData?.split("|")[0]}
            </p>
          </div>
          <button
            onClick={() => onChange({ ...form, actionData: "", title: "" })}
            className="shrink-0 rounded-md p-1 text-muted-foreground/60 transition-colors hover:bg-destructive/10 hover:text-destructive"
            aria-label={ti18n("settings.actionBar.clearSelection")}
          >
            <X className="h-3.5 w-3.5" />
          </button>
        </div>
      ) : (
        <>
          <p className="text-[11px] text-muted-foreground/70">{ti18n("settings.actionBar.dropHint")}</p>
          <div className="flex items-center gap-3">
            <button onClick={handleOpenFile} className="text-[11px] text-voice hover:underline">
              {ti18n("settings.actionBar.selectZip")}
            </button>
            <span className="text-[11px] text-muted-foreground/40">|</span>
            <button onClick={handleOpenDir} className="text-[11px] text-voice hover:underline">
              {ti18n("settings.actionBar.selectFolder")}
            </button>
          </div>
        </>
      )}
      {error && <p className="text-[11px] text-destructive">{error}</p>}
    </div>
  );
}
