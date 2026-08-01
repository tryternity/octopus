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
  Video,
  X,
  type LucideIcon,
} from "lucide-react";
import type { ToastVariant } from "@/lib/useToast";
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
import RecordingPanel from "./RecordingPanel";

export interface ConfigResponse {
  config: Record<string, string | number | boolean>;
  asrEngines: { name: string; label: string; current: boolean; sourceType: number }[];
  llmModels: { name: string; label: string; current: boolean }[];
  ocrModels: { name: string; label: string; current: boolean }[];
  prompts: { id: number; title: string; isSystem: boolean }[];
  activePromptId: number;
  microphones: string[];
}

type PageName = "clipboard" | "settings" | "models" | "prompts" | "system" | "actionbar" | "agent" | "hotword" | "vault" | "recordings";

const NAV_ITEMS: { page: PageName; icon: LucideIcon; labelKey: string }[] = [
  { page: "settings", icon: SettingsIcon, labelKey: "settings.nav.general" },
  { page: "models", icon: Box, labelKey: "settings.nav.models" },
  { page: "actionbar", icon: Command, labelKey: "settings.nav.actionBar" },
  { page: "clipboard", icon: Clipboard, labelKey: "settings.nav.clipboard" },
  { page: "recordings", icon: Video, labelKey: "settings.nav.recordings" },
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
  // toast 反馈（2026-07-21 修订）：success 自动消失，error 不自动消失需用户关闭
  const [toast, setToast] = useState<{ msg: string; variant: ToastVariant } | null>(null);
  // follow-up #10: vault feature 探针。null = 未拉取；false 时隐藏 vault nav。
  const [isVaultEnabled, setIsVaultEnabled] = useState<boolean | null>(null);

  const showToast = useCallback((msg: string, variant: ToastVariant = "success") => {
    setToast({ msg, variant });
    if (variant === "error") return; // error 不自动消失——让用户看清楚后手动关闭
    setTimeout(() => setToast(null), 2000);
  }, []);

  const dismissToast = useCallback(() => setToast(null), []);

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
        ) : effectivePage === "models" ? (
          <ModelsPanel showToast={showToast} />
        ) : effectivePage === "prompts" ? (
          <PromptsPanel showToast={showToast} />
        ) : effectivePage === "actionbar" ? (
          <ActionBarPanel showToast={showToast} />
        ) : effectivePage === "agent" ? (
          <AgentPanel showToast={showToast} />
        ) : effectivePage === "vault" ? (
          <VaultPanel showToast={showToast} />
        ) : effectivePage === "recordings" ? (
          <RecordingPanel showToast={showToast} />
        ) : !configResp ? (
          /* 只有 settings(GeneralPanel) 和 hotword 真正依赖 configResp。
             其他页面各自 invoke 独立命令，不应被 configResp 加载失败阻塞。 */
          <div className="flex items-center justify-center h-full text-muted-foreground">{t("settings.loading")}</div>
        ) : effectivePage === "settings" ? (
          <GeneralPanel configResp={configResp} setVal={setVal} showToast={showToast} refreshConfig={refreshConfig} isVaultEnabled={isVaultEnabled !== false} />
        ) : effectivePage === "hotword" ? (
          <HotwordPanel
            dialect={(configResp.config.fuzzy_dialect as string) || ""}
            asrCorrect={configResp.config.asr_correct as boolean}
            setVal={setVal}
            showToast={showToast}
          />
        ) : null}
      </div>

      {/* Toast —— 半透明背景 + 模糊 + 边框，替代原 bg-black/80 纯黑 */}
      {/* 2026-07-21：error 不自动消失 + 显 X 关闭按钮（用户反馈错误信息看不清） */}
      {toast && (
        <div
          className={[
            "fixed bottom-6 left-1/2 -translate-x-1/2 z-50 flex max-w-[80%] items-center gap-2 rounded-lg px-4 py-2 text-sm shadow-lg backdrop-blur-sm",
            toast.variant === "error"
              ? "border border-destructive/50 bg-destructive/10 text-destructive"
              : "border border-border bg-background/95 text-center",
          ].join(" ")}
          role={toast.variant === "error" ? "alert" : "status"}
        >
          <span className="break-words">{toast.msg}</span>
          {toast.variant === "error" && (
            <button
              type="button"
              onClick={dismissToast}
              aria-label="关闭"
              className="shrink-0 rounded-sm p-0.5 hover:bg-destructive/20"
            >
              <X className="size-3.5" />
            </button>
          )}
        </div>
      )}
    </div>
  );
}

export default Settings;
