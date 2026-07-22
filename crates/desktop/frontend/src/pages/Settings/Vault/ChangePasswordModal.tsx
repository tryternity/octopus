import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { X } from "lucide-react";
import { useT } from "@/lib/i18n";
import { PasswordInput } from "@/components/ui/password-input";
import { Button } from "@/components/ui/button";
import { validateMasterPassword } from "./validateMasterPassword";

/**
 * ChangePasswordModal —— 修改主密码弹窗。
 *
 * 三字段：旧密码 + 新密码 + 确认新密码。
 * 新密码实时强度条（debounce 300ms 调 vault_evaluate_password）。
 * 前端校验通过后才调 vault_change_password 后端命令。
 *
 * 后端 `change_master_password` 只重写 vault_meta 的 3 个包装密文
 * + 刷新 security_stamp，底层 user_vault_key 不变 → 现有 cipher 密文不受影响。
 *
 * 关闭交互：点遮罩 / 点 × / 按 Esc / 成功后自动关闭。
 */
export interface ChangePasswordModalProps {
  open: boolean;
  onClose: () => void;
  showToast: (msg: string, variant?: "success" | "error") => void;
}

interface PasswordStrength {
  score: number;
  entropy_bits: number;
}

export default function ChangePasswordModal({
  open,
  onClose,
  showToast,
}: ChangePasswordModalProps) {
  const t = useT();
  const [oldPwd, setOldPwd] = useState("");
  const [newPwd, setNewPwd] = useState("");
  const [confirmPwd, setConfirmPwd] = useState("");
  const [strength, setStrength] = useState<PasswordStrength | null>(null);
  const [submitting, setSubmitting] = useState(false);

  // 新密码强度条——debounce 300ms
  useEffect(() => {
    if (!newPwd) {
      setStrength(null);
      return;
    }
    const timer = setTimeout(() => {
      invoke<PasswordStrength>("vault_evaluate_password", { password: newPwd })
        .then(setStrength)
        .catch(() => setStrength(null));
    }, 300);
    return () => clearTimeout(timer);
  }, [newPwd]);

  // Esc 关闭
  useEffect(() => {
    if (!open) return;
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape") handleClose();
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [open]); // eslint-disable-line react-hooks/exhaustive-deps

  const reset = useCallback(() => {
    setOldPwd("");
    setNewPwd("");
    setConfirmPwd("");
    setStrength(null);
    setSubmitting(false);
  }, []);

  const handleClose = useCallback(() => {
    reset();
    onClose();
  }, [reset, onClose]);

  // 前端校验
  const validation = validateMasterPassword(newPwd);
  const canSubmit =
    oldPwd.length > 0 &&
    validation.ok &&
    newPwd === confirmPwd &&
    newPwd !== oldPwd &&
    !submitting;

  const handleSubmit = useCallback(async () => {
    if (!canSubmit) return;
    setSubmitting(true);
    try {
      await invoke("vault_change_password", {
        oldPassword: oldPwd,
        newPassword: newPwd,
      });
      showToast(t("settings.vault.changePasswordSuccess"), "success");
      handleClose();
    } catch (e) {
      showToast(t("settings.vault.changePasswordFailed") + String(e), "error");
      setSubmitting(false);
    }
  }, [canSubmit, oldPwd, newPwd, showToast, t, handleClose]);

  if (!open) return null;

  const pct = strength ? (strength.score + 1) * 20 : 0;
  const barColor =
    !strength ? "bg-muted"
    : strength.score <= 1 ? "bg-destructive"
    : strength.score === 2 ? "bg-warning"
    : "bg-success";

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 backdrop-blur-sm"
      onClick={handleClose}
    >
      <div
        className="w-[400px] rounded-xl border border-border bg-background p-6 shadow-2xl"
        onClick={(e) => e.stopPropagation()}
      >
        {/* 标题 + 关闭 */}
        <div className="mb-5 flex items-center justify-between">
          <h2 className="text-sm font-semibold">{t("settings.vault.changePassword")}</h2>
          <button
            type="button"
            tabIndex={-1}
            onClick={handleClose}
            className="rounded p-1 text-muted-foreground hover:bg-muted hover:text-foreground"
          >
            <X className="size-4" />
          </button>
        </div>

        {/* 表单 */}
        <div className="space-y-4">
          {/* 旧密码 */}
          <div>
            <label className="mb-1.5 block text-xs text-muted-foreground">
              {t("settings.vault.changePasswordOld")}
            </label>
            <PasswordInput
              variant="default"
              size="full"
              value={oldPwd}
              onChange={(e) => setOldPwd(e.target.value)}
              autoFocus
              onKeyDown={(e) => e.key === "Enter" && handleSubmit()}
              placeholder={t("settings.vault.changePasswordOldPlaceholder")}
            />
          </div>

          {/* 新密码 */}
          <div>
            <label className="mb-1.5 block text-xs text-muted-foreground">
              {t("settings.vault.changePasswordNew")}
            </label>
            <PasswordInput
              variant="default"
              size="full"
              value={newPwd}
              onChange={(e) => setNewPwd(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && handleSubmit()}
              placeholder={t("settings.vault.changePasswordNewPlaceholder")}
            />
            {/* 强度条 */}
            {strength && (
              <div className="flex items-center gap-2 pt-1.5">
                <div className="h-1 flex-1 overflow-hidden rounded-full bg-muted">
                  <div
                    className={`h-full transition-all ${barColor}`}
                    style={{ width: `${pct}%` }}
                  />
                </div>
                <span className="text-[10px] text-muted-foreground">
                  {t(`settings.vault.generator.strengthLevels.${strength.score}`)}
                </span>
              </div>
            )}
            {/* 校验提示 */}
            {newPwd && !validation.ok && (
              <p className="pt-1 text-[11px] text-muted-foreground">
                {t("settings.vault.setup.strengthHint")}
              </p>
            )}
            {/* 新旧密码相同提示 */}
            {newPwd && oldPwd && newPwd === oldPwd && (
              <p className="pt-1 text-[11px] text-destructive">
                {t("settings.vault.changePasswordSameAsOld")}
              </p>
            )}
          </div>

          {/* 确认新密码 */}
          <div>
            <label className="mb-1.5 block text-xs text-muted-foreground">
              {t("settings.vault.changePasswordConfirm")}
            </label>
            <PasswordInput
              variant="default"
              size="full"
              value={confirmPwd}
              onChange={(e) => setConfirmPwd(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && handleSubmit()}
              placeholder={t("settings.vault.changePasswordConfirmPlaceholder")}
            />
            {/* 不匹配提示 */}
            {confirmPwd && newPwd !== confirmPwd && (
              <p className="pt-1 text-[11px] text-destructive">
                {t("settings.vault.changePasswordMismatch")}
              </p>
            )}
          </div>
        </div>

        {/* 操作按钮 */}
        <div className="mt-6 flex justify-end gap-2">
          <Button variant="outline" size="sm" onClick={handleClose}>
            {t("settings.vault.changePasswordCancel")}
          </Button>
          <Button variant="voice" size="sm" onClick={handleSubmit} disabled={!canSubmit}>
            {submitting ? t("settings.vault.changePasswordSubmitting") : t("settings.vault.changePasswordConfirmBtn")}
          </Button>
        </div>
      </div>
    </div>
  );
}
