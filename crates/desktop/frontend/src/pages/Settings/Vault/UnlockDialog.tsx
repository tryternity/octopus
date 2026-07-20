import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Lock } from "lucide-react";
import { useT } from "@/lib/i18n";
import { Button } from "@/components/ui/button";
import { PasswordInput } from "@/components/ui/password-input";

/**
 * UnlockDialog —— 已初始化但锁定时，输入主密码解锁。
 *
 * 后端 `vault_unlock(password)` 失败统一显示 wrongPassword。
 *
 * **布局**（2026-07-20 e2e 反馈对齐 VaultPicker locked 视图）：
 *   - 顶部 border-b 标题栏（absolute 居中"解锁保险库"）
 *   - 中间表单区垂直居中：PasswordInput（size=full，带 Eye/Eraser）+ Button（w-full）
 *   - PasswordInput 与 Button 等宽——视觉协调
 *   - 删除了原版的"端到端加密存储..."副标题（信息冗余，用户已知）
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
    <form onSubmit={handleSubmit} className="flex h-full flex-col">
      {/* 标题栏：absolute 居中标题 + 左侧 size-7 占位（保持对称） */}
      <div className="relative flex items-center border-b border-border/40 px-4 py-3">
        <div className="size-7" aria-hidden />
        <span className="absolute left-1/2 flex -translate-x-1/2 items-center gap-2">
          <Lock className="size-4" />
          <span className="text-sm font-medium">
            {t("settings.vault.unlock.title")}
          </span>
        </span>
      </div>
      {/* 表单内容垂直居中，宽度限 320px 视觉更紧凑 */}
      <div className="mx-auto flex w-full max-w-[320px] flex-1 flex-col justify-center gap-3 px-6 py-6">
        <PasswordInput
          value={password}
          onChange={(e) => setPassword(e.target.value)}
          onClear={() => setError(null)}
          placeholder={t("settings.vault.unlock.passwordLabel")}
          autoFocus
          autoComplete="current-password"
          size="full"
        />
        {error && <p className="text-xs text-destructive">{error}</p>}
        <Button
          type="submit"
          variant="voice"
          disabled={busy || !password}
          className="w-full"
        >
          {busy ? "..." : t("settings.vault.unlock.submit")}
        </Button>
      </div>
    </form>
  );
}
