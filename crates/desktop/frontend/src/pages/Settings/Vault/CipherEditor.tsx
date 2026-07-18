import { useCallback, useEffect, useState } from "react";
import * as React from "react";
import { invoke } from "@tauri-apps/api/core";
import { ArrowLeft } from "lucide-react";
import { useT } from "@/lib/i18n";
import { Button } from "@/components/ui/button";
import { Input, Select, Textarea } from "@/components/ui/input";
import PasswordGeneratorModal from "./PasswordGeneratorModal";
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
 *   - 用户名 | 密码 同行（grid-cols-2）；密码右侧三按钮：眼睛/生成/复制
 *   - TOTP | 文件夹 同行（grid-cols-2）
 *   - 网址 / 备注 各占整行（textarea resize-y）
 *   - 密码强度条：debounce 300ms 调 vault_evaluate_password
 *   - 生成器：点 🔑 弹 PasswordGeneratorModal（主体复用 PasswordGenerator 组件，
 *     未来 Actionbar 独立窗口场景也能复用同一主体）
 *   - urls / folder / notes 平铺展示（无折叠）
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
 * 统一的 inline SVG icon——尺寸/对齐/颜色继承父级 currentColor。
 * 用 `<img src>` 引 public/icons/*.svg（Font Awesome by @fontawesome）。
 *
 * 用法：`<IconImg name="copy" className="size-3.5" />`
 */
function IconImg({
  name,
  className = "size-4",
  alt = "",
}: {
  name: "copy" | "see-eye" | "generate-key";
  className?: string;
  alt?: string;
}) {
  return (
    <img
      src={`/icons/${name}.svg`}
      alt={alt}
      className={`shrink-0 select-none ${className}`}
      draggable={false}
    />
  );
}

/**
 * 复制按钮。空文本时返回 null（避免点击复制空字符串）。
 * 复制成功走 showToast 反馈。`showToast` 由调用方注入（保持与父级 toast 同源）。
 *
 * 定位：默认 `absolute right-1 top-1/2 -translate-y-1/2`——单独贴 Input 右侧
 * （username 字段）。密码字段右侧有多个按钮并排（眼睛 + 复制），改走 flex 流，
 * 调用方传 `className` 覆盖默认 absolute 样式，否则 absolute 会脱离 flex 容器
 * 覆盖到眼睛按钮上（曾出现 bug：点眼睛触发复制）。
 */
function CopyButton({
  text,
  showToast,
  t,
  labelKey,
  className,
}: {
  text: string;
  showToast: (msg: string) => void;
  t: (k: string) => string;
  labelKey: string;
  className?: string;
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
      className={
        className ??
        "absolute right-1 top-1/2 flex -translate-y-1/2 items-center px-1.5 py-1 text-muted-foreground transition-colors hover:text-foreground"
      }
      title={t(labelKey)}
    >
      <IconImg name="copy" className="size-3.5" alt={t(labelKey)} />
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

  // 密码可见性
  const [showPassword, setShowPassword] = useState(false);

  // 密码生成器 modal——点密码字段右侧 🔑 弹出 PasswordGeneratorModal。
  // 生成器主体（模式/配置/强度）封装在 PasswordGenerator 组件里，跨场景复用。
  const [showGenerator, setShowGenerator] = useState(false);

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
      {/* 密码生成器 modal——fixed 定位，脱离文档流覆盖整窗。
          点密码字段右侧 🔑 弹出，onUsePassword 写回 password 字段。 */}
      <PasswordGeneratorModal
        open={showGenerator}
        onClose={() => setShowGenerator(false)}
        onUsePassword={(pwd) => setPassword(pwd)}
        showToast={showToast}
      />

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
      <div className="flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto rounded-lg border border-border/50 bg-muted/15 p-4">
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

        {/* 行：用户名 | 密码（两列等宽） */}
        <div className="grid grid-cols-2 gap-3">
          {/* 用户名 */}
          <div className="min-w-0 space-y-1">
            <FieldLabel>{t("settings.vault.editor.usernameLabel")}</FieldLabel>
            <div className="relative">
              <Input
                size="full"
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

          {/* 密码 + 强度条（强度条只跟密码相关，放同列） */}
          <div className="min-w-0 space-y-1">
            <FieldLabel>{t("settings.vault.editor.passwordLabel")}</FieldLabel>
            <div className="relative">
              <Input
                size="full"
                type={showPassword ? "text" : "password"}
                value={password}
                onChange={(e) => setPassword(e.target.value)}
                className="w-full font-mono-vault pr-24"
                autoComplete="off"
              />
              {/* 三按钮并排：眼睛（显示） / 生成（toggle 抽屉） / 复制。
                  统一 px-1.5 py-1 + size-3.5，激活态用 text-foreground。 */}
              <div className="absolute right-1 top-1/2 flex -translate-y-1/2 items-center gap-0.5">
                <button
                  type="button"
                  onClick={() => setShowPassword(!showPassword)}
                  className={`flex items-center rounded px-1.5 py-1 transition-colors hover:bg-accent ${
                    showPassword ? "text-foreground" : "text-muted-foreground"
                  }`}
                  title={
                    showPassword
                      ? t("settings.vault.editor.hide")
                      : t("settings.vault.editor.show")
                  }
                >
                  <IconImg name="see-eye" className="size-3.5" />
                </button>
                <button
                  type="button"
                  onClick={() => setShowGenerator(true)}
                  className="flex items-center rounded px-1.5 py-1 text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
                  title={t("settings.vault.editor.generate")}
                >
                  <IconImg name="generate-key" className="size-3.5" />
                </button>
                <CopyButton
                  text={password}
                  showToast={showToast}
                  t={t}
                  labelKey="settings.vault.editor.copy"
                  className="flex items-center px-1.5 py-1 text-muted-foreground transition-colors hover:text-foreground"
                />
              </div>
            </div>
            {/* 强度条 */}
            <PasswordStrengthBar password={password} showToast={showToast} />
          </div>
        </div>

        {/* 行：TOTP | 文件夹（两列等宽） */}
        <div className="grid grid-cols-2 gap-3">
          {/* TOTP */}
          <div className="min-w-0 space-y-1">
            <FieldLabel>{t("settings.vault.editor.totpLabel")}</FieldLabel>
            <Input
              size="full"
              value={totp}
              onChange={(e) => setTotp(e.target.value)}
              placeholder="JBSWY3DPEHPK3PXP"
              className="w-full font-mono-vault"
              autoComplete="off"
            />
            {totpResult && (
              <div className="flex items-center gap-2 rounded-md bg-muted/30 px-2 py-1.5">
                <span className="font-mono-vault text-lg tracking-[0.2em] tabular-nums">
                  {totpResult.code.slice(0, 3)} {totpResult.code.slice(3)}
                </span>
                <span className="ml-auto text-[10px] text-muted-foreground tabular-nums">
                  ⏱ {totpResult.seconds_remaining}s
                </span>
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
                  className="flex items-center text-muted-foreground transition-colors hover:text-foreground"
                  title={t("settings.vault.totp.copyCode")}
                >
                  <IconImg name="copy" className="size-3.5" alt={t("settings.vault.totp.copyCode")} />
                </button>
              </div>
            )}
          </div>

          {/* 文件夹 */}
          <div className="min-w-0 space-y-1">
            <FieldLabel>{t("settings.vault.editor.folderLabel")}</FieldLabel>
            <Select
              size="full"
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
        </div>

        {/* 网址（整行） */}
        <div className="space-y-1">
          <FieldLabel>{t("settings.vault.editor.urlLabel")}</FieldLabel>
          <Textarea
            size="full"
            value={urls}
            onChange={(e) => setUrls(e.target.value)}
            className="w-full min-h-[60px] resize-y font-mono-vault text-xs"
            placeholder={"https://example.com/login"}
          />
        </div>
        {/* 备注（整行） */}
        <div className="space-y-1">
          <FieldLabel>{t("settings.vault.editor.notesLabel")}</FieldLabel>
          <Textarea
            size="full"
            value={notes}
            onChange={(e) => setNotes(e.target.value)}
            className="w-full min-h-[60px] resize-y"
          />
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
