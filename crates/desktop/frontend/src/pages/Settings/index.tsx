import { useState, useEffect, useCallback, useMemo } from "react";
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
  Bot,
  Lock,
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
import AgentPanel from "./AgentPanel";
import { HotwordPanel } from "./HotwordPanel";
import VaultPanel from "./VaultPanel";

export interface ConfigResponse {
  config: Record<string, string | number | boolean>;
  asr_engines: { name: string; label: string; current: boolean; is_local: boolean }[];
  llm_models: { name: string; label: string; current: boolean }[];
  ocr_models: { name: string; label: string; current: boolean }[];
  prompts: { id: number; title: string; is_system: boolean }[];
  active_prompt_id: number;
  microphones: string[];
}

type PageName = "clipboard" | "settings" | "models" | "prompts" | "system" | "actionbar" | "agent" | "hotword" | "vault";

const NAV_ITEMS: { page: PageName; icon: LucideIcon; labelKey: string }[] = [
  { page: "settings", icon: SettingsIcon, labelKey: "settings.nav.general" },
  { page: "models", icon: Box, labelKey: "settings.nav.models" },
  { page: "actionbar", icon: Command, labelKey: "settings.nav.actionBar" },
  { page: "clipboard", icon: Clipboard, labelKey: "settings.nav.clipboard" },
  { page: "hotword", icon: Type, labelKey: "settings.nav.hotword" },
  { page: "prompts", icon: Wand2, labelKey: "settings.nav.prompts" },
  { page: "agent", icon: Bot, labelKey: "settings.nav.agent" },
  // follow-up #10: vault nav 仅在 vault feature on 时显示（isVaultEnabled 控制）。
  { page: "vault", icon: Lock, labelKey: "settings.nav.vault" },
  { page: "system", icon: Activity, labelKey: "settings.nav.system" },
];

function Settings() {
  const t = useT();
  const [page, setPage] = useState<PageName>("settings");
  const [configResp, setConfigResp] = useState<ConfigResponse | null>(null);
  const [toast, setToast] = useState<string | null>(null);
  // follow-up #10: vault feature 探针。null = 未拉取；false 时隐藏 vault nav。
  const [isVaultEnabled, setIsVaultEnabled] = useState<boolean | null>(null);

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
    // follow-up #10: vault feature 探针（命令永远注册，后端 cfg 反射）。
    invoke<boolean>("is_vault_enabled")
      .then(setIsVaultEnabled)
      .catch(() => setIsVaultEnabled(false));
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

  // follow-up #10: vault feature off 时从 nav 列表过滤掉 vault。
  // 用 useMemo 避免每次 render 重新 filter。
  const visibleNavItems = useMemo(() => {
    if (isVaultEnabled === false) {
      return NAV_ITEMS.filter((item) => item.page !== "vault");
    }
    return NAV_ITEMS;
  }, [isVaultEnabled]);

  // follow-up #10: 如果当前 page 是 vault 但 feature 被关掉（或探针返回 false），
  // 应回退到 settings 主面板——避免渲染一个 backend 命令不存在的 VaultPanel。
  const effectivePage: PageName =
    page === "vault" && isVaultEnabled === false ? "settings" : page;

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
      {/* Sidebar —— Raycast list 风格：选中项左侧 voice 竖条 + bg-accent 填充 */}
      <div className="w-[176px] flex-shrink-0 border-r border-border bg-muted/30 flex flex-col raycast-ring">
        <nav className="flex-1 space-y-0.5 pt-3">
          {visibleNavItems.map(({ page: p, icon: Icon, labelKey }) => {
            const active = effectivePage === p;
            return (
              <div
                key={p}
                className={cn(
                  "relative flex items-center gap-2.5 mx-2 px-2.5 py-1.5 rounded-md cursor-pointer text-[13px] tracking-[0.0125em] transition-colors",
                  active
                    ? "bg-accent font-medium text-foreground"
                    : "text-muted-foreground hover:text-foreground hover:bg-accent/60",
                )}
                onClick={() => setPage(p)}
              >
                {/* 选中态左侧 voice 竖条——Raycast list 选中签名 */}
                {active && (
                  <span className="absolute left-[-8px] top-1.5 bottom-1.5 w-[2px] rounded-full bg-voice" />
                )}
                <Icon className="w-4 h-4 flex-shrink-0" />
                <span className="truncate">{t(labelKey)}</span>
              </div>
            );
          })}
        </nav>
      </div>

      {/* Content */}
      <div className="flex-1 min-w-0 overflow-y-auto bg-background p-6">
        {effectivePage === "clipboard" ? (
          <ClipboardPanel showToast={showToast} />
        ) : effectivePage === "system" ? (
          <SystemPanel showToast={showToast} />
        ) : !configResp ? (
          <div className="flex items-center justify-center h-full text-muted-foreground">{t("settings.loading")}</div>
        ) : effectivePage === "settings" ? (
          <GeneralPanel configResp={configResp} setVal={setVal} showToast={showToast} refreshConfig={refreshConfig} isVaultEnabled={isVaultEnabled !== false} />
        ) : effectivePage === "models" ? (
          <ModelsPanel showToast={showToast} />
        ) : effectivePage === "prompts" ? (
          <PromptsPanel showToast={showToast} />
        ) : effectivePage === "actionbar" ? (
          <ActionBarPanel showToast={showToast} />
        ) : effectivePage === "agent" ? (
          <AgentPanel showToast={showToast} />
        ) : effectivePage === "hotword" ? (
          <HotwordPanel
            dialect={(configResp.config.fuzzy_dialect as string) || ""}
            setVal={setVal}
            showToast={showToast}
          />
        ) : effectivePage === "vault" ? (
          <VaultPanel showToast={showToast} />
        ) : null}
      </div>

      {/* Toast —— 半透明背景 + 模糊 + 边框，替代原 bg-black/80 纯黑 */}
      {toast && (
        <div className="fixed bottom-6 left-1/2 -translate-x-1/2 z-50 max-w-[80%] rounded-lg border border-border bg-background/95 px-4 py-2 text-sm text-center shadow-lg backdrop-blur-sm">
          {toast}
        </div>
      )}
    </div>
  );
}

export default Settings;
