import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { RefreshCw, Plus, Trash2, GitBranch, Download, AlertCircle } from "lucide-react";
import { useT } from "@/lib/i18n";
import type { ToastVariant } from "@/lib/useToast";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { PasswordInput } from "@/components/ui/password-input";

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
  gitAvailable: boolean;
  initialized: boolean;
  remotes: [string, string][];
  lastSync: string | null;
  last_commit_sha: string | null;
  /** 当前是否在后台同步——UI 据此显进度条（2026-07-21） */
  syncing: boolean;
  /** 最近一次自动同步结果（Phase 2，scheduler 每小时触发） */
  last_auto_sync: LastAutoSync | null;
}

interface LastAutoSync {
  timestamp: string;
  success: boolean;
  message: string;
}

/** vault-sync-done 事件 payload */
interface SyncDonePayload {
  report: SyncReport | null;
  error: string | null;
}

interface SyncReport {
  pulled: number;
  pushed: number;
  deleted: number;
  hotwordsPulled: number;
  hotwordsPushed: number;
  /** push 失败的 remote → 错误消息（#4 修复——之前 push 失败被吞，谎报「已推送」）。
   * 空 = 全部成功；非空 = 部分 remote 失败（本地已 commit，未上云）。 */
  pushErrors: [string, string][];
  /** pull 阶段因文件损坏被跳过的条目数（#10——不再静默吞错）。 */
  skipped: number;
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
  // sync 错误（存 state 而非仅 toast——stamp 冲突时需据此显示冲突解决 UI）
  const [syncError, setSyncError] = useState<string | null>(null);
  // 冲突解决：resolveMode 控制密码输入展开（null / "remote" / "local"）
  const [resolveMode, setResolveMode] = useState<"remote" | "local" | null>(null);
  const [resolvePwd, setResolvePwd] = useState("");
  const [resolving, setResolving] = useState(false);

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

  // === 后台同步完成事件（2026-07-21）===
  // vault_sync_now 是 async spawn——命令秒回，结果通过 vault-sync-done 事件投递。
  // 用户可能在同步期间切走 / 关窗 / 重开——listen 在 mount 时注册，重开会重新订阅。
  // syncing 状态从 status.syncing（后端 AtomicBool）查得，不依赖事件保活。
  useEffect(() => {
    const unlisten = listen<SyncDonePayload>("vault-sync-done", (event) => {
      const { report, error } = event.payload;
      if (error) {
        showToast(error, "error");
        setSyncError(error);
      } else if (report) {
        // #4 修复：push 部分 remote 失败时用 warning（而非 success），
        // 让用户知道数据未上云。后端 message 已含失败 remote 名 + 「未上云」提示。
        const variant =
          report.pushErrors.length > 0 ? "warning" : "success";
        showToast(report.message || t("settings.vault.sync.syncSuccess"), variant);
        setSyncError(null);
      }
      // 刷新状态——status.syncing 会变回 false
      refreshStatus();
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [showToast, t, refreshStatus]);

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

  // 立即同步——非阻塞触发，结果通过 vault-sync-done 事件投递（2026-07-21）。
  // 命令本身秒回，UI 立即切到进度条状态（status.syncing），用户可继续其他操作。
  const handleSyncNow = useCallback(async () => {
    // 乐观更新 UI——立即显示进度条（不等下次 status 查询）
    setStatus((prev) => (prev ? { ...prev, syncing: true } : prev));
    try {
      await invoke("vault_sync_now");
      // 不在这里 showToast——结果由 vault-sync-done 事件回调处理
    } catch (e) {
      // 启动失败（极少见：spawn_blocking 错）——回滚 syncing + 显示错误
      setStatus((prev) => (prev ? { ...prev, syncing: false } : prev));
      showToast(String(e), "error");
    }
  }, []);

  // === stamp 冲突解决（2026-07-22）===
  const handleResolve = useCallback(async () => {
    if (!resolveMode || !resolvePwd) return;
    setResolving(true);
    try {
      const cmd = resolveMode === "remote"
        ? "vault_sync_resolve_remote"
        : "vault_sync_resolve_local";
      await invoke(cmd, { password: resolvePwd });
      showToast(t("settings.vault.sync.resolveSuccess"), "success");
      // 清理冲突态
      setSyncError(null);
      setResolveMode(null);
      setResolvePwd("");
      // 重新同步
      setStatus((prev) => (prev ? { ...prev, syncing: true } : prev));
      await invoke("vault_sync_now");
    } catch (e) {
      showToast(t("settings.vault.sync.resolveFailed") + String(e), "error");
    }
    setResolving(false);
  }, [resolveMode, resolvePwd, showToast, t]);

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
  if (status && !status.gitAvailable) {
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
        </div>

        {/* 功能说明 */}
        <div className="space-y-2 rounded-lg border border-border/50 bg-muted/30 p-4">
          <p className="text-xs text-foreground/80">
            {t("settings.vault.sync.introWhat")}
          </p>
          <ul className="space-y-1 text-[11px] text-muted-foreground">
            <li>• {t("settings.vault.sync.introItem1")}</li>
            <li>• {t("settings.vault.sync.introItem2")}</li>
            <li>• {t("settings.vault.sync.introItem3")}</li>
          </ul>
          <p className="text-[11px] text-muted-foreground/70">
            {t("settings.vault.sync.introPrereq")}
          </p>
        </div>

        {/* 首次推送（A 机） */}
        <div className="space-y-2 rounded-lg border border-border/50 p-4">
          <p className="text-xs font-medium text-foreground">
            {t("settings.vault.sync.firstPushTitle")}
          </p>
          <p className="text-xs text-muted-foreground">
            {t("settings.vault.sync.firstPushDesc")}
          </p>
          <Button variant="voice" className="w-full" onClick={handleEnable} disabled={busy}>
            {busy ? "..." : t("settings.vault.sync.enable")}
          </Button>
        </div>

        {/* 从远程克隆（B 机） */}
        <div className="space-y-2 rounded-lg border border-border/50 p-4">
          <p className="text-xs font-medium text-foreground">
            {t("settings.vault.sync.cloneTitle")}
          </p>
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

        {/* SSH 提示 */}
        <p className="text-center text-xs text-muted-foreground/70">
          {t("settings.vault.sync.sshHint")}
        </p>
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
          {status.lastSync && (
            <span className="text-muted-foreground">
              · {t("settings.vault.sync.lastSync")}:{" "}
              {status.lastSync.replace("T", " ").replace(/\+.*/, "")}
            </span>
          )}
          {status.last_auto_sync && (
            <span className={status.last_auto_sync.success ? "text-muted-foreground" : "text-destructive"}>
              · {t("settings.vault.sync.autoSync")}:{" "}
              {status.last_auto_sync.success
                ? t("settings.vault.sync.autoSyncOk")
                : t("settings.vault.sync.autoSyncFail")}
              {" "}
              {status.last_auto_sync.timestamp.replace("T", " ").replace(/\+.*/, "").slice(5, 16)}
            </span>
          )}
        </div>
        <div className="flex items-center gap-2">
          {status.syncing ? (
            // 同步中——indeterminate 进度条 + 文案，不阻塞其他操作（2026-07-21）
            <div
              className="flex h-8 min-w-[140px] items-center gap-2 rounded-md border border-voice/30 bg-voice/10 px-3 text-xs font-medium text-voice"
              role="status"
              aria-live="polite"
            >
              <span>{t("settings.vault.sync.syncing")}</span>
              <div className="relative h-1 flex-1 overflow-hidden rounded-full bg-voice/20">
                <div className="vault-sync-progress-bar absolute inset-y-0 w-1/3 rounded-full bg-voice" />
              </div>
            </div>
          ) : (
            <Button
              variant="voice"
              size="sm"
              onClick={handleSyncNow}
              disabled={busy}
            >
              <RefreshCw className="size-3.5" />
              {t("settings.vault.sync.syncNow")}
            </Button>
          )}
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

      {/* stamp 冲突解决——sync 失败且 error 含「主密码」时显示。
          两种错误共用此 UI：
            - MasterPasswordMismatch "远程 vault 用了不同主密码"——可「以远程/本地为准」二选一
            - EmptyRecoveryNeedsPassword "本地空库恢复需确认源机器主密码"——本地空，只能「以远程为准」
          用「空库恢复」关键字区分（空库场景隐藏「以本地为主」按钮——本地空没什么可为主的，
          且会让远程 meta 被空状态覆盖导致 cipher pull 回来解不开）。 */}
      {syncError && syncError.includes("主密码") && (
        <div className="space-y-3 rounded-lg border border-destructive/30 bg-destructive/5 p-4">
          <div className="flex items-start gap-2">
            <AlertCircle className="mt-0.5 size-4 shrink-0 text-destructive" />
            <div className="space-y-1">
              <p className="text-xs font-medium text-destructive">
                {t("settings.vault.sync.conflictTitle")}
              </p>
              <p className="text-[11px] text-muted-foreground">
                {t("settings.vault.sync.conflictDesc")}
              </p>
            </div>
          </div>

          {resolveMode === null ? (
            <div className="flex gap-2">
              <Button
                variant="outline"
                size="sm"
                className="flex-1"
                onClick={() => setResolveMode("remote")}
              >
                {t("settings.vault.sync.useRemote")}
              </Button>
              {/* 空库恢复场景（EmptyRecoveryNeedsPassword）隐藏「以本地为主」——本地空，
                  选了会把远程 meta 覆盖成空状态 + cipher pull 回来解不开。 */}
              {!syncError.includes("空库恢复") && (
                <Button
                  variant="outline"
                  size="sm"
                  className="flex-1"
                  onClick={() => setResolveMode("local")}
                >
                  {t("settings.vault.sync.useLocal")}
                </Button>
              )}
            </div>
          ) : (
            <div className="space-y-2">
              <p className="text-[11px] text-muted-foreground">
                {resolveMode === "remote"
                  ? t("settings.vault.sync.useRemoteHint")
                  : t("settings.vault.sync.useLocalHint")}
              </p>
              <PasswordInput
                variant="default"
                size="full"
                value={resolvePwd}
                onChange={(e) => setResolvePwd(e.target.value)}
                onKeyDown={(e) => e.key === "Enter" && handleResolve()}
                placeholder={t("settings.vault.sync.resolvePwdPlaceholder")}
                autoFocus
              />
              <div className="flex gap-2">
                <Button
                  variant="outline"
                  size="sm"
                  onClick={() => { setResolveMode(null); setResolvePwd(""); }}
                >
                  {t("settings.vault.sync.resolveBack")}
                </Button>
                <Button
                  variant="voice"
                  size="sm"
                  onClick={handleResolve}
                  disabled={!resolvePwd || resolving}
                >
                  {resolving ? "..." : t("settings.vault.sync.resolveConfirm")}
                </Button>
              </div>
            </div>
          )}
        </div>
      )}

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
