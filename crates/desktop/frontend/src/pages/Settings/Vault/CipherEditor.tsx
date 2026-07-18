import { useCallback, useEffect, useState } from "react";
import * as React from "react";
import { invoke } from "@tauri-apps/api/core";
import { ArrowLeft, RefreshCw } from "lucide-react";
import { useT } from "@/lib/i18n";
import { Button } from "@/components/ui/button";
import { Input, Select, Textarea } from "@/components/ui/input";
import {
  buildPayload,
  DEFAULT_RANDOM,
  DEFAULT_EN,
  DEFAULT_ZH,
  DEFAULT_PIN,
  type Mode,
} from "./buildConfig";
import type { CipherDto } from "./CipherList";
import type { FolderDto } from "./folderTypes";

/**
 * CipherEditor —— 新建/编辑一条 Login cipher。
 *
 * 字段：name / urls(多行) / username / password / totp / notes / favorite。
 * 保存：新建 → vault_create_cipher；编辑 → vault_update_cipher。
 * 删除：软删除（permanent=false）；已软删时显示「永久删除」（permanent=true）。
 *
 * 后端 CipherInputDto.login 必填，所以即便用户没填任何 login 字段，
 * 也至少传一个空 LoginDataDto（uris/username/password/totp 全可空）。
 *
 * 设计（frontend-design skill, "Identity Card" 范式）：
 *   - 顶部「身份卡头部」：站点色块 + 首字母 + name/url + ★
 *   - 重点字段（username / password）单列大块 + 复制按钮 + 显示/隐藏
 *   - 密码强度条：debounce 300ms 调 vault_evaluate_password
 *   - 生成器：password 下方抽屉式展开，不再独立弹层
 *   - TOTP 卡：secret 输入 + 实时验证码大字号（24px tracking）整合为一张卡
 *   - 高级（urls / folder / notes）默认折叠在 ▸ 高级 下
 */

interface LoginUriDto {
  uri: string;
  match_type: number | null;
}
interface LoginDataDto {
  uris: LoginUriDto[];
  username: string | null;
  password: string | null;
  totp: string | null;
}
interface FieldDto {
  name: string;
  value: string | null;
  field_type: number;
}
interface CipherInputDto {
  folder_id: number | null;
  favorite: boolean;
  name: string;
  notes: string | null;
  login: LoginDataDto;
  fields: FieldDto[];
  reprompt: number | null;
}

interface TotpResult {
  code: string;
  seconds_remaining: number;
}

interface PasswordStrength {
  score: number; // 0-4
  entropy_bits: number;
}

/**
 * useTotpPoller —— 对指定 cipher 轮询 TOTP code。
 *
 * 每 5s 调一次 `vault_generate_totp`，并返回 `{ code, seconds_remaining }`。
 * - `cipherId === null` 或后端返回无 totp secret 时返回 `null`。
 * - 卸载 / cipherId 变化时自动 cleanup。
 *
 * 不在 form 层做 30s concealed clear——TOTP 本身就 30s 轮换，无需像 password 那样保护。
 * 复制按钮直接 `navigator.clipboard.writeText`，与 vault_copy_password 模式分离。
 * （follow-up #5）
 */
function useTotpPoller(cipherId: number | null): TotpResult | null {
  const [result, setResult] = useState<TotpResult | null>(null);

  useEffect(() => {
    if (cipherId === null) {
      setResult(null);
      return;
    }
    let cancelled = false;

    async function fetchTotp() {
      try {
        const r = await invoke<TotpResult>("vault_generate_totp", { cipherId });
        if (!cancelled) setResult(r);
      } catch {
        // 无 totp secret / cipher 不存在 / vault 锁定 → 静默置空
        if (!cancelled) setResult(null);
      }
    }

    fetchTotp();
    const interval = setInterval(fetchTotp, 5000);
    return () => {
      cancelled = true;
      clearInterval(interval);
    };
  }, [cipherId]);

  return result;
}

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

// === 子组件：FieldLabel / CopyButton / PasswordStrengthBar ===

/**
 * 统一的字段小标签：小号 / 大写 / tracking / muted。
 * 与 settings 其他面板的 label 样式一致，避免引入新视觉变量。
 */
function FieldLabel({ children }: { children: React.ReactNode }) {
  return (
    <label className="block text-[11px] font-medium uppercase tracking-wide text-muted-foreground/80">
      {children}
    </label>
  );
}

/**
 * 复制按钮——绝对定位在 Input 右侧。空文本时返回 null（避免点击复制空字符串）。
 * 复制成功走 showToast 反馈。`showToast` 由调用方注入（保持与父级 toast 同源）。
 */
function CopyButton({
  text,
  showToast,
  t,
  labelKey,
}: {
  text: string;
  showToast: (msg: string) => void;
  t: (k: string) => string;
  labelKey: string;
}) {
  if (!text) return null;
  return (
    <button
      type="button"
      onClick={async () => {
        try {
          await navigator.clipboard.writeText(text);
          showToast(t("settings.vault.editor.copied"));
        } catch {
          // 剪贴板被拒（无焦点等）→ 静默
        }
      }}
      className="absolute right-1 top-1/2 -translate-y-1/2 px-1.5 py-1 text-muted-foreground transition-colors hover:text-foreground"
      title={t(labelKey)}
    >
      ⎘
    </button>
  );
}

/**
 * 密码强度条——debounce 300ms 后调后端 vault_evaluate_password。
 * 0-4 → 20/40/60/80/100% 填充，颜色 destructive(0-1) / warning(2) / success(3-4)，
 * 配 极弱/弱/中/强/极强 小标签。
 *
 * 自管 state，父组件无需关心。卸载时清 timer。
 * 后端命令失败（如未启用 vault feature）→ 静默置空不渲染。
 */
function PasswordStrengthBar({
  password,
  showToast,
}: {
  password: string;
  showToast: (msg: string) => void;
}) {
  const t = useT();
  const [strength, setStrength] = useState<PasswordStrength | null>(null);

  useEffect(() => {
    if (!password) {
      setStrength(null);
      return;
    }
    // debounce 300ms——避免每键都跑 zxcvbn。
    const timer = setTimeout(() => {
      invoke<PasswordStrength>("vault_evaluate_password", { password })
        .then(setStrength)
        .catch((e) => {
          // 后端命令不可用（vault feature 未启用等）→ 静默，不刷 toast 干扰。
          // 仅在非典型错误下提示（保留向后兼容的诊断路径）。
          const msg = extractErrorMessage(e);
          if (msg && !msg.includes("not found") && !msg.includes("missing")) {
            showToast(msg);
          }
          setStrength(null);
        });
    }, 300);
    return () => clearTimeout(timer);
  }, [password, showToast]);

  if (!strength) return null;

  const pct = (strength.score + 1) * 20; // 20/40/60/80/100
  const color =
    strength.score <= 1
      ? "bg-destructive"
      : strength.score === 2
        ? "bg-warning"
        : "bg-success";
  const label =
    strength.score === 0
      ? t("settings.vault.generator.strengthLevels.0")
      : strength.score === 1
        ? t("settings.vault.generator.strengthLevels.1")
        : strength.score === 2
          ? t("settings.vault.generator.strengthLevels.2")
          : strength.score === 3
            ? t("settings.vault.generator.strengthLevels.3")
            : t("settings.vault.generator.strengthLevels.4");

  return (
    <div className="flex items-center gap-2 pt-1">
      <div className="h-1 flex-1 overflow-hidden rounded-full bg-muted">
        <div
          className={`h-full transition-all ${color}`}
          style={{ width: `${pct}%` }}
        />
      </div>
      <span className="text-[10px] text-muted-foreground">{label}</span>
    </div>
  );
}

// === Avatar 色块 ===

/**
 * 由字符串（首 URL 或 name）派生稳定色相。
 * 31 倍乘法 + charCode 累加 → 取模 360。同站点/同名 → 同色（跨会话稳定）。
 */
function deriveAvatarHue(input: string): number {
  let h = 0;
  for (let i = 0; i < input.length; i++) {
    h = (h * 31 + input.charCodeAt(i)) & 0xffffffff;
  }
  return Math.abs(h) % 360;
}

/**
 * 头像 inline style——低饱和（25%）HSL，文字白色。
 * 低饱和保证在浅色/深色主题下都不刺眼。
 */
function avatarStyle(input: string): React.CSSProperties {
  const hue = deriveAvatarHue(input);
  return {
    backgroundColor: `hsl(${hue}, 25%, 50%)`,
    color: "white",
  };
}

export default function CipherEditor({
  cipherId,
  folders,
  onClose,
  showToast,
}: {
  cipherId: number | null;
  folders: FolderDto[];
  onClose: () => Promise<void>;
  showToast: (msg: string) => void;
}) {
  const t = useT();
  const [name, setName] = useState("");
  const [urls, setUrls] = useState(""); // 多行，每行一个 uri
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [totp, setTotp] = useState("");
  const [notes, setNotes] = useState("");
  const [favorite, setFavorite] = useState(false);
  // follow-up #6：folder_id 状态（null = 无 folder / 根目录）
  const [folderId, setFolderId] = useState<number | null>(null);
  const [deletedAt, setDeletedAt] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [loaded, setLoaded] = useState(cipherId === null); // 新建默认 loaded

  // 新增 UI 状态：密码可见性、高级折叠
  const [showPassword, setShowPassword] = useState(false);
  const [showAdvanced, setShowAdvanced] = useState(false);

  // 内嵌密码生成器：点击「生成」按钮弹出 inline 面板，确认后写回 password 字段。
  // 简化版——固定使用各模式默认配置，不暴露 length/wordCount 等可调项。
  // 复杂配置需求由独立浮窗时代过去；这里只覆盖「快速生成一个强密码」主路径。
  const [showGenerator, setShowGenerator] = useState(false);
  const [genMode, setGenMode] = useState<Mode>("passphraseZh");
  const [genResult, setGenResult] = useState<string>("");
  const [genBusy, setGenBusy] = useState(false);

  const regenerate = useCallback(async (mode: Mode) => {
    setGenBusy(true);
    try {
      // buildPayload 入参为 (mode, random, en, zh, pin)；此处全部传默认值，
      // 仅切换 mode 决定走哪个变体。Rust 端 #[serde(tag="mode")] 反序列化。
      const payload = buildPayload(mode, DEFAULT_RANDOM, DEFAULT_EN, DEFAULT_ZH, DEFAULT_PIN);
      const pwd = await invoke<string>("vault_generate", { cfg: payload });
      setGenResult(pwd);
    } catch (e) {
      showToast(extractErrorMessage(e));
    } finally {
      setGenBusy(false);
    }
  }, [showToast]);

  const applyGenerated = useCallback(() => {
    setPassword(genResult);
    setShowGenerator(false);
  }, [genResult]);

  // follow-up #5: 对已保存 cipher 轮询 TOTP code（仅显示，不参与表单提交）
  const totpResult = useTotpPoller(cipherId);

  const loadCipher = useCallback(async () => {
    if (cipherId === null) return;
    try {
      const c = await invoke<CipherDto>("vault_get_cipher", { id: cipherId });
      setName(c.name);
      setUrls((c.login?.uris ?? []).map((u) => u.uri).join("\n"));
      setUsername(c.login?.username ?? "");
      setPassword(c.login?.password ?? "");
      setTotp(c.login?.totp ?? "");
      setNotes(c.notes ?? "");
      setFavorite(c.favorite);
      setFolderId(c.folder_id);
      setDeletedAt(c.deleted_at);
    } catch (e) {
      showToast(String(e));
    }
    setLoaded(true);
  }, [cipherId, showToast]);

  useEffect(() => {
    loadCipher();
  }, [loadCipher]);

  function buildInput(): CipherInputDto {
    const uris = urls
      .split("\n")
      .map((s) => s.trim())
      .filter((s) => s.length > 0)
      .map((uri) => ({ uri, match_type: null }));
    return {
      folder_id: folderId,
      favorite,
      name: name.trim() || "(untitled)",
      notes: notes.trim() || null,
      login: {
        uris,
        username: username.trim() || null,
        password: password || null,
        totp: totp.trim() || null,
      },
      fields: [],
      reprompt: 0,
    };
  }

  const handleSave = useCallback(async () => {
    if (!name.trim()) {
      showToast(t("settings.vault.editor.nameLabel"));
      return;
    }
    setBusy(true);
    try {
      const input = buildInput();
      if (cipherId === null) {
        await invoke("vault_create_cipher", { input });
      } else {
        await invoke("vault_update_cipher", { id: cipherId, input });
      }
      showToast(t("settings.vault.editor.save"));
      await onClose();
    } catch (e) {
      showToast(String(e));
    } finally {
      setBusy(false);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [cipherId, name, urls, username, password, totp, notes, favorite, folderId, showToast, onClose, t]);

  const handleDelete = useCallback(
    async (permanent: boolean) => {
      if (cipherId === null) return;
      setBusy(true);
      try {
        await invoke("vault_delete_cipher", { id: cipherId, permanent });
        showToast(permanent ? t("settings.vault.editor.permanentDelete") : t("settings.vault.editor.delete"));
        await onClose();
      } catch (e) {
        showToast(String(e));
      } finally {
        setBusy(false);
      }
    },
    [cipherId, onClose, showToast, t],
  );

  if (!loaded) {
    return (
      <div className="flex h-full items-center justify-center text-sm text-muted-foreground">
        {t("settings.loading")}
      </div>
    );
  }

  // 头像色块输入：优先首 URL（站点身份更稳定），无则回退 name。
  const firstUrl = urls.split("\n")[0]?.trim() ?? "";
  const avatarInput = firstUrl || name || "?";

  return (
    <div className="flex h-full flex-col gap-3">
      {/* 顶部导航 */}
      <div className="flex shrink-0 items-center gap-3 border-b border-border/40 pb-2">
        <button
          onClick={() => onClose()}
          className="flex items-center gap-1.5 text-xs text-muted-foreground transition-colors hover:text-foreground"
        >
          <ArrowLeft className="size-3.5" />
          {t("settings.vault.list.title")}
        </button>
        <span className="font-mono text-[10px] uppercase tracking-widest text-muted-foreground/70">
          {cipherId === null ? "NEW" : "EDIT"}
        </span>
      </div>

      {/* 主卡片 */}
      <div className="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto rounded-lg border border-border/50 bg-muted/15 p-4">
        {/* 身份卡头部 */}
        <div className="flex items-start gap-3 pb-3">
          <div
            className="flex size-10 shrink-0 items-center justify-center rounded-md text-base font-semibold"
            style={avatarStyle(avatarInput)}
          >
            {(name || "?").charAt(0).toUpperCase()}
          </div>
          <div className="min-w-0 flex-1">
            <Input
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder={t("settings.vault.editor.nameLabel")}
              className="border-0 bg-transparent px-0 font-mono-vault text-base font-medium focus-visible:ring-0"
              autoFocus
            />
            {firstUrl && (
              <div className="truncate font-mono-vault text-xs text-muted-foreground">
                {firstUrl}
              </div>
            )}
          </div>
          <button
            type="button"
            onClick={() => setFavorite(!favorite)}
            className={
              favorite
                ? "px-1 text-foreground"
                : "px-1 text-muted-foreground/40 transition-colors hover:text-foreground"
            }
            title={t("settings.vault.editor.favoriteLabel")}
          >
            ★
          </button>
        </div>

        {/* 用户名 */}
        <div className="space-y-1">
          <FieldLabel>{t("settings.vault.editor.usernameLabel")}</FieldLabel>
          <div className="relative">
            <Input
              value={username}
              onChange={(e) => setUsername(e.target.value)}
              className="w-full font-mono-vault pr-9"
              autoComplete="off"
            />
            <CopyButton
              text={username}
              showToast={showToast}
              t={t}
              labelKey="settings.vault.editor.copy"
            />
          </div>
        </div>

        {/* 密码 */}
        <div className="space-y-1">
          <div className="flex items-center justify-between">
            <FieldLabel>{t("settings.vault.editor.passwordLabel")}</FieldLabel>
            <button
              type="button"
              onClick={() => {
                const next = !showGenerator;
                setShowGenerator(next);
                // 首次展开且尚无结果 → 立即生成一次，避免空面板。
                if (next && !genResult) {
                  regenerate(genMode);
                }
              }}
              className="text-[11px] text-muted-foreground transition-colors hover:text-foreground"
            >
              {t("settings.vault.editor.generate")}
            </button>
          </div>
          <div className="relative">
            <Input
              type={showPassword ? "text" : "password"}
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              className="w-full font-mono-vault px-16"
              autoComplete="off"
            />
            <div className="absolute right-1 top-1/2 flex -translate-y-1/2">
              <button
                type="button"
                onClick={() => setShowPassword(!showPassword)}
                className="px-1.5 py-1 text-muted-foreground transition-colors hover:text-foreground"
                title={
                  showPassword
                    ? t("settings.vault.editor.hide")
                    : t("settings.vault.editor.show")
                }
              >
                {showPassword ? "🙈" : "👁"}
              </button>
              <CopyButton
                text={password}
                showToast={showToast}
                t={t}
                labelKey="settings.vault.editor.copy"
              />
            </div>
          </div>
          {/* 强度条 */}
          <PasswordStrengthBar password={password} showToast={showToast} />
        </div>

        {/* 生成器抽屉 */}
        {showGenerator && (
          <div className="ml-2 border-l border-border/40 pl-3">
            <div className="flex items-center gap-2 py-1.5">
              <select
                value={genMode}
                onChange={(e) => {
                  const m = e.target.value as Mode;
                  setGenMode(m);
                  // 切模式立即重生成——避免显示旧模式的结果造成误解。
                  regenerate(m);
                }}
                className="rounded border border-border bg-background px-2 py-0.5 text-[11px]"
              >
                <option value="passphraseZh">{t("settings.vault.generator.mode.passphraseZh")}</option>
                <option value="passphraseEn">{t("settings.vault.generator.mode.passphraseEn")}</option>
                <option value="random">{t("settings.vault.generator.mode.random")}</option>
                <option value="pin">{t("settings.vault.generator.mode.pin")}</option>
              </select>
              <button
                type="button"
                onClick={() => regenerate(genMode)}
                disabled={genBusy}
                className="px-1.5 py-0.5 text-muted-foreground transition-colors hover:text-foreground disabled:opacity-50"
                title={t("settings.vault.generator.regenerate")}
              >
                <RefreshCw className="size-3" />
              </button>
              <code className="flex-1 break-all font-mono-vault text-xs">
                {genBusy ? "..." : genResult}
              </code>
              <button
                type="button"
                onClick={applyGenerated}
                disabled={!genResult || genBusy}
                className="rounded bg-foreground px-2 py-0.5 text-[11px] text-background transition-opacity hover:opacity-90 disabled:opacity-50"
              >
                {t("settings.vault.editor.useGenerated")}
              </button>
            </div>
          </div>
        )}

        {/* TOTP 卡 */}
        <div className="space-y-1.5">
          <div className="flex items-center gap-2 text-[10px] uppercase tracking-widest text-muted-foreground/60">
            <span className="h-px flex-1 bg-border" />
            TOTP
            <span className="h-px flex-1 bg-border" />
          </div>
          <Input
            value={totp}
            onChange={(e) => setTotp(e.target.value)}
            placeholder="JBSWY3DPEHPK3PXP"
            className="w-full font-mono-vault"
            autoComplete="off"
          />
          {totpResult && (
            <div className="flex items-center gap-3 rounded-md bg-muted/30 p-2.5">
              <span className="font-mono-vault text-2xl tracking-[0.3em] tabular-nums">
                {totpResult.code.slice(0, 3)} {totpResult.code.slice(3)}
              </span>
              <div className="ml-auto flex items-center gap-2 text-xs text-muted-foreground">
                <span>⏱ {totpResult.seconds_remaining}s</span>
                <button
                  type="button"
                  onClick={async () => {
                    try {
                      await navigator.clipboard.writeText(totpResult.code);
                      showToast(t("settings.vault.totp.copyCode"));
                    } catch {
                      // 剪贴板拒绝 → 静默
                    }
                  }}
                  className="transition-colors hover:text-foreground"
                  title={t("settings.vault.totp.copyCode")}
                >
                  ⎘
                </button>
              </div>
            </div>
          )}
        </div>

        {/* 高级（折叠） */}
        <button
          type="button"
          onClick={() => setShowAdvanced(!showAdvanced)}
          className="flex w-full items-center gap-1 text-[10px] uppercase tracking-widest text-muted-foreground/60 transition-colors hover:text-foreground"
        >
          <span>{showAdvanced ? "▾" : "▸"}</span>
          {t("settings.vault.editor.advanced")}
        </button>
        {showAdvanced && (
          <div className="space-y-3">
            {/* URLs */}
            <div className="space-y-1">
              <FieldLabel>{t("settings.vault.editor.urlLabel")}</FieldLabel>
              <Textarea
                value={urls}
                onChange={(e) => setUrls(e.target.value)}
                className="min-h-[60px] w-full resize-y font-mono-vault text-xs"
                placeholder={"https://example.com/login"}
              />
            </div>
            {/* Folder */}
            <div className="space-y-1">
              <FieldLabel>{t("settings.vault.editor.folderLabel")}</FieldLabel>
              <Select
                value={folderId?.toString() ?? ""}
                onChange={(e) =>
                  setFolderId(e.target.value ? Number(e.target.value) : null)
                }
                className="w-full"
              >
                <option value="">{t("settings.vault.folder.folderNone")}</option>
                {folders.map((f) => (
                  <option key={f.id} value={f.id.toString()}>
                    {f.name}
                  </option>
                ))}
              </Select>
            </div>
            {/* Notes */}
            <div className="space-y-1">
              <FieldLabel>{t("settings.vault.editor.notesLabel")}</FieldLabel>
              <Textarea
                value={notes}
                onChange={(e) => setNotes(e.target.value)}
                className="min-h-[60px] w-full resize-y"
              />
            </div>
          </div>
        )}
      </div>

      {/* 底部按钮 */}
      <div className="flex shrink-0 items-center justify-between gap-2.5">
        <div>
          {cipherId !== null && (
            <Button
              variant="destructive-ghost"
              size="sm"
              disabled={busy}
              onClick={() => handleDelete(false)}
            >
              {deletedAt
                ? t("settings.vault.editor.permanentDelete")
                : t("settings.vault.editor.delete")}
            </Button>
          )}
        </div>
        <div className="flex gap-2.5">
          <Button variant="outline" size="sm" onClick={() => onClose()} disabled={busy}>
            {t("settings.vault.editor.cancel")}
          </Button>
          <Button variant="voice" size="sm" onClick={handleSave} disabled={busy}>
            {busy ? "..." : t("settings.vault.editor.save")}
          </Button>
        </div>
      </div>
    </div>
  );
}
