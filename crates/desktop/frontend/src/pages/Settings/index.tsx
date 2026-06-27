import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { History, Settings as SettingsIcon, Box, Wand2, Clipboard, type LucideIcon } from "lucide-react";
import { cn } from "@/lib/utils";
import HistoryPanel from "./HistoryPanel";
import ClipboardPanel from "./ClipboardPanel";
import GeneralPanel from "./GeneralPanel";
import ModelsPanel from "./ModelsPanel";
import PromptsPanel from "./PromptsPanel";

export interface ConfigResponse {
  config: Record<string, string | number | boolean>;
  asr_engines: { name: string; label: string; current: boolean; is_local: boolean }[];
  llm_models: { name: string; label: string; current: boolean }[];
  prompts: { id: number; title: string; is_system: boolean }[];
  active_prompt_id: number;
  microphones: string[];
}

type PageName = "history" | "clipboard" | "settings" | "models" | "prompts";

const NAV_ITEMS: { page: PageName; icon: LucideIcon; label: string }[] = [
  { page: "settings", icon: SettingsIcon, label: "系统设置" },
  { page: "history", icon: History, label: "识别记录" },
  { page: "clipboard", icon: Clipboard, label: "剪贴板" },
  { page: "models", icon: Box, label: "模型管理" },
  { page: "prompts", icon: Wand2, label: "提示词" },
];

function Settings() {
  const [page, setPage] = useState<PageName>("settings");
  const [configResp, setConfigResp] = useState<ConfigResponse | null>(null);
  const [toast, setToast] = useState<string | null>(null);

  const showToast = useCallback((msg: string) => {
    setToast(msg);
    setTimeout(() => setToast(null), 2000);
  }, []);

  const refreshConfig = useCallback(async () => {
    try {
      const resp = await invoke<ConfigResponse>("get_config");
      setConfigResp(resp);
    } catch (e) {
      showToast("加载配置失败：" + e);
    }
  }, [showToast]);

  useEffect(() => {
    refreshConfig();
    let unlisten: UnlistenFn;
    let cancelled = false;
    listen("config-changed", () => refreshConfig()).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    });
    return () => { cancelled = true; unlisten?.(); };
  }, [refreshConfig]);

  const setVal = useCallback(async (key: string, value: string | number | boolean) => {
    try {
      await invoke("set_config", { key, value });
      await refreshConfig();
    } catch (e) {
      showToast("设置失败：" + e);
    }
  }, [refreshConfig, showToast]);

  return (
    <div className="flex h-screen overflow-hidden bg-background text-foreground">
      {/* Sidebar */}
      <div className="w-[160px] flex-shrink-0 bg-muted/40 border-r border-border flex flex-col">
        <div className="px-4 pt-5 pb-4">
          <div className="text-base font-bold tracking-tight">Octopus</div>
          <div className="text-[10px] text-muted-foreground/60 mt-0.5">语音识别 · 剪贴板</div>
        </div>
        <nav className="flex-1 py-1">
          {NAV_ITEMS.map(({ page: p, icon: Icon, label }) => (
            <div
              key={p}
              className={cn(
                "flex items-center gap-2 px-3 mx-1.5 py-2 rounded-md cursor-pointer text-sm transition-colors",
                page === p
                  ? "bg-background text-foreground shadow-sm font-medium"
                  : "text-muted-foreground hover:text-foreground hover:bg-background/50",
              )}
              onClick={() => setPage(p)}
            >
              <Icon className="w-4 h-4 flex-shrink-0" />
              <span className="truncate">{label}</span>
            </div>
          ))}
        </nav>
      </div>

      {/* Content */}
      <div className="flex-1 overflow-y-auto bg-background p-6">
        {page === "clipboard" ? (
          <ClipboardPanel showToast={showToast} />
        ) : !configResp ? (
          <div className="flex items-center justify-center h-full text-muted-foreground">加载中...</div>
        ) : page === "history" ? (
          <HistoryPanel showToast={showToast} />
        ) : page === "settings" ? (
          <GeneralPanel configResp={configResp} setVal={setVal} showToast={showToast} refreshConfig={refreshConfig} />
        ) : page === "models" ? (
          <ModelsPanel showToast={showToast} />
        ) : page === "prompts" ? (
          <PromptsPanel showToast={showToast} />
        ) : null}
      </div>

      {/* Toast */}
      {toast && (
        <div className="fixed bottom-6 left-1/2 -translate-x-1/2 bg-black/80 text-white px-4 py-2 rounded-lg text-sm z-50 max-w-[80%] text-center">
          {toast}
        </div>
      )}
    </div>
  );
}

export default Settings;
