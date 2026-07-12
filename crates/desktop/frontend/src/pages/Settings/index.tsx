import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen as rawListen, type UnlistenFn } from "@tauri-apps/api/event";
import type { Event } from "@tauri-apps/api/event";
import {
  Settings as SettingsIcon,
  Box,
  Wand2,
  Clipboard,
  Activity,
  Command,
  Type,
  type LucideIcon,
} from "lucide-react";
import { cn } from "@/lib/utils";
import { useT } from "@/lib/i18n";
import ClipboardPanel from "./ClipboardPanel";
import GeneralPanel from "./GeneralPanel";
import ModelsPanel from "./ModelsPanel";
import PromptsPanel from "./PromptsPanel";
import SystemPanel from "./SystemPanel";
import ActionBarPanel from "./ActionBarPanel";
import { HotwordPanel } from "./HotwordPanel";

export interface ConfigResponse {
  config: Record<string, string | number | boolean>;
  asr_engines: { name: string; label: string; current: boolean; is_local: boolean }[];
  llm_models: { name: string; label: string; current: boolean }[];
  ocr_models: { name: string; label: string; current: boolean }[];
  prompts: { id: number; title: string; is_system: boolean }[];
  active_prompt_id: number;
  microphones: string[];
}

type PageName = "clipboard" | "settings" | "models" | "prompts" | "system" | "actionbar" | "hotword";

const NAV_ITEMS: { page: PageName; icon: LucideIcon; labelKey: string }[] = [
  { page: "settings", icon: SettingsIcon, labelKey: "settings.nav.general" },
  { page: "clipboard", icon: Clipboard, labelKey: "settings.nav.clipboard" },
  { page: "actionbar", icon: Command, labelKey: "settings.nav.actionBar" },
  { page: "hotword", icon: Type, labelKey: "settings.nav.hotword" },
  { page: "models", icon: Box, labelKey: "settings.nav.models" },
  { page: "prompts", icon: Wand2, labelKey: "settings.nav.prompts" },
  { page: "system", icon: Activity, labelKey: "settings.nav.system" },
];

function Settings() {
  const t = useT();
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
      showToast(t("settings.loadFailed") + e);
    }
  }, [showToast]);

  useEffect(() => {
    refreshConfig();
    // 拉取初始页面（由 open_settings 暂存）
    invoke<string>("get_initial_page").then((page) => {
      if (page) setPage(page as PageName);
    }).catch(() => {});
    let unlisten: UnlistenFn;
    let unlistenNav: UnlistenFn;
    let cancelled = false;
    rawListen("config-changed", () => refreshConfig()).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    });
    rawListen<string>("settings://navigate", (e: Event<string>) => {
      const page = e.payload;
      if (typeof page === "string") setPage(page as PageName);
    }).then((fn) => {
      if (cancelled) fn();
      else unlistenNav = fn;
    });
    return () => { cancelled = true; unlisten?.(); unlistenNav?.(); };
  }, [refreshConfig]);

  const setVal = useCallback(async (key: string, value: string | number | boolean) => {
    try {
      await invoke("set_config", { key, value });
      await refreshConfig();
    } catch (e) {
      showToast(t("settings.setFailed") + e);
    }
  }, [refreshConfig, showToast]);

  return (
    <div className="flex h-screen overflow-hidden bg-background text-foreground">
      {/* Sidebar */}
      <div className="w-[160px] flex-shrink-0 bg-muted/40 border-r border-border flex flex-col">
        <nav className="flex-1 pt-3">
          {NAV_ITEMS.map(({ page: p, icon: Icon, labelKey }) => (
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
              <span className="truncate">{t(labelKey)}</span>
            </div>
          ))}
        </nav>
      </div>

      {/* Content */}
      <div className="flex-1 overflow-y-auto bg-background p-6">
        {page === "clipboard" ? (
          <ClipboardPanel showToast={showToast} />
        ) : page === "system" ? (
          <SystemPanel showToast={showToast} />
        ) : !configResp ? (
          <div className="flex items-center justify-center h-full text-muted-foreground">{t("settings.loading")}</div>
        ) : page === "settings" ? (
          <GeneralPanel configResp={configResp} setVal={setVal} showToast={showToast} refreshConfig={refreshConfig} />
        ) : page === "models" ? (
          <ModelsPanel showToast={showToast} />
        ) : page === "prompts" ? (
          <PromptsPanel showToast={showToast} />
        ) : page === "actionbar" ? (
          <ActionBarPanel showToast={showToast} />
        ) : page === "hotword" ? (
          <HotwordPanel
            dialect={(configResp.config.fuzzy_dialect as string) || ""}
            setVal={setVal}
            showToast={showToast}
          />
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
