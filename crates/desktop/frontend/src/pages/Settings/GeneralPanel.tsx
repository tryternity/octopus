import { useState, useCallback, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";
import { Mic, Volume2, Sparkles, Keyboard, ClipboardList, Palette } from "lucide-react";
import type { ThemeInfo } from "@/lib/theme";
import { applyThemeById as applyTheme } from "@/lib/theme";
import type { ConfigResponse } from "./index";
import { useT, setLocale } from "@/lib/i18n";
import type { ToastVariant } from "@/lib/useToast";
import ShortcutButton from "@/components/ShortcutButton";
import { Card, CardHeader, CardTitle, CardContent } from "@/components/ui/card";
import { Row } from "@/components/ui/row";
import { Toggle } from "@/components/ui/toggle";
import { Select } from "@/components/ui/input";
import { UnderlineTabs } from "@/components/ui/tabs";
import SyncPanel from "./Vault/SyncPanel";

interface GeneralPanelProps {
  configResp: ConfigResponse;
  setVal: (key: string, value: string | number | boolean) => Promise<void>;
  showToast: (msg: string, variant?: ToastVariant) => void;
  refreshConfig: () => Promise<void>;
  /** vault feature 是否启用——控制 vault autotype 快捷键 Row 是否渲染。
   *  feature off 时不应让用户配置一个无效快捷键。 */
  isVaultEnabled?: boolean;
}

export default function GeneralPanel({ configResp, setVal, showToast, refreshConfig, isVaultEnabled }: GeneralPanelProps) {
  const { config: cfg, prompts, active_prompt_id, microphones } = configResp;
  const [capturingKey, setCapturingKey] = useState<string | null>(null);
  const [activeTab, setActiveTab] = useState<"general" | "shortcut" | "voice" | "sync">("general");
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
      // ESC 通常用于退出捕获；但 record_stop_shortcut 允许录 ESC 作为快捷键值
      // （录屏停止键默认就是 Escape，用户应能改成其他键或保持 ESC）。
      if (e.key === "Escape" && configKey !== "record_stop_shortcut") {
        setCapturingKey(null);
        return;
      }
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

  const tabs = [
    { key: "general", label: t("settings.general.tabGeneral") },
    { key: "shortcut", label: t("settings.general.tabShortcut") },
    { key: "voice", label: t("settings.general.tabVoice") },
    { key: "sync", label: t("settings.general.tabSync") },
  ];

  return (
    <div>
      <UnderlineTabs
        items={tabs}
        active={activeTab}
        onChange={(k) => setActiveTab(k as typeof activeTab)}
        className="mb-4"
      />

      {/* ── 一般：外观 + 交互 + 剪贴板 ── */}
      {activeTab === "general" && (
        <>
          <Card className="mb-3">
            <CardHeader>
              <Palette className="w-4 h-4 text-muted-foreground" />
              <CardTitle>{t("settings.general.appearance")}</CardTitle>
            </CardHeader>
            <CardContent>
              <Row label={t("settings.general.theme")} effect={t("settings.effect.now")} hint={t("settings.general.themeHint")}>
                <Select value={(cfg.clipboard_theme as string) || "light"} onChange={(e) => setTheme(e.target.value)}>
                  {themes.map((th) => <option key={th.id} value={th.id}>{th.name}</option>)}
                </Select>
              </Row>
              <Row label={t("settings.uiLanguage")} effect={t("settings.effect.now")}>
                <Select value={(cfg.ui_language as string) || "zh-CN"} onChange={(e) => setUiLanguage(e.target.value)}>
                  <option value="zh-CN">{t("settings.uiLanguageZhCN")}</option>
                  <option value="en">{t("settings.uiLanguageEn")}</option>
                </Select>
              </Row>
            </CardContent>
          </Card>

          <Card className="mb-3">
            <CardHeader>
              <Mic className="w-4 h-4 text-muted-foreground" />
              <CardTitle>{t("settings.general.interaction")}</CardTitle>
            </CardHeader>
            <CardContent>
              <Row label={t("settings.general.micDevice")} effect={t("settings.effect.nextRecording")}>
                <Select value={cfg.microphone as string} onChange={(e) => setVal("microphone", e.target.value)}>
                  <option value="">{t("settings.general.systemDefault")}</option>
                  {microphones.map((m) => <option key={m} value={m}>{m}</option>)}
                </Select>
              </Row>
              <Row label={t("settings.general.denoiseMode")} effect={t("settings.effect.now")}>
                <Select value={cfg.denoise_mode as number} onChange={(e) => setVal("denoise_mode", parseInt(e.target.value))}>
                  <option value={0}>{t("settings.general.denoiseNone")}</option><option value={1}>{t("settings.general.denoiseLight")}</option><option value={2}>{t("settings.general.denoiseDeep")}</option>
                </Select>
              </Row>
              <Row label={t("settings.general.toolbarAutoHide")} effect={t("settings.effect.now")} hint={t("settings.general.toolbarAutoHideHint")}>
                <Toggle on={cfg.hide_toolbar as boolean} onClick={() => toggleVal("hide_toolbar")} />
              </Row>
              <Row label={t("settings.general.pasteSwitchEnglish")} effect={t("settings.effect.now")} hint={t("settings.general.pasteSwitchEnglishHint")}>
                <Toggle on={cfg.switch_input_source_on_paste as boolean} onClick={() => toggleVal("switch_input_source_on_paste")} />
              </Row>
            </CardContent>
          </Card>

          <Card className="mb-3">
            <CardHeader>
              <ClipboardList className="w-4 h-4 text-muted-foreground" />
              <CardTitle>{t("settings.general.clipboardSettings")}</CardTitle>
            </CardHeader>
            <CardContent>
              <Row label={t("settings.general.clipboardListen")} effect={t("settings.effect.now")} hint={t("settings.general.clipboardListenHint")}>
                <Toggle on={cfg.clipboard_enabled as boolean} onClick={() => toggleVal("clipboard_enabled")} />
              </Row>
              <Row label={t("settings.general.maxItems")} effect={t("settings.effect.nextStart")} hint={t("settings.general.maxItemsHint")}>
                <Select value={cfg.clipboard_max_items as number} onChange={(e) => setVal("clipboard_max_items", parseInt(e.target.value))}>
                  {[200, 500, 1000, 2000, 5000].map((v) => <option key={v} value={v}>{v} 条</option>)}
                </Select>
              </Row>
              <Row label={t("settings.general.autoCleanDays")} effect={t("settings.effect.nextStart")} hint={t("settings.general.autoCleanDaysHint")}>
                <Select value={cfg.clipboard_max_age_days as number} onChange={(e) => setVal("clipboard_max_age_days", parseInt(e.target.value))}>
                  {[1, 3, 7, 15, 30].map((v) => <option key={v} value={v}>{v} 天</option>)}
                </Select>
              </Row>
            </CardContent>
          </Card>
        </>
      )}

      {/* ── 快捷键 ── */}
      {activeTab === "shortcut" && (
        <Card className="mb-3">
          <CardHeader>
            <Keyboard className="w-4 h-4 text-muted-foreground" />
            <CardTitle>{t("settings.general.shortcut")}</CardTitle>
          </CardHeader>
          <CardContent>
            <Row label={t("settings.general.asrShortcut")} effect={t("settings.effect.now")}>
              <ShortcutButton shortcut={cfg.asr_shortcut as string} capturing={capturingKey === "asr_shortcut"} onClick={() => startShortcutCapture("asr_shortcut")} />
            </Row>
            <Row label={t("settings.general.polishShortcut")} effect={t("settings.effect.now")} hint={t("settings.general.polishShortcutHint")}>
              <ShortcutButton shortcut={cfg.polish_global_shortcut as string} capturing={capturingKey === "polish_global_shortcut"} onClick={() => startShortcutCapture("polish_global_shortcut")} />
            </Row>
            <Row label={t("settings.general.editShortcut")} effect={t("settings.effect.now")} hint={t("settings.general.editShortcutHint")}>
              <ShortcutButton shortcut={cfg.edit_global_shortcut as string} capturing={capturingKey === "edit_global_shortcut"} onClick={() => startShortcutCapture("edit_global_shortcut")} />
            </Row>
            <Row label={t("settings.general.actionBarShortcut")} effect={t("settings.effect.now")} hint={t("settings.general.actionBarShortcutHint")}>
              <ShortcutButton shortcut={cfg.action_bar_shortcut as string} capturing={capturingKey === "action_bar_shortcut"} onClick={() => startShortcutCapture("action_bar_shortcut")} />
            </Row>
            {isVaultEnabled && (
              <Row label={t("settings.general.vaultAutotypeShortcut")} effect={t("settings.effect.now")} hint={t("settings.general.vaultAutotypeShortcutHint")}>
                <ShortcutButton shortcut={cfg.vault_autotype_shortcut as string} capturing={capturingKey === "vault_autotype_shortcut"} onClick={() => startShortcutCapture("vault_autotype_shortcut")} />
              </Row>
            )}
            <Row label={t("settings.general.screenshotShortcut")} effect={t("settings.effect.now")} hint={t("settings.general.screenshotShortcutHint")}>
              <ShortcutButton shortcut={cfg.screenshot_shortcut as string} capturing={capturingKey === "screenshot_shortcut"} onClick={() => startShortcutCapture("screenshot_shortcut")} />
            </Row>
            {/* 录屏快捷键（config-driven，与 screenshot 同模式，支持热重载） */}
            <Row label={t("settings.general.recordShortcut")} effect={t("settings.effect.now")} hint={t("settings.general.recordShortcutHint")}>
              <ShortcutButton shortcut={cfg.record_shortcut as string} capturing={capturingKey === "record_shortcut"} onClick={() => startShortcutCapture("record_shortcut")} />
            </Row>
            <Row label={t("settings.general.recordStopShortcut")} effect={t("settings.effect.now")} hint={t("settings.general.recordStopShortcutHint")}>
              <ShortcutButton shortcut={cfg.record_stop_shortcut as string} capturing={capturingKey === "record_stop_shortcut"} onClick={() => startShortcutCapture("record_stop_shortcut")} />
            </Row>
            <Row label={t("settings.general.clipboardShortcut")} effect={t("settings.effect.now")}>
              <ShortcutButton shortcut={cfg.clipboard_shortcut as string} capturing={capturingKey === "clipboard_shortcut"} onClick={() => startShortcutCapture("clipboard_shortcut")} />
            </Row>
          </CardContent>
        </Card>
      )}

      {/* ── 语音：识别 + 润色 ── */}
      {activeTab === "voice" && (
        <>
          <Card className="mb-3">
            <CardHeader>
              <Volume2 className="w-4 h-4 text-muted-foreground" />
              <CardTitle>{t("settings.general.recording")}</CardTitle>
            </CardHeader>
            <CardContent>
              <Row label={t("settings.general.recogLang")} effect={t("settings.effect.nextRecording")}>
                <Select value={cfg.language as string} onChange={(e) => setVal("language", e.target.value)}>
                  <option value="auto">{t("settings.general.recogAuto")}</option><option value="zh">{t("settings.general.recogZh")}</option><option value="en">{t("settings.general.recogEn")}</option>
                </Select>
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
                <Select value={cfg.segment_silence as number} onChange={(e) => setVal("segment_silence", parseFloat(e.target.value))}>
                  {[300, 400, 500, 600].map((v) => <option key={v} value={v}>{v}ms</option>)}
                </Select>
              </Row>
            </CardContent>
          </Card>

          <Card className="mb-3">
            <CardHeader>
              <Sparkles className="w-4 h-4 text-muted-foreground" />
              <CardTitle>{t("settings.general.asrPolish")}</CardTitle>
            </CardHeader>
            <CardContent>
              <Row label={t("settings.general.polishMode")} effect={t("settings.effect.now")}>
                <Select value={cfg.polish_mode as number} onChange={(e) => setVal("polish_mode", parseInt(e.target.value))}>
                  <option value={0}>{t("settings.general.polishOff")}</option><option value={1}>{t("settings.general.polishFinalOnly")}</option><option value={2}>{t("settings.general.polishIntermediate")}</option>
                </Select>
              </Row>
              <Row label={t("settings.general.polishPrompt")} effect={t("settings.effect.now")} hint={t("settings.general.polishPromptHint")}>
                <Select value={active_prompt_id} onChange={(e) => setActivePrompt(parseInt(e.target.value))}>
                  {prompts.map((p) => <option key={p.id} value={p.id}>{p.title}{p.is_system ? t("settings.general.builtinSuffix") : ""}</option>)}
                </Select>
              </Row>
              <Row label={t("settings.general.polishInterval")} effect={t("settings.effect.nextRecording")}>
                <Select value={cfg.polish_min_interval as number} onChange={(e) => setVal("polish_min_interval", parseFloat(e.target.value))}>
                  <option value={0}>{t("settings.general.polishIntervalLast")}</option>
                  {[3, 4, 5, 6, 7, 8].map((v) => <option key={v} value={v}>{t("settings.general.polishIntervalEvery", { v })}</option>)}
                </Select>
              </Row>
              <Row label={t("settings.general.polishPauseThreshold")} effect={t("settings.effect.nextRecording")} hint={t("settings.general.polishPauseThresholdHint")}>
                <Select value={cfg.pause_polish_threshold_ms as number} onChange={(e) => setVal("pause_polish_threshold_ms", parseFloat(e.target.value))}>
                  {[600, 700, 800, 900, 1000].map((v) => <option key={v} value={v}>{v}ms</option>)}
                </Select>
              </Row>
            </CardContent>
          </Card>
        </>
      )}
      {activeTab === "sync" && (
        <div className="h-[calc(100vh-200px)] overflow-auto">
          <SyncPanel showToast={showToast} />
        </div>
      )}
    </div>
  );
}
