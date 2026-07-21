import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { RefreshCw, Plus, Trash2, GitBranch, Download, AlertCircle } from "lucide-react";
import { useT } from "@/lib/i18n";
import type { ToastVariant } from "@/lib/useToast";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Row } from "@/components/ui/row";

/**
 * SyncPanel —— 密码箱 Git 同步设置面板（独立 Tab）。
 *
 * 2026-07-21 Phase 1：手动触发同步，SSH key 认证（系统已配）。
 *
 * UI 三态：
 *   1. git 不可用 → 提示
 *   2. 未初始化 → 「启用同步」按钮 + 「从远程克隆」按钮
 *   3. 已初始化 → Remote 列表（增删）+ 立即同步 + 禁用
 *
 * Remote 管理是列表式的——不写死 GitHub/Gitee，用户自由输入任意 git 地址。
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

export default function SyncPanel({
  showToast,
}: {
  showToast: (msg: string, variant?: ToastVariant) => void;
}) {
  const t = useT();
  const [status, setStatus] = useState<SyncStatus | null>(null);
  const [busy, setBusy] = useState(false);

  // Remote 列表状态
  const [remotes, setRemotes] = useState<[string, string][]>([]);
  const [newRemoteName, setNewRemoteName] = useState("");
  const [newRemoteUrl, setNewRemoteUrl] = useState("");

  // Clone 表单状态
  const [cloneUrl, setCloneUrl] = useState("");
  const [showClone, setShowClone] = useState(false);

  const refreshStatus = useCallback(async () => {
    try {
      const s = await invoke<SyncStatus>("vault_sync_status");
      setStatus(s);
      if (s.initialized) {
        const r = await invoke<[string, string][]>("vault_sync_list_remotes");
        setRemotes(r);
      }
    } catch (e) {
      console.error("vault_sync_status failed:", e);
    }
  }, []);

  useEffect(() => {
    refreshStatus();
  }, [refreshStatus]);

  // === 操作 ===

  const handleEnable = useCallback(async () => {
    setBusy(true);
    try {
      await invoke("vault_sync_enable");
      showToast(t("settings.vault.sync.enableSuccess"));
      await refreshStatus();
    } catch (e) {
      showToast(String(e), "error");
    } finally {
      setBusy(false);
    }
  }, [showToast, t, refreshStatus]);

  const handleClone = useCallback(async () => {
    if (!cloneUrl.trim()) return;
    setBusy(true);
    try {
      await invoke("vault_sync_clone", { remoteUrl: cloneUrl.trim() });
      showToast(t("settings.vault.sync.enableSuccess"));
      await refreshStatus();
    } catch (e) {
      showToast(String(e), "error");
    } finally {
      setBusy(false);
    }
  }, [cloneUrl, showToast, t, refreshStatus]);

  const handleSyncNow = useCallback(async () => {
    setBusy(true);
    try {
      const report = await invoke<SyncReport>("vault_sync_now");
      showToast(report.message || t("settings.vault.sync.syncSuccess"));
      await refreshStatus();
    } catch (e) {
      showToast(String(e), "error");
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
      setRemotes([]);
      await refreshStatus();
    } catch (e) {
      showToast(String(e), "error");
    } finally {
      setBusy(false);
    }
  }, [showToast, t, refreshStatus]);

  const handleAddRemote = useCallback(async () => {
    const name = newRemoteName.trim() || "origin";
    const url = newRemoteUrl.trim();
    if (!url) return;
    setBusy(true);
    try {
      await invoke("vault_sync_add_remote", { name, url });
      setNewRemoteName("");
      setNewRemoteUrl("");
      const r = await invoke<[string, string][]>("vault_sync_list_remotes");
      setRemotes(r);
      showToast(t("settings.vault.sync.remoteAdded"));
    } catch (e) {
      // 私有库检测失败 / git 错误等——直接展示后端 Display 字符串（含用户可读建议）
      showToast(String(e), "error");
    } finally {
      setBusy(false);
    }
  }, [newRemoteName, newRemoteUrl, showToast, t]);

  const handleRemoveRemote = useCallback(
    async (name: string) => {
      setBusy(true);
      try {
        await invoke("vault_sync_remove_remote", { name });
        const r = await invoke<[string, string][]>("vault_sync_list_remotes");
        setRemotes(r);
      } catch (e) {
        showToast(String(e), "error");
      } finally {
        setBusy(false);
      }
    },
    [showToast],
  );

  const handleTestRemote = useCallback(
    async (url: string) => {
      setBusy(true);
      try {
        await invoke("vault_sync_test_connection", { remoteUrl: url });
        showToast(t("settings.vault.sync.connectionOk"));
      } catch (e) {
        showToast(String(e), "error");
      } finally {
        setBusy(false);
      }
    },
    [showToast, t],
  );

  // === 渲染 ===

  // git 不可用
  if (status && !status.git_available) {
    return (
      <div className="flex h-full items-center justify-center">
        <div className="flex items-center gap-2 text-sm text-warning">
          <AlertCircle className="size-4" />
          <span>{t("settings.vault.sync.notAvailable")}</span>
        </div>
      </div>
    );
  }

  if (!status) {
    return (
      <div className="p-4 text-sm text-muted-foreground">{t("settings.loading")}</div>
    );
  }

  // 未初始化——选择「启用」或「克隆」
  if (!status.initialized) {
    return (
      <div className="mx-auto max-w-md space-y-6 p-6">
        <div className="space-y-2 text-center">
          <GitBranch className="mx-auto size-8 text-muted-foreground/50" />
          <h3 className="text-sm font-medium">{t("settings.vault.sync.title")}</h3>
          <p className="text-xs text-muted-foreground">
            {t("settings.vault.sync.sshHint")}
          </p>
        </div>

        {/* 首次推送（A 机） */}
        <div className="space-y-2 rounded-lg border border-border/50 p-4">
          <p className="text-xs text-muted-foreground">
            {t("settings.vault.sync.firstPushDesc")}
          </p>
          <Button variant="voice" className="w-full" onClick={handleEnable} disabled={busy}>
            {busy ? "..." : t("settings.vault.sync.enable")}
          </Button>
        </div>

        {/* 从远程克隆（B 机） */}
        <div className="space-y-2 rounded-lg border border-border/50 p-4">
          <p className="text-xs text-muted-foreground">
            {t("settings.vault.sync.cloneDesc")}
          </p>
          {showClone ? (
            <>
              <Input
                size="full"
                value={cloneUrl}
                onChange={(e) => setCloneUrl(e.target.value)}
                placeholder={t("settings.vault.sync.remoteUrlPlaceholder")}
                className="font-mono text-xs"
                spellCheck={false}
                autoComplete="off"
              />
              <div className="flex gap-2">
                <Button
                  variant="voice"
                  size="sm"
                  className="flex-1"
                  onClick={handleClone}
                  disabled={busy || !cloneUrl.trim()}
                >
                  {busy ? (
                    t("settings.vault.sync.checkingPrivacy")
                  ) : (
                    t("settings.vault.sync.clone")
                  )}
                </Button>
                <Button
                  variant="outline"
                  size="sm"
                  onClick={() => setShowClone(false)}
                  disabled={busy}
                >
                  ✕
                </Button>
              </div>
              <p className="text-xs text-muted-foreground/70">
                {t("settings.vault.sync.privacyHint")}
              </p>
            </>
          ) : (
            <Button
              variant="outline"
              className="w-full"
              onClick={() => setShowClone(true)}
            >
              <Download className="size-3.5" />
              {t("settings.vault.sync.cloneFromRemote")}
            </Button>
          )}
        </div>
      </div>
    );
  }

  // 已初始化——Remote 列表 + 操作
  return (
    <div className="mx-auto max-w-2xl space-y-4 p-4">
      {/* 状态行 */}
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-2 text-xs">
          <GitBranch className="size-3.5 text-muted-foreground" />
          <span className="font-medium text-foreground">
            {t("settings.vault.sync.enabled")}
          </span>
          {status.last_sync && (
            <span className="text-muted-foreground">
              · {t("settings.vault.sync.lastSync")}:{" "}
              {status.last_sync.replace("T", " ").replace(/\+.*/, "")}
            </span>
          )}
        </div>
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

      {/* Remote 列表 */}
      <div className="space-y-2">
        <div className="text-xs font-medium uppercase tracking-wide text-muted-foreground/80">
          {t("settings.vault.sync.remotes")}
        </div>

        {remotes.length === 0 ? (
          <p className="rounded-md border border-dashed border-border/50 px-3 py-4 text-center text-xs text-muted-foreground">
            {t("settings.vault.sync.noRemotes")}
          </p>
        ) : (
          <div className="space-y-1">
            {remotes.map(([name, url]) => (
              <div
                key={name}
                className="flex items-center gap-2 rounded-md border border-border/40 px-3 py-2"
              >
                <span className="shrink-0 font-mono text-xs font-medium text-foreground">
                  {name}
                </span>
                <span className="min-w-0 flex-1 truncate font-mono text-xs text-muted-foreground">
                  {url}
                </span>
                <Button
                  variant="ghost"
                  size="icon-sm"
                  onClick={() => handleTestRemote(url)}
                  disabled={busy}
                  title={t("settings.vault.sync.testConnection")}
                  className="shrink-0"
                >
                  <RefreshCw className="size-3.5" />
                </Button>
                <Button
                  variant="ghost"
                  size="icon-sm"
                  onClick={() => handleRemoveRemote(name)}
                  disabled={busy}
                  title={t("settings.vault.sync.removeRemote")}
                  className="shrink-0 text-muted-foreground hover:text-destructive"
                >
                  <Trash2 className="size-3.5" />
                </Button>
              </div>
            ))}
          </div>
        )}

        {/* 添加 remote */}
        <div className="flex items-center gap-2 pt-2">
          <Input
            value={newRemoteName}
            onChange={(e) => setNewRemoteName(e.target.value)}
            placeholder="origin"
            className="w-24 shrink-0 font-mono text-xs"
            spellCheck={false}
            autoComplete="off"
          />
          <Input
            size="full"
            value={newRemoteUrl}
            onChange={(e) => setNewRemoteUrl(e.target.value)}
            placeholder={t("settings.vault.sync.remoteUrlPlaceholder")}
            className="min-w-0 flex-1 font-mono text-xs"
            spellCheck={false}
            autoComplete="off"
          />
          <Button
            variant="outline"
            size="icon-sm"
            onClick={handleAddRemote}
            disabled={busy || !newRemoteUrl.trim()}
            title={t("settings.vault.sync.addRemote")}
            className="shrink-0"
          >
            {busy ? (
              <RefreshCw className="size-4 animate-spin" />
            ) : (
              <Plus className="size-4" />
            )}
          </Button>
        </div>

        {/* 私有库检测提示（2026-07-21）——常驻，避免用户输公有库被拒后困惑 */}
        <p className="text-xs text-muted-foreground/70">
          {t("settings.vault.sync.privacyHint")}
        </p>
      </div>

      {/* SSH 提示 */}
      <p className="text-xs text-muted-foreground/70">
        {t("settings.vault.sync.sshHint")}
      </p>
    </div>
  );
}
