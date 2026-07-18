import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Lock, ShieldCheck } from "lucide-react";
import { useT } from "@/lib/i18n";
import { Button } from "@/components/ui/button";
import { UnderlineTabs } from "@/components/ui/tabs";
import SetupWizard from "./Vault/SetupWizard";
import UnlockDialog from "./Vault/UnlockDialog";
import CipherList from "./Vault/CipherList";
import HealthReport from "./Vault/HealthReport";
import ImportExport from "./Vault/ImportExport";

/**
 * VaultPanel —— 密码保险库主面板（Settings 内一页）。
 *
 * 三态机：
 *   !initialized        → SetupWizard（首次创建主密码）
 *   !unlocked           → UnlockDialog（输入主密码解锁）
 *   unlocked            → 主面板（ciphers / health / io 三 tab）
 *
 * 状态从后端 `vault_status` 拉。setup/unlock 成功后 refresh。
 */
interface VaultStatus {
  initialized: boolean;
  user_vault_unlocked: boolean;
}

type Tab = "ciphers" | "health" | "io";

export default function VaultPanel({ showToast }: { showToast: (msg: string) => void }) {
  const t = useT();
  const [status, setStatus] = useState<VaultStatus | null>(null);
  const [tab, setTab] = useState<Tab>("ciphers");
  // 自动锁定超时（秒）—— 0=永不，30-3600。归属 vault 自身配置，挂在 VaultPanel 顶部。
  const [lockTimeout, setLockTimeout] = useState<number>(180);

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
    // 心跳：保险库 tab 在前台时每 30s 调一次 vault_heartbeat，
    // 刷新 last_active_at 防止自动锁定。tab 切走 / 窗口关闭 →
    // useEffect cleanup 清 interval，心跳停止，超过 lockTimeout 后自动锁定。
    const heartbeatInterval = window.setInterval(() => {
      invoke("vault_heartbeat").catch(() => {
        // 静默失败（可能在锁定 / 测试中）
      });
    }, 30 * 1000);

    // unmount 时主动锁定保险库——关闭设置窗口 / 切换到其他设置 tab
    // 都会触发 unmount，确保离开页面后必须重新输主密码（防偷窥）。
    return () => {
      window.clearInterval(heartbeatInterval);
      invoke("vault_lock").catch(() => {
        // 静默失败：可能在测试 / 关闭 app 期间，无需打扰用户
      });
    };
  }, [refreshStatus]);

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
  if (!status.user_vault_unlocked) {
    return <UnlockDialog onSuccess={refreshStatus} showToast={showToast} />;
  }

  return (
    <div className="flex h-full flex-col gap-4">
      <div className="flex items-center justify-between">
        <div>
          <h2 className="flex items-center gap-2 text-xl font-semibold">
            <ShieldCheck className="size-5" />
            {t("settings.vault.title")}
          </h2>
          <p className="text-sm text-muted-foreground">
            {t("settings.vault.description")}
          </p>
        </div>
        <div className="flex items-center gap-3">
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
                className="border bg-background px-2 py-1 text-xs rounded"
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
              <span className="text-xs text-amber-600 dark:text-amber-400 max-w-[260px] text-right">
                {t("settings.vault.lockTimeoutWarning")}
              </span>
            )}
          </div>
          <Button variant="outline" size="sm" onClick={handleLock}>
            <Lock />
            {t("settings.vault.lock")}
          </Button>
        </div>
      </div>

      <UnderlineTabs
        items={[
          { key: "ciphers", label: t("settings.vault.list.title") },
          { key: "health", label: t("settings.vault.health.title") },
          { key: "io", label: t("settings.vault.importExport.title") },
        ]}
        active={tab}
        onChange={(k) => setTab(k as Tab)}
      />

      <div className="min-h-0 flex-1">
        {tab === "ciphers" && <CipherList showToast={showToast} />}
        {tab === "health" && <HealthReport showToast={showToast} />}
        {tab === "io" && <ImportExport showToast={showToast} />}
      </div>
    </div>
  );
}
