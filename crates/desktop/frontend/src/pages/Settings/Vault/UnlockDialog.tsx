import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useT } from "@/lib/i18n";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";

/**
 * UnlockDialog —— 已初始化但锁定时，输入主密码解锁。
 *
 * 后端 `vault_unlock(password)` 失败统一显示 wrongPassword。
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
    <form onSubmit={handleSubmit} className="mx-auto max-w-md space-y-4">
      <h2 className="text-xl font-semibold">{t("settings.vault.unlock.title")}</h2>
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
      <Button type="submit" variant="voice" disabled={busy || !password}>
        {busy ? "..." : t("settings.vault.unlock.submit")}
      </Button>
    </form>
  );
}
