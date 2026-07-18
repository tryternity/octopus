import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Check, Copy, RefreshCw } from "lucide-react";
import { useT } from "@/lib/i18n";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Toggle as UIToggle } from "@/components/ui/toggle";
import { Segmented } from "@/components/ui/tabs";
import {
  buildPayload,
  DEFAULT_RANDOM,
  DEFAULT_EN,
  DEFAULT_ZH,
  DEFAULT_PIN,
  type Mode,
  type GeneratorPayload,
  type RandomConfig as RandomCfg,
  type PassphraseEnConfig as EnCfg,
  type PassphraseZhConfig as ZhCfg,
  type PinConfig as PinCfg,
} from "./buildConfig";

/**
 * PasswordGenerator —— 独立浮窗（label=password_generator_window）。
 *
 * 全局快捷键 CmdOrCtrl+Shift+G 触发；后端 register_vault_generator_shortcut
 * 新建/聚焦此窗口。
 *
 * 4 种生成模式（对应 octopus_vault::generator::GeneratorConfig 变体，
 * 后端用 #[serde(tag="mode", rename_all="camelCase")] 内标签）：
 *   random       随机字符（length + uppercase/lowercase/numbers/symbols/avoid_ambiguous）
 *   passphraseEn 英文短语（word_count + separator + capitalize + include_number）
 *   passphraseZh 中文短语（word_count + separator + include_number + include_symbol）
 *   pin          数字 PIN（length）
 *
 * 配置组装（mode → 后端 payload）抽到 ./buildConfig.ts，配套单元测试覆盖
 * 各 mode 分发与 serde 约定（见 buildConfig.test.ts，回归门 for final-review C1）。
 *
 * 切模式即重新生成；显示区可复制。
 */

const Toggle = ({ checked, onChange }: { checked: boolean; onChange: (v: boolean) => void }) => (
  <UIToggle on={checked} onClick={() => onChange(!checked)} />
);

// 简易强度评估（前端独立估，不依赖后端）。
// 返回 0..4：长度 < 8=0, < 12=1；字符种类多样性 +1/类；> 20 +1。
function estimateStrength(s: string): number {
  if (!s) return 0;
  let score = 0;
  if (s.length >= 8) score += 1;
  if (s.length >= 12) score += 1;
  const classes =
    (/[a-z]/.test(s) ? 1 : 0) +
    (/[A-Z]/.test(s) ? 1 : 0) +
    (/[0-9]/.test(s) ? 1 : 0) +
    (/[^a-zA-Z0-9]/.test(s) ? 1 : 0);
  if (classes >= 3) score += 1;
  if (classes >= 4 && s.length >= 16) score += 1;
  return Math.min(score, 4);
}

const strengthColor = ["bg-destructive", "bg-amber-500", "bg-yellow-500", "bg-voice", "bg-success"];

export default function PasswordGenerator() {
  const t = useT();
  const [mode, setMode] = useState<Mode>("passphraseZh");
  const [result, setResult] = useState("");
  const [copied, setCopied] = useState(false);

  // 各模式本地配置（默认值与 Rust 端 *Config::default() 对齐，定义于 buildConfig.ts）
  const [randomCfg, setRandomCfg] = useState<RandomCfg>(DEFAULT_RANDOM);
  const [enCfg, setEnCfg] = useState<EnCfg>(DEFAULT_EN);
  const [zhCfg, setZhCfg] = useState<ZhCfg>(DEFAULT_ZH);
  const [pinCfg, setPinCfg] = useState<PinCfg>(DEFAULT_PIN);

  const buildPayloadCb = useCallback((): GeneratorPayload => {
    return buildPayload(mode, randomCfg, enCfg, zhCfg, pinCfg);
  }, [mode, randomCfg, enCfg, zhCfg, pinCfg]);

  const regenerate = useCallback(async () => {
    try {
      const pwd = await invoke<string>("vault_generate", { cfg: buildPayloadCb() });
      setResult(pwd);
      setCopied(false);
    } catch (e) {
      setResult(String(e));
    }
  }, [buildPayloadCb]);

  // 初始 + 模式/配置变化时重新生成
  useEffect(() => {
    regenerate();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [mode, randomCfg, enCfg, zhCfg, pinCfg]);

  const handleCopy = useCallback(async () => {
    try {
      await navigator.clipboard.writeText(result);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch {
      // 忽略——剪贴板权限失败时静默
    }
  }, [result]);

  const score = useMemo(() => estimateStrength(result), [result]);

  return (
    <div className="flex h-screen flex-col gap-3 bg-background p-4 text-foreground">
      {/* 模式切换 */}
      <Segmented
        items={[
          { key: "passphraseZh", label: t("settings.vault.generator.mode.passphraseZh") },
          { key: "passphraseEn", label: t("settings.vault.generator.mode.passphraseEn") },
          { key: "random", label: t("settings.vault.generator.mode.random") },
          { key: "pin", label: t("settings.vault.generator.mode.pin") },
        ]}
        active={mode}
        onChange={(k) => setMode(k as Mode)}
      />

      {/* 显示区 */}
      <div className="min-h-[72px] select-text break-all rounded-md border border-border bg-muted/30 p-3 font-mono text-lg">
        {result || "..."}
      </div>

      {/* 强度条 */}
      <div className="flex items-center gap-2">
        <span className="text-[11px] uppercase tracking-wide text-muted-foreground/70">
          {t("settings.vault.generator.strength")}
        </span>
        <div className="flex flex-1 gap-1">
          {[0, 1, 2, 3, 4].map((i) => (
            <div
              key={i}
              className={`h-1.5 flex-1 rounded-full ${i <= score ? strengthColor[score] : "bg-muted"}`}
            />
          ))}
        </div>
        <span className="text-[11px] tabular-nums text-muted-foreground">
          {t(`settings.vault.generator.strengthLevels.${score}`)}
        </span>
      </div>

      {/* 模式专属配置 */}
      <div className="rounded-md border border-border/50 bg-muted/15 p-3">
        {mode === "random" && (
          <div className="space-y-2">
            <div className="flex items-center gap-2">
              <label className="w-16 text-[11px] uppercase tracking-wide text-muted-foreground/70">
                {t("settings.vault.generator.length")}
              </label>
              <input
                type="range"
                min={4}
                max={64}
                value={randomCfg.length}
                onChange={(e) => setRandomCfg({ ...randomCfg, length: Number(e.target.value) })}
                className="flex-1 accent-voice"
              />
              <span className="w-8 text-right text-sm tabular-nums">{randomCfg.length}</span>
            </div>
            <div className="grid grid-cols-2 gap-x-3 gap-y-1.5 text-xs">
              {([
                ["uppercase", randomCfg.uppercase],
                ["lowercase", randomCfg.lowercase],
                ["numbers", randomCfg.numbers],
                ["symbols", randomCfg.symbols],
                ["avoid_ambiguous", randomCfg.avoid_ambiguous],
              ] as const).map(([key, val]) => (
                <label key={key} className="flex items-center gap-2">
                  <Toggle
                    checked={val as boolean}
                    onChange={(v) => setRandomCfg({ ...randomCfg, [key]: v })}
                  />
                  <span className="text-muted-foreground">
                    {t(`settings.vault.generator.${key === "avoid_ambiguous" ? "avoidAmbiguous" : key}`)}
                  </span>
                </label>
              ))}
            </div>
          </div>
        )}

        {mode === "passphraseEn" && (
          <div className="space-y-2 text-xs">
            <div className="flex items-center gap-2">
              <label className="w-16 text-[11px] uppercase tracking-wide text-muted-foreground/70">
                {t("settings.vault.generator.wordCount")}
              </label>
              <input
                type="number"
                min={2}
                max={10}
                value={enCfg.word_count}
                onChange={(e) => setEnCfg({ ...enCfg, word_count: Number(e.target.value) || 3 })}
                className="w-16 rounded-md border border-border bg-background px-2 py-1 text-sm tabular-nums"
              />
              <label className="ml-2 w-16 text-[11px] uppercase tracking-wide text-muted-foreground/70">
                {t("settings.vault.generator.separator")}
              </label>
              <Input
                value={enCfg.separator}
                onChange={(e) => setEnCfg({ ...enCfg, separator: e.target.value })}
                className="w-16 font-mono"
                size="sm"
              />
            </div>
            <div className="grid grid-cols-2 gap-x-3 gap-y-1.5">
              <label className="flex items-center gap-2">
                <Toggle
                  checked={enCfg.capitalize}
                  onChange={(v) => setEnCfg({ ...enCfg, capitalize: v })}
                />
                <span className="text-muted-foreground">{t("settings.vault.generator.capitalize")}</span>
              </label>
              <label className="flex items-center gap-2">
                <Toggle
                  checked={enCfg.include_number}
                  onChange={(v) => setEnCfg({ ...enCfg, include_number: v })}
                />
                <span className="text-muted-foreground">{t("settings.vault.generator.includeNumber")}</span>
              </label>
            </div>
          </div>
        )}

        {mode === "passphraseZh" && (
          <div className="space-y-2 text-xs">
            <div className="flex items-center gap-2">
              <label className="w-16 text-[11px] uppercase tracking-wide text-muted-foreground/70">
                {t("settings.vault.generator.wordCount")}
              </label>
              <input
                type="number"
                min={2}
                max={10}
                value={zhCfg.word_count}
                onChange={(e) => setZhCfg({ ...zhCfg, word_count: Number(e.target.value) || 4 })}
                className="w-16 rounded-md border border-border bg-background px-2 py-1 text-sm tabular-nums"
              />
              <label className="ml-2 w-16 text-[11px] uppercase tracking-wide text-muted-foreground/70">
                {t("settings.vault.generator.separator")}
              </label>
              <Input
                value={zhCfg.separator}
                onChange={(e) => setZhCfg({ ...zhCfg, separator: e.target.value })}
                className="w-16 font-mono"
                size="sm"
              />
            </div>
            <div className="grid grid-cols-2 gap-x-3 gap-y-1.5">
              <label className="flex items-center gap-2">
                <Toggle
                  checked={zhCfg.include_number}
                  onChange={(v) => setZhCfg({ ...zhCfg, include_number: v })}
                />
                <span className="text-muted-foreground">{t("settings.vault.generator.includeNumber")}</span>
              </label>
              <label className="flex items-center gap-2">
                <Toggle
                  checked={zhCfg.include_symbol}
                  onChange={(v) => setZhCfg({ ...zhCfg, include_symbol: v })}
                />
                <span className="text-muted-foreground">{t("settings.vault.generator.includeSymbol")}</span>
              </label>
            </div>
          </div>
        )}

        {mode === "pin" && (
          <div className="flex items-center gap-2">
            <label className="w-16 text-[11px] uppercase tracking-wide text-muted-foreground/70">
              {t("settings.vault.generator.length")}
            </label>
            <input
              type="range"
              min={4}
              max={12}
              value={pinCfg.length}
              onChange={(e) => setPinCfg({ ...pinCfg, length: Number(e.target.value) })}
              className="flex-1 accent-voice"
            />
            <span className="w-8 text-right text-sm tabular-nums">{pinCfg.length}</span>
          </div>
        )}
      </div>

      {/* 操作栏 */}
      <div className="mt-auto flex gap-2">
        <Button variant="outline" className="flex-1" onClick={regenerate}>
          <RefreshCw />
          {t("settings.vault.generator.regenerate")}
        </Button>
        <Button variant="voice" className="flex-1" onClick={handleCopy}>
          {copied ? <Check className="text-success" /> : <Copy />}
          {t("settings.vault.generator.copy")}
        </Button>
      </div>
    </div>
  );
}
