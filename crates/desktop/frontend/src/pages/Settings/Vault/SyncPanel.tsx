import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { RefreshCw, GitBranch, Check, AlertCircle } from "lucide-react";
import { useT } from "@/lib/i18n";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Row } from "@/components/ui/row";

/**
 * SyncPanel —— 密码箱 Git 同步设置面板。
 *
 * 2026-07-21 Phase 1：手动触发同步，SSH key 认证（系统已配）。
 *
 * 状态机：
 *   gitAvailable=false → 不渲染（整个面板隐藏）
 *   !initialized → 显示「启用同步」按钮 → 展开 remote URL 输入
 *   initialized → 显示状态 + 「立即同步」+「禁用」
 */

interface SyncStatus {
  git_available: boolean;
  initialized: boolean;
  remotes: [string, string][];
  last_sync: string | null;
  last_commit_sha: string | null;
}

interface SyncReport {
  pulled: number;
  pushed: number;
  deleted: number;
  message: string;
}

export default function SyncPanel({ showToast }: { showToast: (msg: string) => void }) {
  const t = useT();
  const [status, setStatus] = useState<SyncStatus | null>(null);
  const [remoteUrl, setRemoteUrl] = useState("");
  const [giteeUrl, setGiteeUrl] = useState("");
  const [busy, setBusy] = useState(false);

  const refreshStatus = useCallback(async () => {
    try {
      const s = await invoke<SyncStatus>("vault_sync_status");
      setStatus(s);
    } catch (e) {
      console.error("vault_sync_status failed:", e);
    }
  }, []);

  useEffect(() => {
    refreshStatus();
  }, [refreshStatus]);

  const handleEnable = useCallback(async () => {
    if (!remoteUrl.trim()) return;
    setBusy(true);
    try {
      await invoke("vault_sync_enable", {
        remoteUrl: remoteUrl.trim(),
        giteeUrl: giteeUrl.trim() || null,
      });
      showToast(t("settings.vault.sync.enableSuccess"));
      await refreshStatus();
    } catch (e) {
      showToast(String(e));
    } finally {
      setBusy(false);
    }
  }, [remoteUrl, giteeUrl, showToast, t, refreshStatus]);

  const handleTestConnection = useCallback(async () => {
    if (!remoteUrl.trim()) return;
    setBusy(true);
    try {
      await invoke("vault_sync_test_connection", { remoteUrl: remoteUrl.trim() });
      showToast(t("settings.vault.sync.connectionOk"));
    } catch (e) {
      showToast(String(e));
    } finally {
      setBusy(false);
    }
  }, [remoteUrl, showToast, t]);

  const handleSyncNow = useCallback(async () => {
    setBusy(true);
    try {
      const report = await invoke<SyncReport>("vault_sync_now");
      showToast(report.message || t("settings.vault.sync.syncSuccess"));
      await refreshStatus();
    } catch (e) {
      showToast(String(e));
    } finally {
      setBusy(false);
    }
  }, [showToast, t, refreshStatus]);

  const handleDisable = useCallback(async () => {
    if (!confirm(t("settings.vault.sync.disableConfirm"))) return;
    setBusy(true);
    try {
      await invoke("vault_sync_disable");
      showToast(t("settings.vault.sync.notInitialized"));
      await refreshStatus();
    } catch (e) {
      showToast(String(e));
    } finally {
      setBusy(false);
    }
  }, [showToast, t, refreshStatus]);

  // git 不可用时不渲染
  if (status && !status.git_available) {
    return (
      <div className="flex items-center gap-2 rounded-md border border-warning/40 bg-warning/5 px-3 py-2 text-xs text-warning">
        <AlertCircle className="size-3.5 shrink-0" />
        <span>{t("settings.vault.sync.notAvailable")}</span>
      </div>
    );
  }

  if (!status) {
    return (
      <div className="text-xs text-muted-foreground">{t("settings.loading")}</div>
    );
  }

  // 已初始化——显示状态 + 操作
  if (status.initialized) {
    return (
      <div className="space-y-3">
        {/* 状态行 */}
        <div className="flex items-center gap-2 text-xs">
          <GitBranch className="size-3.5 text-muted-foreground" />
          <span className="font-medium text-foreground">
            {t("settings.vault.sync.enabled")}
          </span>
          {status.last_sync && (
            <span className="text-muted-foreground">
              · {t("settings.vault.sync.lastSync")}: {status.last_sync.replace("T", " ").replace(/\+.*/, "")}
            </span>
          )}
        </div>

        {/* Remote 列表 */}
        {status.remotes.length > 0 && (
          <div className="space-y-1">
            {status.remotes.map(([name, url]) => (
              <div key={name} className="flex items-center gap-2 text-xs text-muted-foreground">
                <span className="font-mono">{name}:</span>
                <span className="truncate">{url}</span>
              </div>
            ))}
          </div>
        )}

        {/* 操作按钮 */}
        <div className="flex items-center gap-2">
          <Button
            variant="voice"
            size="sm"
            onClick={handleSyncNow}
            disabled={busy}
          >
            {busy ? (
              t("settings.vault.sync.syncing")
            ) : (
              <>
                <RefreshCw className="size-3.5" />
                {t("settings.vault.sync.syncNow")}
              </>
            )}
          </Button>
          <Button
            variant="outline"
            size="sm"
            onClick={handleDisable}
            disabled={busy}
          >
            {t("settings.vault.sync.disable")}
          </Button>
        </div>
      </div>
    );
  }

  // 未初始化——配置表单
  return (
    <div className="space-y-3">
      <div className="flex items-center gap-2 text-xs">
        <GitBranch className="size-3.5 text-muted-foreground" />
        <span className="font-medium text-foreground">
          {t("settings.vault.sync.title")}
        </span>
        <span className="text-muted-foreground">
          · {t("settings.vault.sync.notInitialized")}
        </span>
      </div>

      <Row label={t("settings.vault.sync.remoteUrl")}>
        <Input
          size="full"
          value={remoteUrl}
          onChange={(e) => setRemoteUrl(e.target.value)}
          placeholder={t("settings.vault.sync.remoteUrlPlaceholder")}
          className="font-mono text-xs"
          spellCheck={false}
          autoComplete="off"
        />
      </Row>

      <Row label={t("settings.vault.sync.giteeUrl")}>
        <Input
          size="full"
          value={giteeUrl}
          onChange={(e) => setGiteeUrl(e.target.value)}
          placeholder={t("settings.vault.sync.giteeUrlPlaceholder")}
          className="font-mono text-xs"
          spellCheck={false}
          autoComplete="off"
        />
      </Row>

      {/* SSH 提示 */}
      <p className="text-xs text-muted-foreground">
        {t("settings.vault.sync.sshHint")}
      </p>

      {/* 操作按钮 */}
      <div className="flex items-center gap-2">
        <Button
          variant="outline"
          size="sm"
          onClick={handleTestConnection}
          disabled={busy || !remoteUrl.trim()}
        >
          {busy ? t("settings.vault.sync.testing") : (
            <>
              <Check className="size-3.5" />
              {t("settings.vault.sync.testConnection")}
            </>
          )}
        </Button>
        <Button
          variant="voice"
          size="sm"
          onClick={handleEnable}
          disabled={busy || !remoteUrl.trim()}
        >
          {busy ? "..." : t("settings.vault.sync.enable")}
        </Button>
      </div>
    </div>
  );
}
