import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { LogicalSize } from "@tauri-apps/api/window";
import { listen } from "@tauri-apps/api/event";
import { Copy, Keyboard, KeyRound, AtSign, Eye, EyeOff, Lock, RefreshCw, X } from "lucide-react";
import { useT } from "@/lib/i18n";
import { Button } from "@/components/ui/button";
import { PasswordInput } from "@/components/ui/password-input";
import { classifyError } from "./classifyError";

/**
 * VaultPicker —— vault Auto-Type cipher 选择浮窗（label=vault_picker_window）。
 *
 * 流程：
 *   1. 全局热键 CmdOrCtrl+Shift+L → 后端 `register_vault_autotype_shortcut`
 *      show + set_focus 窗口并 emit `vault://picker-refresh`（首次则 build 窗口）。
 *   2. 窗口 mount / 收到 refresh → 调 `vault_detect_and_match` 取匹配 cipher。
 *   3. 用户点击 → `vault_autotype`（默认）或 `vault_copy_password`（按住修饰键）。
 *   4. 选中后 hide 窗口（保留实例，下次热键更快）；Escape 也 hide。
 *   5. vault 未解锁 → 显示内联解锁表单（成功后自动 refresh）。
 *   6. vault 未初始化 → 提示去 Settings 初始化（picker 无法独立 setup）。
 *
 * 关键约定：
 *   - hide 而非 close：复用实例，避免每次按热键都重建 React 树。
 *   - 错误分类用纯函数 classifyError（单测见 classifyError.test.ts），
 *     后端契约：require_user_vault_key → "vault 未解锁"；
 *     octopus_vault::unlock 内部错误含 "vault 未初始化"。
 */

interface LoginDataDto {
  uris: { uri: string; match_type: number | null }[];
  username: string | null;
  password: string | null;
  totp: string | null;
}

interface CipherDto {
  id: number;
  name: string;
  favorite: boolean;
  login: LoginDataDto | null;
  /** 0=None / 1=Password（reprompt 保护的高敏感 cipher，自动填充前需再次输入主密码） */
  reprompt?: number;
}

/** 三模式 autotype（2026-07-20）。
 *  - usernamePassword: 完整填（username + Tab + password），焦点须在 username 框
 *  - passwordOnly（默认）: 仅填密码，焦点已在 password 框
 *  - usernameOnly: 仅填用户名，焦点在 username 框
 *
 *  背后原因：webmail SPA（mail.163.com 等）的 Tab 切焦点不可靠。给用户三种独立控制，
 *  据当前光标位置选合适模式。Bitwarden/1Password 桌面助手默认也是 PasswordOnly。
 *
 *  字面值 camelCase：后端 AutoTypeMode enum 标了 serde(rename_all = "camelCase")，
 *  Tauri 命令边界序列化时 Rust 端期望 camelCase 字符串。*/
type AutotypeMode = "usernamePassword" | "passwordOnly" | "usernameOnly";

type ViewState =
  | { kind: "loading" }
  | { kind: "list"; ciphers: CipherDto[] }
  | { kind: "locked" }
  | { kind: "uninit" }
  | { kind: "error"; message: string }
  | { kind: "reprompt"; cipher: CipherDto; copyOnly: boolean; mode: AutotypeMode }
  | { kind: "autotyping" };

export default function VaultPicker() {
  const t = useT();
  const [view, setView] = useState<ViewState>({ kind: "loading" });
  const [unlockPassword, setUnlockPassword] = useState("");
  const [unlockError, setUnlockError] = useState<string | null>(null);
  // 密码可见性——按 cipher id 独立 toggle（多个 cipher 互不影响）。
  // 默认全部 mask（•••••）防浮窗一闪而过被旁人偷看。
  const [revealedPasswords, setRevealedPasswords] = useState<Record<number, boolean>>({});
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(async () => {
    setView({ kind: "loading" });
    setUnlockError(null);
    try {
      const list = await invoke<CipherDto[]>("vault_detect_and_match");
      setView({ kind: "list", ciphers: list });
    } catch (e) {
      setView(classifyError(e));
    }
  }, []);

  // 首次 mount → 立刻拉取匹配 cipher。
  useEffect(() => {
    refresh();
  }, [refresh]);

  // 后端 show 窗口时 emit `vault://picker-refresh` → 重新拉取（保证每次都拿最新数据）。
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    (async () => {
      const u = await listen("vault://picker-refresh", () => {
        refresh();
      });
      unlisten = u;
    })();
    return () => {
      unlisten?.();
    };
  }, [refresh]);

  // **2026-07-20 e2e 反馈**：按 view 动态调整浮窗高度，避免小内容也占满固定高度
  // 导致上下大片空白。后端初始 200px（紧凑视图），list 视图按 cipher 数撑高。
  // 高度估算：标题栏 36 + padding + 各视图内容高 + 上下留白
  useEffect(() => {
    let height: number;
    switch (view.kind) {
      case "loading":
        height = 110;
        break;
      case "locked":
      case "reprompt":
        // 标题栏 36 + Input 36 + Button 36 + gap+padding ~60 + 副文（reprompt） ~24
        height = view.kind === "reprompt" ? 220 : 200;
        break;
      case "uninit":
      case "error":
        height = 130;
        break;
      case "autotyping":
        height = 110;
        break;
      case "list": {
        // 标题栏 36 + 每个 cipher 三段约 88px + 空状态 60
        const ciphers = "ciphers" in view ? view.ciphers.length : 0;
        const contentH = ciphers === 0 ? 60 : ciphers * 88;
        height = Math.min(560, Math.max(140, 36 + contentH + 16));
        break;
      }
      default:
        height = 200;
    }
    void getCurrentWindow().setSize(new LogicalSize(400, height));
  }, [view]);

  // Escape → 隐藏窗口；保留实例以便下次热键直接 show。
  useEffect(() => {
    const keyHandler = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        void getCurrentWindow().hide();
      }
    };
    window.addEventListener("keydown", keyHandler);
    return () => {
      window.removeEventListener("keydown", keyHandler);
    };
  }, []);

  const handleUnlock = useCallback(
    async (e: React.FormEvent) => {
      e.preventDefault();
      if (!unlockPassword) return;
      setBusy(true);
      setUnlockError(null);
      try {
        await invoke("vault_unlock", { password: unlockPassword });
        setUnlockPassword("");
        await refresh();
      } catch (err) {
        setUnlockError(t("settings.vault.unlock.wrongPassword"));
        void err;
      } finally {
        setBusy(false);
      }
    },
    [unlockPassword, refresh, t],
  );

  const handlePick = useCallback(
    async (c: CipherDto, copyOnly: boolean, mode: AutotypeMode = "passwordOnly") => {
      // reprompt 保护的高敏感 cipher：弹密码框，确认后再调后端
      // （后端 vault_autotype / vault_copy_password 都会强制再次校验 master_password，不可绕过）
      if (c.reprompt === 1) {
        setUnlockPassword("");
        setUnlockError(null);
        setView({ kind: "reprompt", cipher: c, copyOnly, mode });
        return;
      }
      await runAutotype(c, copyOnly, mode, undefined);
    },
    [],
  );

  /** 实际调 vault_autotype / vault_copy_password——reprompt 通过后也走这里。
   *  masterPassword 仅在 reprompt 场景传入。
   *
   *  **2026-07-20 e2e 修复**：不要 await getCurrentWindow().hide()——hide 会让
   *  webview 进入 terminated 状态，紧接着的 invoke 永远到不了后端（race condition）。
   *  hide 改由后端 vault_autotype 自己做（用 AppHandle 拿窗口引用）。*/
  const runAutotype = useCallback(
    async (
      c: CipherDto,
      copyOnly: boolean,
      mode: AutotypeMode,
      masterPassword: string | undefined,
    ) => {
      setBusy(true);
      try {
        if (copyOnly) {
          await invoke("vault_copy_password", {
            cipherId: c.id,
            masterPassword: masterPassword ?? null,
          });
          // copy 后 hide 浮窗（copy 不像 autotype 需要后端做焦点管理，前端 hide 安全）
          await getCurrentWindow().hide();
        } else {
          // autotype 由后端 hide——前端不调 hide，避免 invoke 失联
          await invoke("vault_autotype", {
            cipherId: c.id,
            masterPassword: masterPassword ?? null,
            mode,
          });
        }
      } catch (e) {
        setView(classifyError(e));
      } finally {
        setBusy(false);
      }
    },
    [],
  );

  // === locked: 内联解锁表单 ===
  // 布局：标题栏置顶，表单内容垂直居中（浮窗固定 360px 高，内容少，居中避免空白堆积
  // 在底部）。Input 和 Button 都 w-full 等宽，视觉协调。
  if (view.kind === "locked") {
    return (
      <form
        onSubmit={handleUnlock}
        className="flex h-screen flex-col bg-background text-foreground"
      >
        {/* 标题栏：absolute 居中标题 + 右侧 X 按钮 + 左侧同等宽占位保持对称 */}
        <div className="relative flex items-center border-b border-border/40 px-4 py-2">
          {/* 左侧占位——与右侧 X 按钮等宽，让 absolute 标题真正居中 */}
          <div className="size-7" aria-hidden />
          <span className="absolute left-1/2 flex -translate-x-1/2 items-center gap-2">
            <Lock className="size-4" />
            <span className="text-sm font-medium">
              {t("settings.vault.unlock.title")}
            </span>
          </span>
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            onClick={() => getCurrentWindow().hide()}
            className="ml-auto"
          >
            <X />
          </Button>
        </div>
        {/* 表单内容垂直居中 */}
        <div className="flex flex-1 flex-col justify-center gap-3 px-6 py-4">
          <PasswordInput
            value={unlockPassword}
            onChange={(e) => setUnlockPassword(e.target.value)}
            onClear={() => setUnlockError(null)}
            placeholder={t("settings.vault.unlock.passwordLabel")}
            autoFocus
            autoComplete="current-password"
            size="full"
          />
          {unlockError && (
            <p className="text-xs text-destructive">{unlockError}</p>
          )}
          <Button
            type="submit"
            variant="voice"
            disabled={busy || !unlockPassword}
            className="w-full"
          >
            {busy ? "..." : t("settings.vault.unlock.submit")}
          </Button>
        </div>
      </form>
    );
  }

  // === uninit: 提示去 Settings 初始化 ===
  if (view.kind === "uninit") {
    return (
      <div className="flex h-screen flex-col bg-background text-foreground">
        <div className="relative flex items-center border-b border-border/40 px-4 py-2">
          <div className="size-7" aria-hidden />
          <span className="absolute left-1/2 flex -translate-x-1/2 items-center gap-2">
            <Lock className="size-4" />
            <span className="text-sm font-medium">
              {t("settings.vault.autotype.uninitTitle")}
            </span>
          </span>
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            onClick={() => getCurrentWindow().hide()}
            className="ml-auto"
          >
            <X />
          </Button>
        </div>
        <div className="flex flex-1 items-center px-6 py-4">
          <p className="text-xs text-muted-foreground">
            {t("settings.vault.autotype.uninitHint")}
          </p>
        </div>
      </div>
    );
  }

  // === reprompt: 高敏感 cipher 二次输入主密码 ===
  if (view.kind === "reprompt") {
    const submitReprompt: React.FormEventHandler = async (e) => {
      e.preventDefault();
      if (!unlockPassword) return;
      const cipher = view.cipher;
      const copyOnly = view.copyOnly;
      const mode = view.mode;
      const pwd = unlockPassword;
      setView({ kind: "autotyping" });
      await runAutotype(cipher, copyOnly, mode, pwd);
    };
    return (
      <form
        onSubmit={submitReprompt}
        className="flex h-screen flex-col bg-background text-foreground"
      >
        {/* 标题栏：absolute 居中标题 + 右侧 X 按钮 + 左侧同等宽占位 */}
        <div className="relative flex items-center border-b border-border/40 px-4 py-2">
          <div className="size-7" aria-hidden />
          <span className="absolute left-1/2 flex -translate-x-1/2 items-center gap-2">
            <Lock className="size-4" />
            <span className="text-sm font-medium">
              {t("settings.vault.autotype.repromptTitle")}
            </span>
          </span>
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            onClick={() => refresh()}
            className="ml-auto"
          >
            <X />
          </Button>
        </div>
        {/* 内容垂直居中 */}
        <div className="flex flex-1 flex-col justify-center gap-3 px-6 py-4">
          <p className="text-xs text-muted-foreground">
            {t("settings.vault.autotype.repromptHint", { name: view.cipher.name })}
          </p>
          <PasswordInput
            value={unlockPassword}
            onChange={(e) => setUnlockPassword(e.target.value)}
            onClear={() => setUnlockError(null)}
            placeholder={t("settings.vault.unlock.passwordLabel")}
            autoFocus
            autoComplete="current-password"
            size="full"
          />
          {unlockError && (
            <p className="text-xs text-destructive">{unlockError}</p>
          )}
          <Button
            type="submit"
            variant="voice"
            disabled={busy || !unlockPassword}
            className="w-full"
          >
            {busy ? "..." : t("settings.vault.autotype.trigger")}
          </Button>
        </div>
      </form>
    );
  }

  // === autotyping: 等后端注入完成 ===
  if (view.kind === "autotyping") {
    return (
      <div className="flex h-screen items-center justify-center bg-background text-xs text-muted-foreground">
        {t("settings.loading")}
      </div>
    );
  }

  // === error / list / loading: 共用外壳 ===
  return (
    <div className="flex h-screen flex-col bg-background text-foreground">
      {/* 顶部标题栏：absolute 居中标题 + 右侧 Refresh + X 按钮 + 左侧同等宽占位 */}
      <div className="relative flex items-center border-b border-border px-4 py-2">
        {/* 左侧占位——与右侧 2 个 icon-sm 按钮（gap-1）等宽，让 absolute 标题真正居中。
            2 个 icon-sm 按钮（约 24px each）+ gap-1（4px）≈ 52px */}
        <div className="w-[52px]" aria-hidden />
        <span className="absolute left-1/2 -translate-x-1/2 text-sm font-medium">
          {t("settings.vault.autotype.trigger")}
        </span>
        <div className="ml-auto flex items-center gap-1">
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            onClick={refresh}
            title={t("settings.vault.generator.regenerate")}
            disabled={busy}
          >
            <RefreshCw />
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            onClick={() => getCurrentWindow().hide()}
          >
            <X />
          </Button>
        </div>
      </div>

      {/* 内容区 */}
      <div className="flex-1 overflow-auto">
        {view.kind === "loading" && (
          <div className="px-4 py-3 text-sm text-muted-foreground">...</div>
        )}

        {view.kind === "list" && view.ciphers.length === 0 && (
          <div className="px-4 py-3 text-sm text-muted-foreground">
            {t("settings.vault.autotype.noMatch")}
          </div>
        )}

        {view.kind === "list" &&
          view.ciphers.map((c) => {
            const username = c.login?.username || "";
            const password = c.login?.password || "";
            const revealed = !!revealedPasswords[c.id];
            return (
              <div
                key={c.id}
                className="border-b border-border/50 last:border-b-0 hover:bg-accent"
              >
                {/* 第一段：名称行 + 复制(无) + 完整填充(⌨)
                    名称行无对应复制按钮（整个条目的"名"不属于字段），右侧只放一个
                    完整填充图标，与下面两段的 autotype 列对齐。 */}
                <div className="flex items-center gap-1 px-4 pt-2">
                  <div className="min-w-0 flex-1 truncate text-sm font-semibold">
                    {c.name}
                  </div>
                  {/* 占位与下方 copy 列对齐 */}
                  <div className="size-7 shrink-0" aria-hidden />
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon-sm"
                    onClick={(e) => {
                      e.stopPropagation();
                      handlePick(c, false, "usernamePassword");
                    }}
                    disabled={busy}
                    title={t("settings.vault.autotype.mode.usernamePassword")}
                    className="shrink-0 text-muted-foreground hover:text-foreground"
                  >
                    <Keyboard className="size-4" />
                  </Button>
                </div>

                {/* 第二段：用户名行 + 复制(📋) + 仅填用户名(@) */}
                <div className="flex items-center gap-1 px-4 py-1">
                  <div className="min-w-0 flex-1 truncate text-xs text-muted-foreground">
                    {username || "—"}
                  </div>
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon-sm"
                    onClick={(e) => {
                      e.stopPropagation();
                      void invoke("vault_copy_username", { cipherId: c.id });
                    }}
                    disabled={busy || !username}
                    title={t("settings.vault.autotype.copyUsername")}
                    className="shrink-0 text-muted-foreground hover:text-foreground"
                  >
                    <Copy className="size-4" />
                  </Button>
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon-sm"
                    onClick={(e) => {
                      e.stopPropagation();
                      handlePick(c, false, "usernameOnly");
                    }}
                    disabled={busy || !username}
                    title={t("settings.vault.autotype.mode.usernameOnly")}
                    className="shrink-0 text-muted-foreground hover:text-foreground"
                  >
                    <AtSign className="size-4" />
                  </Button>
                </div>

                {/* 第三段：密码行 + 复制(📋) + 仅填密码(🔑)
                    默认 mask 显示（••••），点眼睛切换可见。 */}
                <div className="flex items-center gap-1 px-4 pb-2">
                  <button
                    type="button"
                    onClick={() =>
                      setRevealedPasswords((m) => ({ ...m, [c.id]: !m[c.id] }))
                    }
                    disabled={!password}
                    title={
                      revealed
                        ? t("settings.vault.autotype.hidePassword")
                        : t("settings.vault.autotype.revealPassword")
                    }
                    className="flex min-w-0 flex-1 items-center gap-1 text-left outline-none"
                  >
                    <span className="truncate text-xs text-muted-foreground">
                      {password
                        ? revealed
                          ? password
                          : "•".repeat(Math.max(6, Math.min(12, password.length)))
                        : "—"}
                    </span>
                    {password &&
                      (revealed ? (
                        <EyeOff className="size-3 shrink-0 text-muted-foreground/60" />
                      ) : (
                        <Eye className="size-3 shrink-0 text-muted-foreground/60" />
                      ))}
                  </button>
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon-sm"
                    onClick={(e) => {
                      e.stopPropagation();
                      handlePick(c, true, "passwordOnly");
                    }}
                    disabled={busy || !password}
                    title={t("settings.vault.generator.copy")}
                    className="shrink-0 text-muted-foreground hover:text-foreground"
                  >
                    <Copy className="size-4" />
                  </Button>
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon-sm"
                    onClick={(e) => {
                      e.stopPropagation();
                      handlePick(c, false, "passwordOnly");
                    }}
                    disabled={busy || !password}
                    title={t("settings.vault.autotype.mode.passwordOnly")}
                    className="shrink-0 text-muted-foreground hover:text-foreground"
                  >
                    <KeyRound className="size-4" />
                  </Button>
                </div>
              </div>
            );
          })}

        {view.kind === "error" && (
          <div className="px-4 py-3 text-sm text-destructive">{view.message}</div>
        )}
      </div>
    </div>
  );
}
