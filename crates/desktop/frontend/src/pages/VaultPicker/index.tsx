import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { listen } from "@tauri-apps/api/event";
import { Copy, Keyboard, KeyRound, AtSign, Lock, RefreshCw, X } from "lucide-react";
import { useT } from "@/lib/i18n";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
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
 *  - UsernamePassword: 完整填（username + Tab + password），焦点须在 username 框
 *  - PasswordOnly（默认）: 仅填密码，焦点已在 password 框
 *  - UsernameOnly: 仅填用户名，焦点在 username 框
 *
 *  背后原因：webmail SPA（mail.163.com 等）的 Tab 切焦点不可靠。给用户三种独立控制，
 *  据当前光标位置选合适模式。Bitwarden/1Password 桌面助手默认也是 PasswordOnly。*/
type AutotypeMode = "UsernamePassword" | "PasswordOnly" | "UsernameOnly";

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
    async (c: CipherDto, copyOnly: boolean, mode: AutotypeMode = "PasswordOnly") => {
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
   *  masterPassword 仅在 reprompt 场景传入。*/
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
        } else {
          // 关键：autotype 前先 hide 浮窗，让浏览器回到前台。
          // 后端 vault_autotype 会 sleep + 校验前台不是 octopus 自身（防钓鱼注入），
          // 若浮窗未 hide 则校验失败 → fallback 到剪贴板。
          await getCurrentWindow().hide();
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
  if (view.kind === "locked") {
    return (
      <form onSubmit={handleUnlock} className="flex h-screen flex-col gap-3 bg-background p-4 text-foreground">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-2">
            <Lock className="size-4" />
            <span className="text-sm font-medium">
              {t("settings.vault.unlock.title")}
            </span>
          </div>
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            onClick={() => getCurrentWindow().hide()}
          >
            <X />
          </Button>
        </div>
        <Input
          type="password"
          value={unlockPassword}
          onChange={(e) => setUnlockPassword(e.target.value)}
          placeholder={t("settings.vault.unlock.passwordLabel")}
          autoFocus
          autoComplete="current-password"
        />
        {unlockError && <p className="text-xs text-destructive">{unlockError}</p>}
        <Button
          type="submit"
          variant="voice"
          disabled={busy || !unlockPassword}
        >
          {busy ? "..." : t("settings.vault.unlock.submit")}
        </Button>
      </form>
    );
  }

  // === uninit: 提示去 Settings 初始化 ===
  if (view.kind === "uninit") {
    return (
      <div className="flex h-screen flex-col gap-2 bg-background p-4 text-foreground">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-2">
            <Lock className="size-4" />
            <span className="text-sm font-medium">
              {t("settings.vault.autotype.uninitTitle")}
            </span>
          </div>
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            onClick={() => getCurrentWindow().hide()}
          >
            <X />
          </Button>
        </div>
        <p className="text-xs text-muted-foreground">
          {t("settings.vault.autotype.uninitHint")}
        </p>
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
      <form onSubmit={submitReprompt} className="flex h-screen flex-col gap-3 bg-background p-4 text-foreground">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-2">
            <Lock className="size-4" />
            <span className="text-sm font-medium">
              {t("settings.vault.autotype.repromptTitle")}
            </span>
          </div>
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            onClick={() => refresh()}
          >
            <X />
          </Button>
        </div>
        <p className="text-xs text-muted-foreground">
          {t("settings.vault.autotype.repromptHint", { name: view.cipher.name })}
        </p>
        <Input
          type="password"
          value={unlockPassword}
          onChange={(e) => setUnlockPassword(e.target.value)}
          placeholder={t("settings.vault.unlock.passwordLabel")}
          autoFocus
          autoComplete="current-password"
        />
        {unlockError && <p className="text-xs text-destructive">{unlockError}</p>}
        <Button
          type="submit"
          variant="voice"
          disabled={busy || !unlockPassword}
        >
          {busy ? "..." : t("settings.vault.autotype.trigger")}
        </Button>
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
      {/* 顶部标题栏 */}
      <div className="flex items-center justify-between border-b border-border px-4 py-2">
        <span className="text-sm font-medium">
          {t("settings.vault.autotype.trigger")}
        </span>
        <div className="flex items-center gap-1">
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
          view.ciphers.map((c) => (
            <div
              key={c.id}
              className="group flex items-center gap-1 border-b border-border/50 px-4 py-2 last:border-b-0 hover:bg-accent"
            >
              {/* 行主体：点击触发 PasswordOnly（最常用，webmail SPA 首选） */}
              <button
                type="button"
                className="flex flex-1 items-center gap-2 text-left outline-none min-w-0"
                onClick={() => handlePick(c, false, "PasswordOnly")}
                disabled={busy}
                title={t("settings.vault.autotype.mode.passwordOnly")}
              >
                <div className="min-w-0 flex-1">
                  <div className="truncate text-sm font-medium">{c.name}</div>
                  <div className="truncate text-xs text-muted-foreground">
                    {c.login?.username || "—"}
                  </div>
                </div>
              </button>
              {/* 3 个 autotype 模式 + 1 个 copy——按当前光标位置选合适图标。
                  视觉策略：PasswordOnly（KeyRound）用 voice 色高亮（签名元素，
                  webmail SPA 默认场景），其他保持 muted-foreground 让用户知道
                  是次要选项。hover 时统一切到 foreground 色。 */}
              <Button
                type="button"
                variant="ghost"
                size="icon-sm"
                onClick={(e) => {
                  e.stopPropagation();
                  handlePick(c, false, "UsernamePassword");
                }}
                disabled={busy}
                title={t("settings.vault.autotype.mode.usernamePassword")}
                className="text-muted-foreground hover:text-foreground"
              >
                <Keyboard className="size-4" />
              </Button>
              <Button
                type="button"
                variant="ghost"
                size="icon-sm"
                onClick={(e) => {
                  e.stopPropagation();
                  handlePick(c, false, "PasswordOnly");
                }}
                disabled={busy}
                title={t("settings.vault.autotype.mode.passwordOnly")}
                className="text-voice hover:text-voice"
              >
                <KeyRound className="size-4" />
              </Button>
              <Button
                type="button"
                variant="ghost"
                size="icon-sm"
                onClick={(e) => {
                  e.stopPropagation();
                  handlePick(c, false, "UsernameOnly");
                }}
                disabled={busy}
                title={t("settings.vault.autotype.mode.usernameOnly")}
                className="text-muted-foreground hover:text-foreground"
              >
                <AtSign className="size-4" />
              </Button>
              <Button
                type="button"
                variant="ghost"
                size="icon-sm"
                onClick={(e) => {
                  e.stopPropagation();
                  handlePick(c, true, "PasswordOnly");
                }}
                disabled={busy}
                title={t("settings.vault.generator.copy")}
                className="text-muted-foreground hover:text-foreground"
              >
                <Copy className="size-4" />
              </Button>
            </div>
          ))}

        {view.kind === "error" && (
          <div className="px-4 py-3 text-sm text-destructive">{view.message}</div>
        )}
      </div>
    </div>
  );
}
