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
 * 后端 vault_error::serialize 返回的 JSON 字符串：`{ code, message }`。
 * 见 crates/desktop/src/vault_error.rs。任何 reject 都先尝试解出 message，失败
 * 退回 String(err)（向后兼容旧裸字符串错误）。
 */
function extractErrorMessage(raw: unknown): string {
  const str = String(raw).trim();
  if (str.startsWith("{")) {
    try {
      const parsed = JSON.parse(str) as { message?: unknown };
      if (typeof parsed.message === "string" && parsed.message.length > 0) {
        return parsed.message;
      }
    } catch {
      // 落到默认返回
    }
  }
  return str;
}

/** 把 value 钳制到 [min, max]——用于数字输入，避免后端报错。 */
const clamp = (value: number, min: number, max: number): number =>
  Math.max(min, Math.min(max, value));

const Toggle = ({ checked, onChange }: { checked: boolean; onChange: (v: boolean) => void }) => (
  <UIToggle on={checked} onClick={() => onChange(!checked)} />
);

// 简易前端强度评估——基于「生成参数算熵」，不依赖后端 zxcvbn。
//
// 关键：生成器知道自己用的是什么模式 + 配置，直接按参数算熵比从字符串猜
// 准确得多（字符串启发式对中文短语结构性歧视——把中文当 1 类字符 + 长度
// 按字符数算，导致 4 个中文双字词总被评为「极弱」，实际熵有 48 bit）。
//
// 各模式熵（按 Rust 端 generator 实现）：
//   random       length × log2(charset_size)
//                charset = uppercase(26) + lowercase(26) + numbers(10) + symbols(~32)
//   passphraseEn word_count × log2(7776)   // EFF 大词表
//   passphraseZh word_count × log2(4096)   // jieba 双字词词频 TOP
//   pin          length × log2(10)
//
// 评分映射（0-4）：
//   < 28 bit → 0 极弱 / 28-35 → 1 弱 / 36-59 → 2 一般 / 60-127 → 3 强 / ≥ 128 → 4 极强
// 用于生成器内部即时反馈；写回 password 字段后 CipherEditor 的 PasswordStrengthBar
// 会再走后端 vault_evaluate_password 做精确评估。
function estimateStrengthByConfig(
  mode: Mode,
  random: RandomCfg,
  en: EnCfg,
  zh: ZhCfg,
  pin: PinCfg,
): number {
  let bits = 0;
  if (mode === "random") {
    let charset = 0;
    if (random.uppercase) charset += 26;
    if (random.lowercase) charset += 26;
    if (random.numbers) charset += 10;
    if (random.symbols) charset += 32;
    if (charset > 0) bits = random.length * Math.log2(charset);
  } else if (mode === "passphraseEn") {
    bits = en.word_count * Math.log2(7776);
  } else if (mode === "passphraseZh") {
    bits = zh.word_count * Math.log2(4096);
  } else {
    bits = pin.length * Math.log2(10);
  }
  if (bits < 28) return 0;
  if (bits < 36) return 1;
  if (bits < 60) return 2;
  if (bits < 128) return 3;
  return 4;
}

const strengthColor = ["bg-destructive", "bg-amber-500", "bg-yellow-500", "bg-voice", "bg-success"];

/**
 * PasswordGenerator —— 密码生成器主体（跨场景复用）。
 *
 * 纯内容组件，不含外壳。调用方按场景包外壳：
 *   - CipherEditor 场景 → PasswordGeneratorModal 包本组件（onUsePassword 写回字段）
 *   - Actionbar 独立窗口场景（future）→ Tauri 窗口 root 直接渲染本组件（onAutotype 触发自动输入）
 *
 * 4 种生成模式（对应 octopus_vault::generator::GeneratorConfig 变体）：
 *   passphraseZh 中文短语（word_count + separator + include_number + include_symbol）
 *   passphraseEn 英文短语（word_count + separator + capitalize + include_number）
 *   random       随机字符（length + uppercase/lowercase/numbers/symbols/avoid_ambiguous）
 *   pin          数字 PIN（length）
 *
 * 配置组装（mode → 后端 payload）抽到 ./buildConfig.ts，配套单元测试覆盖
 * 各 mode 分发与 serde 约定（见 buildConfig.test.ts）。
 *
 * 切模式 / 改配置即重新生成。
 */
export interface PasswordGeneratorProps {
  /** 「使用此密码」回调——cipher 编辑场景写回 password 字段。提供才显示该按钮。 */
  onUsePassword?: (pwd: string) => void;
  /** 「Auto-type」回调——Actionbar 独立窗口场景触发自动输入到网页（future）。提供才显示该按钮。 */
  onAutotype?: (pwd: string) => void;
  /** 「取消」回调——关闭浮窗/Modal 不做任何操作。提供才显示该按钮。 */
  onCancel?: () => void;
  /** 复制到剪贴板反馈。 */
  showToast: (msg: string) => void;
}

export default function PasswordGenerator({
  onUsePassword,
  onAutotype,
  onCancel,
  showToast,
}: PasswordGeneratorProps) {
  const t = useT();
  const [mode, setMode] = useState<Mode>("random");
  const [result, setResult] = useState("");
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
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
      setErrorMsg(null);
      setCopied(false);
    } catch (e) {
      // 后端返回 {code, message} JSON；解出 message 显示，旧裸字符串向后兼容。
      setResult("");
      setErrorMsg(extractErrorMessage(e));
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
      showToast(t("settings.vault.generator.copy"));
      setTimeout(() => setCopied(false), 2000);
    } catch {
      // 忽略——剪贴板权限失败时静默
    }
  }, [result, showToast, t]);

  const score = useMemo(
    () => estimateStrengthByConfig(mode, randomCfg, enCfg, zhCfg, pinCfg),
    [mode, randomCfg, enCfg, zhCfg, pinCfg],
  );

  return (
    <div className="flex flex-col gap-3">
      {/* 模式切换 */}
      <Segmented
        items={[
          { key: "random", label: t("settings.vault.generator.mode.random") },
          { key: "passphraseEn", label: t("settings.vault.generator.mode.passphraseEn") },
          { key: "passphraseZh", label: t("settings.vault.generator.mode.passphraseZh") },
          { key: "pin", label: t("settings.vault.generator.mode.pin") },
        ]}
        active={mode}
        onChange={(k) => setMode(k as Mode)}
      />

      {/* 显示区 */}
      <div className="min-h-[72px] select-text break-all rounded-md border border-border bg-muted/30 p-3 font-mono text-lg">
        {errorMsg ? (
          <span className="font-sans text-sm text-destructive">{errorMsg}</span>
        ) : (
          result || "..."
        )}
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
                min={5}
                max={128}
                value={randomCfg.length}
                onChange={(e) =>
                  setRandomCfg({
                    ...randomCfg,
                    length: clamp(Number(e.target.value), 5, 128),
                  })
                }
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
                    onChange={(v) => {
                      // 字符类型 toggle：uppercase/lowercase/numbers/symbols 至少要保留一个。
                      // avoid_ambiguous 是过滤开关，不参与字符集构成，可独立切换。
                      const isCharsetFlag =
                        key === "uppercase" ||
                        key === "lowercase" ||
                        key === "numbers" ||
                        key === "symbols";
                      if (!v && isCharsetFlag) {
                        const othersOn =
                          (key !== "uppercase" && randomCfg.uppercase) ||
                          (key !== "lowercase" && randomCfg.lowercase) ||
                          (key !== "numbers" && randomCfg.numbers) ||
                          (key !== "symbols" && randomCfg.symbols);
                        if (!othersOn) {
                          // 全关会让后端 charset 为空——拒绝这次切换，保留至少一个。
                          return;
                        }
                      }
                      setRandomCfg({ ...randomCfg, [key]: v });
                    }}
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
                min={3}
                max={10}
                value={enCfg.word_count}
                onChange={(e) =>
                  setEnCfg({
                    ...enCfg,
                    word_count: clamp(Number(e.target.value) || 3, 3, 10),
                  })
                }
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
                min={3}
                max={8}
                value={zhCfg.word_count}
                onChange={(e) =>
                  setZhCfg({
                    ...zhCfg,
                    word_count: clamp(Number(e.target.value) || 4, 3, 8),
                  })
                }
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
              min={1}
              max={32}
              value={pinCfg.length}
              onChange={(e) =>
                setPinCfg({ ...pinCfg, length: clamp(Number(e.target.value), 1, 32) })
              }
              className="flex-1 accent-voice"
            />
            <span className="w-8 text-right text-sm tabular-nums">{pinCfg.length}</span>
          </div>
        )}
      </div>

      {/* 操作栏——取消（如有）独立左侧，其他按钮按 props 动态显示 */}
      <div className="flex gap-2">
        {onCancel && (
          <Button variant="ghost" onClick={onCancel}>
            {t("settings.vault.editor.cancel")}
          </Button>
        )}
        <Button variant="outline" className="flex-1" onClick={regenerate}>
          <RefreshCw />
          {t("settings.vault.generator.regenerate")}
        </Button>
        <Button variant="outline" className="flex-1" onClick={handleCopy} disabled={!result}>
          {copied ? <Check className="text-success" /> : <Copy />}
          {t("settings.vault.generator.copy")}
        </Button>
        {onUsePassword && (
          <Button
            variant="voice"
            className="flex-1"
            onClick={() => onUsePassword(result)}
            disabled={!result}
          >
            {t("settings.vault.editor.useGenerated")}
          </Button>
        )}
        {onAutotype && (
          <Button
            variant="voice"
            className="flex-1"
            onClick={() => onAutotype(result)}
            disabled={!result}
          >
            {t("settings.vault.autotype.trigger")}
          </Button>
        )}
      </div>
    </div>
  );
}
