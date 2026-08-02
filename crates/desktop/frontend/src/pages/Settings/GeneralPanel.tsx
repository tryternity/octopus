import { useState, useCallback, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";
import { Mic, Volume2, Sparkles, Keyboard, ClipboardList, Palette, AlertCircle, TerminalSquare } from "lucide-react";
import type { ThemeInfo } from "@/lib/theme";
import { applyThemeById as applyTheme } from "@/lib/theme";
import { isMac } from "@/lib/platform";
import type { ConfigResponse } from "./index";
import { useT, setLocale } from "@/lib/i18n";
import type { ToastVariant } from "@/lib/useToast";
import ShortcutButton from "@/components/ShortcutButton";
import { Card, CardHeader, CardTitle, CardContent } from "@/components/ui/card";
import { Row } from "@/components/ui/row";
import { Toggle } from "@/components/ui/toggle";
import { Select } from "@/components/ui/input";
import { UnderlineTabs } from "@/components/ui/tabs";
import { PermissionCard, PERMISSIONS } from "@/components/PermissionCard";
import SyncPanel from "./Vault/SyncPanel";
import EnvironmentPanel from "./EnvironmentPanel";

// 字号 slider 边界——与 Terminal/index.tsx MIN/MAX_FONT_SIZE 对齐。
const TERMINAL_FONT_SIZE_MIN = 8;
const TERMINAL_FONT_SIZE_MAX = 32;
const TERMINAL_FONT_SIZE_DEFAULT = 13;
// 默认字体族——与 infra/config.rs default_terminal_font_family() 对齐（单一真相源在后端）。
const TERMINAL_FONT_FAMILY_DEFAULT = "Menlo";

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
  const { config: cfg, prompts, activePromptId, microphones } = configResp;
  const [capturingKey, setCapturingKey] = useState<string | null>(null);
  const [activeTab, setActiveTab] = useState<"general" | "shortcut" | "permission" | "voice" | "font" | "env" | "sync">("general");
  const [themes, setThemes] = useState<ThemeInfo[]>([]);
  // 系统已安装的等宽字体族名列表——mount 时通过 list_monospace_fonts 后端命令拉取。
  // 列表元素即字体族名（如 "Menlo"），直接作为下拉 label + value，也直接写入
  // terminal_font_family（xterm fontFamily 接受单个族名，浏览器自动 fallback monospace）。
  const [monoFonts, setMonoFonts] = useState<string[]>([]);

  useEffect(() => {
    invoke<ThemeInfo[]>("list_themes").then(setThemes).catch(console.error);
  }, []);

  useEffect(() => {
    invoke<string[]>("list_monospace_fonts").then(setMonoFonts).catch(console.error);
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
      // ESC 用于退出捕获（octopus 全局通用停止键）
      if (e.key === "Escape") {
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

  const tabs: { key: string; label: string }[] = [
    { key: "general", label: t("settings.general.tabGeneral") },
    { key: "shortcut", label: t("settings.general.tabShortcut") },
    { key: "voice", label: t("settings.general.tabVoice") },
    { key: "font", label: t("settings.general.tabFont") },
    { key: "env", label: t("settings.general.tabEnv") },
    // macOS 专有：隐私与权限 tab（麦克风/辅助功能/屏幕录制）
    ...(isMac ? [{ key: "permission", label: t("settings.general.tabPermission") }] : []),
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
            {/* 语音识别（单键三模式触发键）——dropdown 5 选 1，非 ShortcutButton capture。
                后端 set_config("asr_shortcut") 校验枚举 + 热重载 PTT。 */}
            <Row label={t("settings.general.asrShortcut")} effect={t("settings.effect.now")} hint={t("settings.general.asrShortcutHint")}>
              <Select value={(cfg.asr_shortcut as string) || "OptRight"} onChange={(e) => setVal("asr_shortcut", e.target.value)}>
                <option value="OptRight">{t("settings.general.asrKeyOptRight")}</option>
                <option value="CmdRight">{t("settings.general.asrKeyCmdRight")}</option>
                <option value="CtrlRight">{t("settings.general.asrKeyCtrlRight")}</option>
                <option value="ShiftRight">{t("settings.general.asrKeyShiftRight")}</option>
                <option value="Fn">{t("settings.general.asrKeyFn")}</option>
              </Select>
            </Row>
            <Row label={t("settings.general.editShortcut")} effect={t("settings.effect.now")} hint={t("settings.general.editShortcutHint")}>
              <ShortcutButton shortcut={cfg.edit_global_shortcut as string} capturing={capturingKey === "edit_global_shortcut"} onClick={() => startShortcutCapture("edit_global_shortcut")} />
            </Row>
            <Row label={t("settings.general.clipboardShortcut")} effect={t("settings.effect.now")}>
              <ShortcutButton shortcut={cfg.clipboard_shortcut as string} capturing={capturingKey === "clipboard_shortcut"} onClick={() => startShortcutCapture("clipboard_shortcut")} />
            </Row>
            <Row label={t("settings.general.actionBarShortcut")} effect={t("settings.effect.now")} hint={t("settings.general.actionBarShortcutHint")}>
              <ShortcutButton shortcut={cfg.action_bar_shortcut as string} capturing={capturingKey === "action_bar_shortcut"} onClick={() => startShortcutCapture("action_bar_shortcut")} />
            </Row>
            <Row label={t("settings.general.screenshotShortcut")} effect={t("settings.effect.now")} hint={t("settings.general.screenshotShortcutHint")}>
              <ShortcutButton shortcut={cfg.screenshot_shortcut as string} capturing={capturingKey === "screenshot_shortcut"} onClick={() => startShortcutCapture("screenshot_shortcut")} />
            </Row>
            {/* 录屏快捷键（config-driven，与 screenshot 同模式，支持热重载）。
                停止录屏固定 ESC 不暴露（octopus 全局通用停止键）。 */}
            <Row label={t("settings.general.recordShortcut")} effect={t("settings.effect.now")} hint={t("settings.general.recordShortcutHint")}>
              <ShortcutButton shortcut={cfg.record_shortcut as string} capturing={capturingKey === "record_shortcut"} onClick={() => startShortcutCapture("record_shortcut")} />
            </Row>
            {isVaultEnabled && (
              <Row label={t("settings.general.vaultAutotypeShortcut")} effect={t("settings.effect.now")} hint={t("settings.general.vaultAutotypeShortcutHint")}>
                <ShortcutButton shortcut={cfg.vault_autotype_shortcut as string} capturing={capturingKey === "vault_autotype_shortcut"} onClick={() => startShortcutCapture("vault_autotype_shortcut")} />
              </Row>
            )}
          </CardContent>
        </Card>
      )}

      {/* ── 隐私与权限（macOS 专有）：麦克风 / 辅助功能 / 屏幕录制 ── */}
      {activeTab === "permission" && (
        <Card>
          <CardHeader>
            <CardTitle className="text-base">{t("settings.general.tabPermission")}</CardTitle>
          </CardHeader>
          <CardContent className="space-y-3">
            {PERMISSIONS.map((def) => (
              <PermissionCard key={def.key} def={def} />
            ))}
            {/* 升级提示（无证书签名 app 的 TCC 限制，放底部低调提示） */}
            <div className="flex items-start gap-2 px-3 py-2 rounded-md border border-amber-600/40 bg-amber-600/5 text-xs text-amber-700 dark:text-amber-500 mt-2">
              <AlertCircle className="w-4 h-4 flex-shrink-0 mt-0.5" />
              <span className="flex-1 whitespace-normal">{t("onboarding.permissions.upgradeNote")}</span>
            </div>
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
                <Select value={activePromptId} onChange={(e) => setActivePrompt(parseInt(e.target.value))}>
                  {prompts.map((p) => <option key={p.id} value={p.id}>{p.title}{p.isSystem ? t("settings.general.builtinSuffix") : ""}</option>)}
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
      {activeTab === "env" && (
        <EnvironmentPanel showToast={showToast} />
      )}

      {/* ── 字体字号（terminal_font_size / terminal_font_family；后续可扩展编辑器字体）── */}
      {activeTab === "font" && (
        <Card className="mb-3">
          <CardHeader>
            <TerminalSquare className="w-4 h-4 text-muted-foreground" />
            <CardTitle>{t("settings.general.terminalFont")}</CardTitle>
          </CardHeader>
          <CardContent>
            {/* 字号：slider + 数字显示。onchange 立即 setVal，xterm 即时 effect。 */}
            <Row
              label={t("settings.general.fontSize")}
              effect={t("settings.effect.now")}
              hint={t("settings.general.terminalFontHint")}
            >
              <div className="flex items-center gap-3">
                <input
                  type="range"
                  min={TERMINAL_FONT_SIZE_MIN}
                  max={TERMINAL_FONT_SIZE_MAX}
                  step={1}
                  value={typeof cfg.terminal_font_size === "number" && cfg.terminal_font_size > 0
                    ? cfg.terminal_font_size
                    : TERMINAL_FONT_SIZE_DEFAULT}
                  onChange={(e) => setVal("terminal_font_size", Number(e.target.value))}
                  className="w-40 accent-voice"
                />
                <span className="w-10 text-right text-sm tabular-nums">
                  {typeof cfg.terminal_font_size === "number" && cfg.terminal_font_size > 0
                    ? cfg.terminal_font_size
                    : TERMINAL_FONT_SIZE_DEFAULT}
                </span>
              </div>
            </Row>

            {/* 字体族：dropdown 选项来自系统已安装的等宽字体（list_monospace_fonts 动态加载）。
                value 即字体族名（如 "Menlo"），直接存入 terminal_font_family——xterm
                fontFamily 接受单个族名，浏览器自动 fallback 到 monospace。
                若 config 存的是旧格式 CSS 降级链（如 '"SF Mono", Menlo, ...'）匹配不到任何
                系统字体，dropdown 回退到首项（首个系统等宽字体）。 */}
            <Row
              label={t("settings.general.fontFamily")}
              effect={t("settings.effect.now")}
              hint={t("settings.general.terminalFontHint")}
            >
              <Select
                value={
                  monoFonts.includes(cfg.terminal_font_family as string)
                    ? (cfg.terminal_font_family as string)
                    : monoFonts[0] ?? ""
                }
                onChange={(e) => void setVal("terminal_font_family", e.target.value)}
              >
                {monoFonts.map((f) => (
                  <option key={f} value={f}>{f}</option>
                ))}
              </Select>
            </Row>

            {/* 预览——用当前字号 + 字体族渲染样例文字，让用户直观感受效果。 */}
            <Row label={t("settings.general.fontPreview")}>
              <div className="flex items-center gap-2 flex-1">
                <div
                  className="flex-1 rounded-md border border-border/40 bg-background px-3 py-2 text-muted-foreground"
                  style={{
                    fontSize: `${typeof cfg.terminal_font_size === "number" && cfg.terminal_font_size > 0
                      ? cfg.terminal_font_size
                      : TERMINAL_FONT_SIZE_DEFAULT}px`,
                    fontFamily: typeof cfg.terminal_font_family === "string" && cfg.terminal_font_family
                      ? cfg.terminal_font_family
                      : undefined,
                  }}
                >
                  {t("settings.general.fontPreviewText")}
                </div>
                {/* 恢复默认：字号 13 + SF Mono。仅当当前值偏离默认时显示，避免无意义点击。 */}
                {(typeof cfg.terminal_font_size === "number"
                  && cfg.terminal_font_size !== TERMINAL_FONT_SIZE_DEFAULT)
                  || (typeof cfg.terminal_font_family === "string"
                    && cfg.terminal_font_family
                    && cfg.terminal_font_family !== TERMINAL_FONT_FAMILY_DEFAULT) ? (
                  <button
                    type="button"
                    onClick={() => {
                      void setVal("terminal_font_size", TERMINAL_FONT_SIZE_DEFAULT);
                      void setVal("terminal_font_family", TERMINAL_FONT_FAMILY_DEFAULT);
                    }}
                    className="shrink-0 px-2.5 py-1 rounded-md text-xs border border-border bg-transparent hover:bg-muted transition-colors"
                  >
                    {t("settings.general.fontRestoreDefault")}
                  </button>
                ) : null}
              </div>
            </Row>
          </CardContent>
        </Card>
      )}
      {activeTab === "sync" && (
        <div className="h-[calc(100vh-200px)] overflow-auto">
          <SyncPanel showToast={showToast} />
        </div>
      )}
    </div>
  );
}
