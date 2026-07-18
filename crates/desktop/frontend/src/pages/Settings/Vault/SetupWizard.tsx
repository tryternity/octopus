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
 *
 * 视觉（UI 重设计）：小号 uppercase 标题 + warning 左侧细条 + 全宽 primary 按钮。
 * 密码输入框保持系统默认字体（隐藏圆点由 UA 渲染，不强制等宽）。
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
    <div className="flex h-full items-center justify-center overflow-y-auto py-8">
      <form onSubmit={handleSubmit} className="w-full max-w-md space-y-4">
      {/* 小号 uppercase 标题——控制台风 */}
      <div>
        <h2 className="text-xs font-semibold uppercase tracking-[0.15em] text-foreground">
          {t("settings.vault.createVaultTitle")}
        </h2>
        <p className="mt-1 text-[11px] text-muted-foreground">
          {t("settings.vault.setup.strengthHint")}
        </p>
      </div>

      {/* warning 左侧细条——subtle，不抢视觉 */}
      <p className="border-l-2 border-warning bg-warning/5 p-3 text-xs text-warning">
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
        {password && (
          <ul className="mt-1 space-y-0.5 text-[11px] text-muted-foreground">
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
      <Button
        type="submit"
        variant="primary"
        className="w-full uppercase tracking-wide"
        disabled={busy}
      >
        {busy ? "..." : t("settings.vault.setup.submit")}
      </Button>
      </form>
    </div>
  );
}

function hasSymbol(s: string): boolean {
  return findMasterPasswordIssues(s).indexOf("missing_symbol") === -1;
}

function RuleCheck({ ok, label }: { ok: boolean; label: string }) {
  return (
    <li className={ok ? "text-success" : "text-muted-foreground"}>
      <span className="mr-1">{ok ? "✓" : "○"}</span>
      {label}
    </li>
  );
}
