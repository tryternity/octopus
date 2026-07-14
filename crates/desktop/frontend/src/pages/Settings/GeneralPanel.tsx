import { useState, useCallback, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";
import { cn } from "@/lib/utils";
import { Mic, Volume2, Sparkles, Keyboard, ClipboardList, Palette } from "lucide-react";
import type { ThemeInfo } from "@/lib/theme";
import { applyThemeById as applyTheme } from "@/lib/theme";
import type { ConfigResponse } from "./index";
import { useT, setLocale } from "@/lib/i18n";

interface GeneralPanelProps {
  configResp: ConfigResponse;
  setVal: (key: string, value: string | number | boolean) => Promise<void>;
  showToast: (msg: string) => void;
  refreshConfig: () => Promise<void>;
}

function Card({ icon: Icon, title, children }: { icon: React.ElementType; title: string; children: React.ReactNode }) {
  return (
    <div className="mb-3 border border-border rounded-lg overflow-hidden bg-background">
      <div className="flex items-center gap-2 px-4 py-2.5 bg-muted/40 border-b border-border">
        <Icon className="w-4 h-4 text-muted-foreground" />
        <h3 className="text-sm font-semibold">{title}</h3>
      </div>
      <div className="px-4 py-1">{children}</div>
    </div>
  );
}

function Row({ label, hint, effect, children }: { label: string; hint?: string; effect?: string; children: React.ReactNode }) {
  return (
    <div className="flex items-center justify-between py-2.5 border-b border-border/40 last:border-0 gap-3">
      <div className="flex flex-col gap-0.5 flex-1 min-w-0">
        <span className="text-sm">{label}{effect && <span className="ml-1.5 text-[10px] text-muted-foreground/60 px-1 py-0.5 rounded bg-muted">{effect}</span>}</span>
        {hint && <span className="text-xs text-muted-foreground/60">{hint}</span>}
      </div>
      {children}
    </div>
  );
}

function Toggle({ on, onClick }: { on: boolean; onClick: () => void }) {
  return (
    <button
      className={cn(
        "relative w-10 h-[22px] rounded-full transition-colors flex-shrink-0",
        on ? "bg-voice" : "bg-muted-foreground/25",
      )}
      onClick={onClick}
    >
      <span className={cn(
        "absolute top-0.5 left-0.5 w-[18px] h-[18px] bg-white rounded-full transition-transform shadow-sm",
        on && "translate-x-[18px]",
      )} />
    </button>
  );
}

function ShortcutButton({ shortcut, capturing, onClick }: { shortcut: string; capturing: boolean; onClick: () => void }) {
  const t = useT();
  if (capturing) {
    return (
      <button
        className="px-3 py-1.5 rounded-md text-xs font-medium text-voice bg-voice/5 border border-voice/40 cursor-pointer animate-pulse"
        onClick={onClick}
      >
        {t("settings.general.shortcutRecordingHint")}
      </button>
    );
  }
  const keys = shortcut.split("+");
  return (
    <button
      className="flex items-center gap-1 px-2.5 py-1.5 rounded-md border border-border bg-stone-50 hover:border-foreground/30 cursor-pointer transition-colors group"
      onClick={onClick}
    >
      {keys.map((k, i) => (
        <span key={i} className="flex items-center gap-1">
          {i > 0 && <span className="text-muted-foreground/40 text-[10px]">+</span>}
          <kbd className="min-w-[20px] px-1.5 py-0.5 text-[11px] font-medium text-stone-700 bg-white rounded border border-stone-200 shadow-sm group-hover:border-stone-300 transition-colors">
            {k === "CmdOrCtrl" ? "⌘" : k === "Alt" ? "⌥" : k === "Shift" ? "⇧" : k}
          </kbd>
        </span>
      ))}
    </button>
  );
}

const selectClass = "px-2.5 py-1.5 border border-border rounded-md text-sm bg-background min-w-[160px] max-w-[200px] cursor-pointer hover:border-foreground/30 transition-colors outline-none focus:border-voice/40";

export default function GeneralPanel({ configResp, setVal, showToast, refreshConfig }: GeneralPanelProps) {
  const { config: cfg, prompts, active_prompt_id, microphones } = configResp;
  const [capturingKey, setCapturingKey] = useState<string | null>(null);
  const [themes, setThemes] = useState<ThemeInfo[]>([]);

  useEffect(() => {
    invoke<ThemeInfo[]>("list_themes").then(setThemes).catch(console.error);
  }, []);

  const t = useT();

  const setTheme = useCallback(async (themeId: string) => {
    await applyTheme(themeId);
    await setVal("clipboard_theme", themeId);
  }, [setVal]);

  const setUiLanguage = useCallback(async (lang: string) => {
    await setVal("ui_language", lang);
    setLocale(lang as "zh-CN" | "en");
    await emit("locale-changed", lang);
  }, [setVal]);

  const toggleVal = useCallback(async (key: string) => {
    const current = cfg[key] as boolean;
    await setVal(key, !current);
  }, [cfg, setVal]);

  const setActivePrompt = useCallback(async (id: number) => {
    try {
      await invoke("set_active_prompt", { id });
      await refreshConfig();
    } catch (e) { showToast(t("settings.setFailed") + e); }
  }, [refreshConfig, showToast]);

  const startShortcutCapture = useCallback((configKey: string) => {
    setCapturingKey(configKey);
  }, []);

  // 监听器生命周期绑定到 capturingKey：切 Tab/关窗卸载时 useEffect cleanup 自动
  // removeEventListener，避免 stale handler 持续 preventDefault/stopPropagation 劫持
  // 全局键盘（其他输入框打不出字）+ setVal 写已卸载组件。
  useEffect(() => {
    if (!capturingKey) return;
    const configKey = capturingKey;
    const handler = async (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      if (e.key === "Escape") { setCapturingKey(null); return; }
      // 纯修饰键不触发，等待用户按实际键
      if (e.key === "Alt" || e.key === "Shift" || e.key === "Control" || e.key === "Meta") return;
      const parts: string[] = [];
      if (e.metaKey || e.ctrlKey) parts.push("CmdOrCtrl");
      if (e.altKey) parts.push("Alt");
      if (e.shiftKey) parts.push("Shift");
      const keyName = e.code.startsWith("Key") ? e.code.slice(3) : e.code;
      parts.push(keyName);
      const shortcut = parts.join("+");
      try {
        await invoke("check_shortcut", { shortcut });
        await setVal(configKey, shortcut);
      } catch (err) { showToast("" + err); }
      setCapturingKey(null);
    };
    document.addEventListener("keydown", handler, true);
    return () => document.removeEventListener("keydown", handler, true);
  }, [capturingKey, setVal, showToast]);

  return (
    <div className="max-w-[640px]">
      <Card icon={Palette} title={t("settings.general.appearance")}>
        <Row label={t("settings.general.theme")} effect={t("settings.effect.now")} hint={t("settings.general.themeHint")}>
          <select className={selectClass} value={(cfg.clipboard_theme as string) || "light"} onChange={(e) => setTheme(e.target.value)}>
            {themes.map((t) => <option key={t.id} value={t.id}>{t.name}</option>)}
          </select>
        </Row>
        <Row label={t("settings.uiLanguage")} effect={t("settings.effect.now")}>
          <select className={selectClass} value={(cfg.ui_language as string) || "zh-CN"} onChange={(e) => setUiLanguage(e.target.value)}>
            <option value="zh-CN">{t("settings.uiLanguageZhCN")}</option>
            <option value="en">{t("settings.uiLanguageEn")}</option>
          </select>
        </Row>
      </Card>

      <Card icon={Mic} title={t("settings.general.interaction")}>
        <Row label={t("settings.general.micDevice")} effect={t("settings.effect.nextRecording")}>
          <select className={selectClass} value={cfg.microphone as string} onChange={(e) => setVal("microphone", e.target.value)}>
            <option value="">{t("settings.general.systemDefault")}</option>
            {microphones.map((m) => <option key={m} value={m}>{m}</option>)}
          </select>
        </Row>
        <Row label={t("settings.general.denoiseMode")} effect={t("settings.effect.now")}>
          <select className={selectClass} value={cfg.denoise_mode as number} onChange={(e) => setVal("denoise_mode", parseInt(e.target.value))}>
            <option value={0}>{t("settings.general.denoiseNone")}</option><option value={1}>{t("settings.general.denoiseLight")}</option><option value={2}>{t("settings.general.denoiseDeep")}</option>
          </select>
        </Row>
        <Row label={t("settings.general.toolbarAutoHide")} effect={t("settings.effect.now")} hint={t("settings.general.toolbarAutoHideHint")}>
          <Toggle on={cfg.hide_toolbar as boolean} onClick={() => toggleVal("hide_toolbar")} />
        </Row>
        <Row label={t("settings.general.clipboardListen")} effect={t("settings.effect.now")} hint={t("settings.general.clipboardListenHint")}>
          <Toggle on={cfg.clipboard_enabled as boolean} onClick={() => toggleVal("clipboard_enabled")} />
        </Row>
        <Row label={t("settings.general.pasteSwitchEnglish")} effect={t("settings.effect.now")} hint={t("settings.general.pasteSwitchEnglishHint")}>
          <Toggle on={cfg.switch_input_source_on_paste as boolean} onClick={() => toggleVal("switch_input_source_on_paste")} />
        </Row>
      </Card>

      <Card icon={Keyboard} title={t("settings.general.shortcut")}>
        <Row label={t("settings.general.asrShortcut")} effect={t("settings.effect.now")}>
          <ShortcutButton shortcut={cfg.asr_shortcut as string} capturing={capturingKey === "asr_shortcut"} onClick={() => startShortcutCapture("asr_shortcut")} />
        </Row>
        <Row label={t("settings.general.polishShortcut")} effect={t("settings.effect.now")} hint={t("settings.general.polishShortcutHint")}>
          <ShortcutButton shortcut={cfg.polish_global_shortcut as string} capturing={capturingKey === "polish_global_shortcut"} onClick={() => startShortcutCapture("polish_global_shortcut")} />
        </Row>
        <Row label={t("settings.general.editShortcut")} effect={t("settings.effect.now")} hint={t("settings.general.editShortcutHint")}>
          <ShortcutButton shortcut={cfg.edit_global_shortcut as string} capturing={capturingKey === "edit_global_shortcut"} onClick={() => startShortcutCapture("edit_global_shortcut")} />
        </Row>
        <Row label={t("settings.general.clipboardShortcut")} effect={t("settings.effect.now")}>
          <ShortcutButton shortcut={cfg.clipboard_shortcut as string} capturing={capturingKey === "clipboard_shortcut"} onClick={() => startShortcutCapture("clipboard_shortcut")} />
        </Row>
        <Row label={t("settings.general.clipboardTabShortcut")} effect={t("settings.effect.now")} hint={t("settings.general.clipboardTabShortcutHint")}>
          <div className="flex items-center gap-1.5">
            <select className={selectClass + " min-w-[120px]"} value={(cfg.clipboard_tab_modifier as string) || "ctrl"} onChange={(e) => setVal("clipboard_tab_modifier", e.target.value)}>
              <option value="cmd">⌘ Command</option>
              <option value="ctrl">⌃ Control</option>
              <option value="alt">⌥ Option</option>
            </select>
            <span className="text-xs text-muted-foreground">+ 1..7</span>
          </div>
        </Row>
        <Row label={t("settings.general.actionBarShortcut")} effect={t("settings.effect.now")} hint={t("settings.general.actionBarShortcutHint")}>
          <ShortcutButton shortcut={cfg.action_bar_shortcut as string} capturing={capturingKey === "action_bar_shortcut"} onClick={() => startShortcutCapture("action_bar_shortcut")} />
        </Row>
      </Card>

      <Card icon={Volume2} title={t("settings.general.recording")}>
        <Row label={t("settings.general.recogLang")} effect={t("settings.effect.nextRecording")}>
          <select className={selectClass} value={cfg.language as string} onChange={(e) => setVal("language", e.target.value)}>
            <option value="auto">{t("settings.general.recogAuto")}</option><option value="zh">{t("settings.general.recogZh")}</option><option value="en">{t("settings.general.recogEn")}</option>
          </select>
        </Row>
        <Row label={t("settings.general.hardwareAccel")} effect={t("settings.effect.nextRecording")}>
          <Toggle on={cfg.asr_hardware_accelerated as boolean} onClick={() => toggleVal("asr_hardware_accelerated")} />
        </Row>
        <Row label={t("settings.general.pinyinCorrect")} effect={t("settings.effect.now")} hint={t("settings.general.pinyinCorrectHint")}>
          <Toggle on={cfg.asr_correct as boolean} onClick={() => toggleVal("asr_correct")} />
        </Row>
        <Row label={t("settings.general.tradSimpOutput")} effect={t("settings.effect.now")} hint={t("settings.general.tradSimpOutputHint")}>
          <Toggle on={cfg.output_simplified as boolean} onClick={() => toggleVal("output_simplified")} />
        </Row>
        <Row label={t("settings.general.sentencePause")} effect={t("settings.effect.nextRecording")} hint={t("settings.general.sentencePauseHint")}>
          <select className={selectClass} value={cfg.segment_silence as number} onChange={(e) => setVal("segment_silence", parseFloat(e.target.value))}>
            {[300, 400, 500, 600].map((v) => <option key={v} value={v}>{v}ms</option>)}
          </select>
        </Row>
      </Card>

      <Card icon={Sparkles} title={t("settings.general.asrPolish")}>
        <Row label={t("settings.general.polishMode")} effect={t("settings.effect.now")}>
          <select className={selectClass} value={cfg.polish_mode as number} onChange={(e) => setVal("polish_mode", parseInt(e.target.value))}>
            <option value={0}>{t("settings.general.polishOff")}</option><option value={1}>{t("settings.general.polishFinalOnly")}</option><option value={2}>{t("settings.general.polishIntermediate")}</option>
          </select>
        </Row>
        <Row label={t("settings.general.polishPrompt")} effect={t("settings.effect.now")} hint={t("settings.general.polishPromptHint")}>
          <select className={selectClass} value={active_prompt_id} onChange={(e) => setActivePrompt(parseInt(e.target.value))}>
            {prompts.map((p) => <option key={p.id} value={p.id}>{p.title}{p.is_system ? t("settings.general.builtinSuffix") : ""}</option>)}
          </select>
        </Row>
        <Row label={t("settings.general.polishInterval")} effect={t("settings.effect.nextRecording")}>
          <select className={selectClass} value={cfg.polish_min_interval as number} onChange={(e) => setVal("polish_min_interval", parseFloat(e.target.value))}>
            <option value={0}>{t("settings.general.polishIntervalLast")}</option>
            {[3, 4, 5, 6, 7, 8].map((v) => <option key={v} value={v}>{t("settings.general.polishIntervalEvery", { v })}</option>)}
          </select>
        </Row>
        <Row label={t("settings.general.polishPauseThreshold")} effect={t("settings.effect.nextRecording")} hint={t("settings.general.polishPauseThresholdHint")}>
          <select className={selectClass} value={cfg.pause_polish_threshold_ms as number} onChange={(e) => setVal("pause_polish_threshold_ms", parseFloat(e.target.value))}>
            {[600, 700, 800, 900, 1000].map((v) => <option key={v} value={v}>{v}ms</option>)}
          </select>
        </Row>
      </Card>

      <Card icon={ClipboardList} title={t("settings.general.clipboardSettings")}>
        <Row label={t("settings.general.maxItems")} effect={t("settings.effect.nextStart")} hint={t("settings.general.maxItemsHint")}>
          <select className={selectClass} value={cfg.clipboard_max_items as number} onChange={(e) => setVal("clipboard_max_items", parseInt(e.target.value))}>
            {[100, 200, 300, 500, 1000].map((v) => <option key={v} value={v}>{v} 条</option>)}
          </select>
        </Row>
        <Row label={t("settings.general.autoCleanDays")} effect={t("settings.effect.nextStart")} hint={t("settings.general.autoCleanDaysHint")}>
          <select className={selectClass} value={cfg.clipboard_max_age_days as number} onChange={(e) => setVal("clipboard_max_age_days", parseInt(e.target.value))}>
            {[1, 3, 7, 15, 30].map((v) => <option key={v} value={v}>{v} 天</option>)}
          </select>
        </Row>
      </Card>
    </div>
  );
}
