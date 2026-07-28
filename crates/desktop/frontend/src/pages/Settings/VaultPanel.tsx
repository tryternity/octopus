import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Lock, KeyRound } from "lucide-react";
import { useT } from "@/lib/i18n";
import { Button } from "@/components/ui/button";
import { PillTabs } from "@/components/ui/tabs";
import SetupWizard from "./Vault/SetupWizard";
import UnlockDialog from "./Vault/UnlockDialog";
import CipherList from "./Vault/CipherList";
import HealthReport from "./Vault/HealthReport";
import ImportExport from "./Vault/ImportExport";
import ChangePasswordModal from "./Vault/ChangePasswordModal";

/**
 * VaultPanel —— 密码保险库主面板（Settings 内一页）。
 *
 * 三态机：
 *   !initialized        → SetupWizard（首次创建主密码）
 *   !unlocked           → UnlockDialog（输入主密码解锁）
 *   unlocked            → 主面板（list / health / io 三视图，顶部 Tab 切换）
 *
 * 状态从后端 `vault_status` 拉。setup/unlock 成功后 refresh。
 *
 * 视觉：控制台布局——紧凑 header（VAULT 字标 + 加密 meta）+ Tab 栏（3 视图切换）
 * + body（sidebar + 主区）+ footer（端到端加密提示）。
 */
interface VaultStatus {
  initialized: boolean;
  userVaultUnlocked: boolean;
}

type View = "list" | "health" | "io";

export default function VaultPanel({ showToast }: { showToast: (msg: string, variant?: "success" | "error") => void }) {
  const t = useT();
  const [status, setStatus] = useState<VaultStatus | null>(null);
  const [view, setView] = useState<View>("list");
  // 自动锁定超时（秒）—— 0=永不，30-3600。归属 vault 自身配置，挂在 VaultPanel 顶部。
  const [lockTimeout, setLockTimeout] = useState<number>(180);
  const [changePwdOpen, setChangePwdOpen] = useState(false);

  const refreshStatus = useCallback(async () => {
    try {
      const s = await invoke<VaultStatus>("vault_status");
      setStatus(s);
    } catch (e) {
      showToast(t("settings.loadFailed") + String(e));
    }
  }, [showToast, t]);

  useEffect(() => {
    refreshStatus();
    // 心跳：保险库 tab 在前台 **且窗口聚焦** 时每 30s 调一次 vault_heartbeat，
    // 刷新 last_active_at 防止自动锁定。
    //
    // 关键：窗口失焦（切到其他 app / 最小化 / 切其他桌面）时停心跳，
    // 后端 last_active_at 不再刷新，超过 lockTimeout 后 is_user_vault_unlocked
    // 自动返回 false 并 zeroize key。窗口获焦时立即调 vault_status 检查状态，
    // 若已超时则 UI 自动切到锁屏（避免「页面仍显示已解锁但实际已锁」的脱节）。
    let heartbeatTimer: number | null = null;

    function startHeartbeat() {
      if (heartbeatTimer != null) return;
      heartbeatTimer = window.setInterval(() => {
        invoke("vault_heartbeat").catch(() => {
          // 静默失败（可能在锁定 / 测试中）
        });
      }, 30 * 1000);
    }

    function stopHeartbeat() {
      if (heartbeatTimer != null) {
        window.clearInterval(heartbeatTimer);
        heartbeatTimer = null;
      }
    }

    function handleFocus() {
      // 窗口获焦：立即检查 vault 状态（可能已超时锁定）
      // refreshStatus 会调 vault_status，后端在 is_user_vault_unlocked
      // 检查中主动 zeroize 超时的 key，并返回 userVaultUnlocked=false
      refreshStatus();
      // 重启心跳（如果仍解锁）
      startHeartbeat();
    }

    function handleBlur() {
      // 窗口失焦：停心跳，让后端 last_active_at 不再刷新
      // 这样超过 lockTimeout 后会自然超时（无需前端主动锁）
      stopHeartbeat();
    }

    // 初始：假设当前聚焦（mounted 时通常已聚焦）
    startHeartbeat();

    // Tauri webview 窗口的 focus/blur 事件
    window.addEventListener("focus", handleFocus);
    window.addEventListener("blur", handleBlur);
    // document visibilitychange（macOS 切桌面 / 最小化时也触发）
    document.addEventListener("visibilitychange", () => {
      if (document.hidden) {
        handleBlur();
      } else {
        handleFocus();
      }
    });

    // unmount 时主动锁定保险库——关闭设置窗口 / 切换到其他设置 tab
    // 都会触发 unmount，确保离开页面后必须重新输主密码（防偷窥）。
    return () => {
      stopHeartbeat();
      window.removeEventListener("focus", handleFocus);
      window.removeEventListener("blur", handleBlur);
      invoke("vault_lock").catch(() => {
        // 静默失败：可能在测试 / 关闭 app 期间，无需打扰用户
      });
    };
  }, [refreshStatus]);

  // 解锁后无需额外拉数据——Tab 内容由各自组件（CipherList/HealthReport/ImportExport）自管。

  // 加载当前自动锁定超时（与后端 AppConfig.vault_lock_timeout_secs 一致）
  useEffect(() => {
    invoke<number>("vault_get_lock_timeout")
      .then(setLockTimeout)
      .catch(() => {
        // 静默失败：保留默认 180，不阻塞 UI
      });
  }, []);

  const handleLockTimeoutChange = useCallback(
    async (secs: number) => {
      // 乐观更新——后端校验失败再回滚到旧值。
      const prev = lockTimeout;
      setLockTimeout(secs);
      try {
        await invoke("vault_set_lock_timeout", { secs });
      } catch (e) {
        setLockTimeout(prev);
        showToast(String(e));
      }
    },
    [lockTimeout, showToast],
  );

  const handleLock = useCallback(async () => {
    try {
      await invoke("vault_lock");
      await refreshStatus();
    } catch (e) {
      showToast(String(e));
    }
  }, [refreshStatus, showToast]);

  if (!status) {
    return (
      <div className="flex h-full items-center justify-center text-sm text-muted-foreground">
        {t("settings.loading")}
      </div>
    );
  }

  // 首次未初始化 → 引导创建主密码
  if (!status.initialized) {
    return <SetupWizard onCompleted={refreshStatus} showToast={showToast} />;
  }

  // 已初始化但未解锁 → 解锁弹窗
  if (!status.userVaultUnlocked) {
    return <UnlockDialog onSuccess={refreshStatus} showToast={showToast} />;
  }

  return (
    <div className="flex h-full flex-col">
      {/* Header row —— 紧凑控制台风：左 VAULT 字标，右 超时下拉 + Lock */}
      <div className="flex items-start justify-between border-b border-border pb-3">
        <div className="min-w-0">
          <h2 className="text-xs font-semibold uppercase tracking-[0.15em] text-foreground">
            {t("settings.vault.title")}
          </h2>
        </div>
        <div className="flex shrink-0 items-center gap-3">
          {/* 自动锁定超时——归属 vault 配置（非通用 Settings 表单） */}
          <div className="flex flex-col items-end gap-1">
            <div className="flex items-center gap-2">
              <label
                htmlFor="vault-lock-timeout"
                className="text-xs text-muted-foreground"
              >
                {t("settings.vault.lockTimeoutLabel")}
              </label>
              <select
                id="vault-lock-timeout"
                value={lockTimeout}
                onChange={(e) => handleLockTimeoutChange(Number(e.target.value))}
                className="rounded border border-border bg-background px-2 py-1 text-xs"
              >
                <option value={30}>30s</option>
                <option value={60}>1min</option>
                <option value={180}>
                  3min ({t("settings.vault.lockTimeoutRecommended")})
                </option>
                <option value={300}>5min</option>
                <option value={900}>15min</option>
                <option value={0}>{t("settings.vault.lockTimeoutNever")}</option>
              </select>
            </div>
            {lockTimeout === 0 && (
              <span className="max-w-[260px] text-right text-xs text-warning">
                {t("settings.vault.lockTimeoutWarning")}
              </span>
            )}
          </div>
          <Button variant="outline" size="sm" onClick={() => setChangePwdOpen(true)}>
            <KeyRound />
            {t("settings.vault.changePassword")}
          </Button>
          <Button variant="outline" size="sm" onClick={handleLock}>
            <Lock />
            {t("settings.vault.lock")}
          </Button>
        </div>
      </div>

      {/* Tab 栏——3 个视图切换（PillTabs，与 ModelsPanel 同款）。
          Git 同步已挪到系统设置 GeneralPanel 的 sync 子 Tab（不依赖 vault 解锁）。 */}
      <PillTabs
        items={[
          { key: "list", label: t("settings.vault.list.title") },
          { key: "health", label: t("settings.vault.health.title") },
          { key: "io", label: t("settings.vault.importExport.title") },
        ]}
        active={view}
        onChange={(k) => setView(k as View)}
      />

      {/* Body: sidebar + main —— sidebar 在 CipherList 内渲染（folder 选择 + 计数） */}
      <div className="min-h-0 flex-1 overflow-hidden">
        {view === "list" && <CipherList showToast={showToast} />}
        {view === "health" && (
          <div className="h-full overflow-auto">
            <HealthReport showToast={showToast} />
          </div>
        )}
        {view === "io" && (
          <div className="h-full overflow-auto">
            <ImportExport showToast={showToast} />
          </div>
        )}
      </div>

      {/* Footer —— 端到端加密提示（本地 AES-256-GCM + Argon2id） */}
      <div className="border-t border-border pt-2 text-[11px] text-muted-foreground">
        <span>{t("settings.vault.footerEncrypted")}</span>
      </div>

      {/* 修改主密码弹窗 */}
      <ChangePasswordModal
        open={changePwdOpen}
        onClose={() => setChangePwdOpen(false)}
        showToast={showToast}
      />
    </div>
  );
}
