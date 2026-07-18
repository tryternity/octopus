import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useT } from "@/lib/i18n";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
    findMasterPasswordIssues,
    type MasterPasswordIssue,
} from "./validateMasterPassword";

const inputCls = "w-full";

/**
 * SetupWizard —— 首次初始化保险库：设置主密码。
 *
 * 强度要求：长度 ≥ 12 位 + 必含 4 类（大写/小写/数字/符号）。两次输入必须一致。
 * 后端 `vault_setup(password)` 完成 Argon2 派生 + 写 manifest + 解锁 session。
 */
function issueToMessage(issue: MasterPasswordIssue, t: (k: string) => string): string {
    switch (issue) {
        case "too_short":
            return t("settings.vault.setup.errTooShort");
        case "missing_uppercase":
            return t("settings.vault.setup.errMissingUppercase");
        case "missing_lowercase":
            return t("settings.vault.setup.errMissingLowercase");
        case "missing_digit":
            return t("settings.vault.setup.errMissingDigit");
        case "missing_symbol":
            return t("settings.vault.setup.errMissingSymbol");
    }
}
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
    const issues = findMasterPasswordIssues(password);
    if (issues.length > 0) {
      // 显示第一个问题（最关键的，按 enum 顺序：too_short 优先）
      setError(issueToMessage(issues[0], t));
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
      <p className="text-xs text-muted-foreground">
        {t("settings.vault.setup.strengthHint")}
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
        {password && (
          <ul className="mt-1 space-y-0.5 text-[11px]">
            <RuleCheck ok={password.length >= 12} label={t("settings.vault.setup.errTooShort").replace(/^[^：:]*[：:]\s*/, "")} />
            <RuleCheck ok={/[A-Z]/.test(password)} label={t("settings.vault.setup.errMissingUppercase").replace(/^[^：:]*[：:]\s*/, "")} />
            <RuleCheck ok={/[a-z]/.test(password)} label={t("settings.vault.setup.errMissingLowercase").replace(/^[^：:]*[：:]\s*/, "")} />
            <RuleCheck ok={/\d/.test(password)} label={t("settings.vault.setup.errMissingDigit").replace(/^[^：:]*[：:]\s*/, "")} />
            <RuleCheck ok={hasSymbol(password)} label={t("settings.vault.setup.errMissingSymbol").replace(/^[^：:]*[：:]\s*/, "")} />
          </ul>
        )}
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

function hasSymbol(s: string): boolean {
  return findMasterPasswordIssues(s).indexOf("missing_symbol") === -1;
}

function RuleCheck({ ok, label }: { ok: boolean; label: string }) {
  return (
    <li className={ok ? "text-green-600 dark:text-green-400" : "text-muted-foreground"}>
      <span className="mr-1">{ok ? "✓" : "○"}</span>
      {label}
    </li>
  );
}
