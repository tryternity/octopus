import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useT } from "@/lib/i18n";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";

/**
 * UnlockDialog —— 已初始化但锁定时，输入主密码解锁。
 *
 * 后端 `vault_unlock(password)` 失败统一显示 wrongPassword。
 *
 * 视觉（UI 重设计）：小号 uppercase 标题 + 全宽 primary 按钮。
 */
export default function UnlockDialog({
  onSuccess,
  showToast,
}: {
  onSuccess: () => Promise<void>;
  showToast: (msg: string) => void;
}) {
  const t = useT();
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    setBusy(true);
    try {
      await invoke("vault_unlock", { password });
      setPassword("");
      await onSuccess();
    } catch (e) {
      const msg = t("settings.vault.unlock.wrongPassword");
      setError(msg);
      showToast(msg);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="flex h-full items-center justify-center">
      <form onSubmit={handleSubmit} className="w-full max-w-md space-y-4">
        <div>
          <h2 className="text-xs font-semibold uppercase tracking-[0.15em] text-foreground">
            {t("settings.vault.unlockVaultTitle")}
          </h2>
          <p className="mt-1 text-[11px] text-muted-foreground">
            {t("settings.vault.description")}
          </p>
        </div>
        <div className="space-y-1.5">
          <label className="block text-[11px] font-medium uppercase tracking-wide text-muted-foreground/80">
            {t("settings.vault.unlock.passwordLabel")}
          </label>
          <Input
            type="password"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            className="w-full"
            autoFocus
            autoComplete="current-password"
          />
        </div>
        {error && <p className="text-sm text-destructive">{error}</p>}
        <Button
          type="submit"
          variant="primary"
          className="w-full uppercase tracking-wide"
          disabled={busy || !password}
        >
          {busy ? "..." : t("settings.vault.unlock.submit")}
        </Button>
      </form>
    </div>
  );
}
