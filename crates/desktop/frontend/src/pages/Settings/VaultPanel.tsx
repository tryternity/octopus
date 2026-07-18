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
  }, [refreshStatus]);

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
        <Button variant="outline" size="sm" onClick={handleLock}>
          <Lock />
          {t("settings.vault.lock")}
        </Button>
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
