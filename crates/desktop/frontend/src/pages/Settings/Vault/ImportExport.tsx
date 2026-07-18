import { useCallback, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Download, Upload } from "lucide-react";
import { useT } from "@/lib/i18n";
import { Button } from "@/components/ui/button";

/**
 * ImportExport —— Bitwarden JSON 导入 + 导出。
 *
 * 导入：用户选 .json 文件 → 读文本 → invoke vault_import_bitwarden({ json })
 *   返回 ImportReport { imported, skipped, errors }
 *
 * 导出：invoke vault_export() 返回 JSON 字符串 → Blob 下载。
 *   导出明文敏感，UI 显示 warning。
 *
 * 注意：本项目无 @tauri-apps/plugin-fs，所以用浏览器 FileReader/Blob API（webview 支持）。
 */

interface ImportReport {
  imported: number;
  skipped: number;
  errors: string[];
}

export default function ImportExport({ showToast }: { showToast: (msg: string) => void }) {
  const t = useT();
  const [busy, setBusy] = useState<"import" | "export" | null>(null);
  const fileInputRef = useRef<HTMLInputElement | null>(null);

  const handleImportClick = useCallback(() => {
    fileInputRef.current?.click();
  }, []);

  const handleFileSelected = useCallback(
    async (e: React.ChangeEvent<HTMLInputElement>) => {
      const file = e.target.files?.[0];
      // 重置 input value 允许下次选同一文件
      e.target.value = "";
      if (!file) return;
      setBusy("import");
      try {
        const text = await file.text();
        const report = await invoke<ImportReport>("vault_import_bitwarden", { json: text });
        showToast(
          t("settings.vault.importExport.importSuccess", {
            imported: report.imported,
            skipped: report.skipped,
          }),
        );
      } catch (err) {
        showToast(String(err));
      } finally {
        setBusy(null);
      }
    },
    [showToast, t],
  );

  const handleExport = useCallback(async () => {
    setBusy("export");
    try {
      const json = await invoke<string>("vault_export");
      const blob = new Blob([json], { type: "application/json" });
      const url = URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = url;
      const ts = new Date().toISOString().slice(0, 10);
      a.download = `vault-export-${ts}.json`;
      document.body.appendChild(a);
      a.click();
      document.body.removeChild(a);
      // 释放 blob URL（下次回收）
      setTimeout(() => URL.revokeObjectURL(url), 1000);
      showToast(t("settings.vault.importExport.exportLabel"));
    } catch (err) {
      showToast(String(err));
    } finally {
      setBusy(null);
    }
  }, [showToast, t]);

  return (
    <div className="mx-auto max-w-md space-y-4">
      <h2 className="text-xl font-semibold">{t("settings.vault.importExport.title")}</h2>

      <div className="space-y-2 rounded-lg border border-border/50 bg-muted/15 p-4">
        <div className="flex items-center justify-between gap-3">
          <div className="min-w-0">
            <p className="text-sm font-medium">{t("settings.vault.importExport.importLabel")}</p>
            <p className="text-xs text-muted-foreground/70">Bitwarden JSON</p>
          </div>
          <Button
            variant="outline"
            size="sm"
            onClick={handleImportClick}
            disabled={busy !== null}
          >
            <Upload />
            {busy === "import" ? "..." : t("settings.vault.importExport.importLabel")}
          </Button>
        </div>
        <input
          ref={fileInputRef}
          type="file"
          accept="application/json,.json"
          className="hidden"
          onChange={handleFileSelected}
        />
      </div>

      <div className="space-y-2 rounded-lg border border-amber-500/30 bg-amber-500/5 p-4">
        <div className="flex items-center justify-between gap-3">
          <div className="min-w-0">
            <p className="text-sm font-medium">{t("settings.vault.importExport.exportLabel")}</p>
            <p className="text-xs text-amber-700 dark:text-amber-400">
              {t("settings.vault.importExport.exportWarning")}
            </p>
          </div>
          <Button
            variant="outline"
            size="sm"
            onClick={handleExport}
            disabled={busy !== null}
          >
            <Download />
            {busy === "export" ? "..." : t("settings.vault.importExport.exportLabel")}
          </Button>
        </div>
      </div>
    </div>
  );
}
