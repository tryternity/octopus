import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ArrowLeft, RefreshCw } from "lucide-react";
import { cn } from "@/lib/utils";
import { useT } from "@/lib/i18n";
import { Button } from "@/components/ui/button";
import { Input, Select, Textarea } from "@/components/ui/input";
import { Toggle as UIToggle } from "@/components/ui/toggle";
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

// Toggle 适配：把 ActionBarPanel 范式 (checked, onChange) 转到共享 UIToggle (on, onClick)。
const Toggle = ({ checked, onChange }: { checked: boolean; onChange: (v: boolean) => void }) => (
  <UIToggle on={checked} onClick={() => onChange(!checked)} />
);

const inputCls = "w-full";

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

  return (
    <div className="flex h-full flex-col gap-3">
      {/* 导航栏 */}
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

      {/* 表单卡片 */}
      <div className="flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto rounded-lg border border-border/50 bg-muted/15 p-4">
        <div className="space-y-1.5">
          <label className="block text-[11px] font-medium uppercase tracking-wide text-muted-foreground/80">
            {t("settings.vault.editor.nameLabel")}
          </label>
          <Input value={name} onChange={(e) => setName(e.target.value)} className={inputCls} autoFocus />
        </div>

        <div className="space-y-1.5">
          <label className="block text-[11px] font-medium uppercase tracking-wide text-muted-foreground/80">
            {t("settings.vault.editor.folderLabel")}
          </label>
          <Select
            value={folderId?.toString() ?? ""}
            onChange={(e) =>
              setFolderId(e.target.value ? Number(e.target.value) : null)
            }
            className={inputCls}
          >
            <option value="">{t("settings.vault.folder.folderNone")}</option>
            {folders.map((f) => (
              <option key={f.id} value={f.id.toString()}>
                {f.name}
              </option>
            ))}
          </Select>
        </div>

        <div className="space-y-1.5">
          <label className="block text-[11px] font-medium uppercase tracking-wide text-muted-foreground/80">
            {t("settings.vault.editor.urlLabel")}
          </label>
          <Textarea
            value={urls}
            onChange={(e) => setUrls(e.target.value)}
            className={cn(inputCls, "min-h-[60px] resize-y font-mono text-xs")}
            placeholder={"https://example.com/login"}
          />
        </div>

        <div className="grid grid-cols-2 gap-3">
          <div className="space-y-1.5">
            <label className="block text-[11px] font-medium uppercase tracking-wide text-muted-foreground/80">
              {t("settings.vault.editor.usernameLabel")}
            </label>
            <Input value={username} onChange={(e) => setUsername(e.target.value)} className={inputCls} />
          </div>
          <div className="space-y-1.5">
            <div className="flex items-center justify-between">
              <label className="block text-[11px] font-medium uppercase tracking-wide text-muted-foreground/80">
                {t("settings.vault.editor.passwordLabel")}
              </label>
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
                className="text-xs text-primary hover:underline"
              >
                {t("settings.vault.editor.generate")}
              </button>
            </div>
            <Input
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              className={cn(inputCls, "font-mono")}
              type="text"
              autoComplete="off"
            />
            {showGenerator && (
              <div className="space-y-2 rounded-md border border-border/50 bg-card p-3">
                <div className="flex items-center gap-2">
                  <select
                    value={genMode}
                    onChange={(e) => {
                      const m = e.target.value as Mode;
                      setGenMode(m);
                      // 切模式立即重生成——避免显示旧模式的结果造成误解。
                      regenerate(m);
                    }}
                    className="rounded-md border border-border bg-background px-2 py-1 text-xs"
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
                    className="rounded-md border border-border px-2 py-1 text-xs hover:bg-muted disabled:opacity-50"
                    title={t("settings.vault.generator.regenerate")}
                  >
                    <RefreshCw className="size-3.5" />
                  </button>
                  <code className="flex-1 break-all font-mono text-sm">
                    {genBusy ? "..." : genResult}
                  </code>
                </div>
                <div className="flex justify-end gap-2">
                  <button
                    type="button"
                    onClick={() => setShowGenerator(false)}
                    className="px-2 py-1 text-xs text-muted-foreground hover:underline"
                  >
                    {t("settings.vault.editor.cancel")}
                  </button>
                  <button
                    type="button"
                    onClick={applyGenerated}
                    disabled={!genResult || genBusy}
                    className="rounded-md bg-primary px-2 py-1 text-xs text-primary-foreground hover:opacity-90 disabled:opacity-50"
                  >
                    {t("settings.vault.editor.useGenerated")}
                  </button>
                </div>
              </div>
            )}
          </div>
        </div>

        <div className="space-y-1.5">
          <label className="block text-[11px] font-medium uppercase tracking-wide text-muted-foreground/80">
            {t("settings.vault.editor.totpLabel")}
          </label>
          <Input
            value={totp}
            onChange={(e) => setTotp(e.target.value)}
            className={cn(inputCls, "font-mono")}
            placeholder="JBSWY3DPEHPK3PXP"
            autoComplete="off"
          />
          {totpResult && (
            <div className="flex items-center gap-2 pt-1">
              <span className="font-mono text-lg tracking-widest">{totpResult.code}</span>
              <span className="text-xs text-muted-foreground">
                ({totpResult.seconds_remaining}s)
              </span>
              <button
                type="button"
                onClick={async () => {
                  try {
                    await navigator.clipboard.writeText(totpResult.code);
                    showToast(t("settings.vault.totp.copyCode"));
                  } catch (e) {
                    showToast(String(e));
                  }
                }}
                className="ml-2 text-xs underline"
              >
                {t("settings.vault.totp.copyCode")}
              </button>
            </div>
          )}
        </div>

        <div className="space-y-1.5">
          <label className="block text-[11px] font-medium uppercase tracking-wide text-muted-foreground/80">
            {t("settings.vault.editor.notesLabel")}
          </label>
          <Textarea
            value={notes}
            onChange={(e) => setNotes(e.target.value)}
            className={cn(inputCls, "min-h-[60px] resize-y")}
          />
        </div>

        <div className="flex items-center gap-2.5">
          <Toggle checked={favorite} onChange={setFavorite} />
          <span className="text-xs text-muted-foreground">
            {t("settings.vault.editor.favoriteLabel")}
          </span>
        </div>
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
