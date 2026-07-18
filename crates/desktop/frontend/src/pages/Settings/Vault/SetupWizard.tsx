import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useT } from "@/lib/i18n";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";

const inputCls = "w-full";

/**
 * SetupWizard —— 首次初始化保险库：设置主密码。
 *
 * 弱密码门槛：长度 < 12。两次输入必须一致。
 * 后端 `vault_setup(password)` 完成 Argon2 派生 + 写 manifest + 解锁 session。
 */
export default function SetupWizard({
  onCompleted,
  showToast,
}: {
  onCompleted: () => Promise<void>;
  showToast: (msg: string) => void;
}) {
  const t = useT();
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    if (password.length < 12) {
      setError(t("settings.vault.setup.weakPassword"));
      return;
    }
    if (password !== confirm) {
      setError(t("settings.vault.setup.passwordMismatch"));
      return;
    }
    setBusy(true);
    try {
      await invoke("vault_setup", { password });
      await onCompleted();
    } catch (e) {
      const msg = String(e);
      setError(msg);
      showToast(msg);
    } finally {
      setBusy(false);
    }
  }

  return (
    <form onSubmit={handleSubmit} className="mx-auto max-w-md space-y-4">
      <h2 className="text-xl font-semibold">{t("settings.vault.setup.title")}</h2>
      <p className="rounded-md border border-amber-500/30 bg-amber-500/5 px-3 py-2 text-sm text-amber-700 dark:text-amber-400">
        {t("settings.vault.setup.warning")}
      </p>
      <div className="space-y-1.5">
        <label className="block text-[11px] font-medium uppercase tracking-wide text-muted-foreground/80">
          {t("settings.vault.setup.passwordLabel")}
        </label>
        <Input
          type="password"
          value={password}
          onChange={(e) => setPassword(e.target.value)}
          className={inputCls}
          autoFocus
          autoComplete="new-password"
        />
      </div>
      <div className="space-y-1.5">
        <label className="block text-[11px] font-medium uppercase tracking-wide text-muted-foreground/80">
          {t("settings.vault.setup.passwordConfirm")}
        </label>
        <Input
          type="password"
          value={confirm}
          onChange={(e) => setConfirm(e.target.value)}
          className={inputCls}
          autoComplete="new-password"
        />
      </div>
      {error && <p className="text-sm text-destructive">{error}</p>}
      <Button type="submit" variant="voice" disabled={busy}>
        {busy ? "..." : t("settings.vault.setup.submit")}
      </Button>
    </form>
  );
}
