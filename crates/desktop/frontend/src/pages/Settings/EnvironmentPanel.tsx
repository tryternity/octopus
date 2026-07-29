// 环境与路径设置面板：截图/录屏保存目录 + 环境变量（原 Models 页 env tab 挪来）。
// 路径 section 在顶部，环境变量在下方（原 EnvironmentTab 组件复用）。

import { useState, useEffect, useCallback } from "react";
import { invoke } from "@/lib/tauri";
import { open } from "@tauri-apps/plugin-dialog";
import { FolderOpen } from "lucide-react";
import { useT } from "@/lib/i18n";
import EnvironmentTab from "./Models/EnvironmentTab";

interface PathConfig {
  record_output_dir: string;
  screen_output_dir: string;
}

export default function EnvironmentPanel({ showToast }: { showToast: (msg: string) => void }) {
  const t = useT();
  const [paths, setPaths] = useState<PathConfig>({ record_output_dir: "", screen_output_dir: "" });

  const loadPaths = useCallback(async () => {
    try {
      const cfg = await invoke<Record<string, string>>("get_config");
      setPaths({
        record_output_dir: (cfg.record_output_dir as string) || "",
        screen_output_dir: (cfg.screen_output_dir as string) || "",
      });
    } catch { /* ignore */ }
  }, []);

  useEffect(() => { loadPaths(); }, [loadPaths]);

  const pickDir = async (key: "record_output_dir" | "screen_output_dir") => {
    try {
      const selected = await open({ directory: true, multiple: false });
      if (typeof selected === "string" && selected) {
        await invoke("set_config", { key, value: selected });
        setPaths((prev) => ({ ...prev, [key]: selected }));
      }
    } catch { /* user cancelled */ }
  };

  const clearDir = async (key: "record_output_dir" | "screen_output_dir") => {
    await invoke("set_config", { key, value: "" });
    setPaths((prev) => ({ ...prev, [key]: "" }));
  };

  const renderPathRow = (
    key: "record_output_dir" | "screen_output_dir",
    label: string,
    fallback: string,
    value: string,
  ) => (
    <div className="flex items-center gap-2 px-3 py-2 rounded-md border border-border/60 bg-surface">
      <span className="text-xs text-muted-foreground flex-shrink-0 w-16">{label}</span>
      <button
        className="flex-1 flex items-center gap-1.5 min-w-0 text-left group"
        onClick={() => pickDir(key)}
      >
        <FolderOpen className="w-3.5 h-3.5 text-muted-foreground group-hover:text-foreground flex-shrink-0" />
        <span className={`text-xs truncate ${value ? "text-foreground" : "text-muted-foreground/60"}`} title={value || fallback}>
          {value || fallback}
        </span>
      </button>
      {value && (
        <button
          className="text-[10px] text-muted-foreground hover:text-destructive flex-shrink-0"
          onClick={() => clearDir(key)}
        >
          {t("settings.env.reset")}
        </button>
      )}
    </div>
  );

  return (
    <div className="flex flex-col h-full overflow-y-auto thin-scrollbar">
      {/* ── 保存路径 section ── */}
      <div className="space-y-1.5 px-3 py-3 border-b border-border">
        <h3 className="text-xs font-semibold text-foreground mb-1">{t("settings.env.pathsTitle")}</h3>
        {renderPathRow("record_output_dir", t("settings.env.recordings"), "~/Documents/octopus/recordings/", paths.record_output_dir)}
        {renderPathRow("screen_output_dir", t("settings.env.screens"), "~/Documents/octopus/screens/", paths.screen_output_dir)}
      </div>

      {/* ── 环境变量 section ── */}
      <div className="flex-1 px-3 py-3">
        <h3 className="text-xs font-semibold text-foreground mb-2">{t("settings.env.envVarsTitle")}</h3>
        <EnvironmentTab showToast={showToast} />
      </div>
    </div>
  );
}
