/**
 * PermissionGate —— 录屏权限检查 + 申请 banner（spec §8.3 简化版）。
 *
 * mount 时调 check_record_permission()，根据返回状态显示 banner：
 * - granted：渲染 children（无 banner）。
 * - not-determined：渲染 amber banner + [申请权限] 按钮 → request_screen_record_permission()，
 *   申请成功后重检（macOS CG.request 会触发系统弹窗）。
 * - denied：渲染 destructive banner + [打开系统设置] 按钮 → open_privacy_settings({section: "screen_capture"})。
 *
 * 后端 PermissionStatus 序列化用 lowercase（crates/record/src/protocol.rs）：
 * "granted" / "denied" / "not-determined"。
 *
 * 视觉风格沿用 octopus Settings 既有 panel 的 banner 规范（参考 HistoryPanel 的
 * amber-600/10 + amber-700 文案色 + destructive 系列的 destructive/10 + text-destructive）。
 */
import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ShieldAlert, ShieldCheck } from "lucide-react";
import { Button } from "@/components/ui/button";
import { useT } from "@/lib/i18n";
import { cn } from "@/lib/utils";

type PermissionStatus = "granted" | "denied" | "notDetermined";

interface PermissionGateProps {
  /** 授权通过时渲染的内容。 */
  children: React.ReactNode;
  /** 权限检查失败时的回调（toast 反馈）。 */
  onError?: (msg: string) => void;
  /** 权限状态变化时回调（供父组件感知，例如 banner 控制空状态文案）。 */
  onStatusChange?: (status: PermissionStatus) => void;
}

export function PermissionGate({
  children,
  onError,
  onStatusChange,
}: PermissionGateProps) {
  const t = useT();
  const [status, setStatus] = useState<PermissionStatus | null>(null);

  const refresh = useCallback(async () => {
    try {
      const s = await invoke<PermissionStatus>("check_record_permission");
      setStatus(s);
      onStatusChange?.(s);
    } catch (e) {
      onError?.(t("settings.recordings.permission.checkFailed") + e);
      // 检查失败时按 not-determined 渲染——让用户能看到提示而非白屏
      setStatus("notDetermined");
    }
  }, [onError, onStatusChange, t]);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const handleRequest = useCallback(async () => {
    try {
      const s = await invoke<PermissionStatus>(
        "request_screen_record_permission",
      );
      setStatus(s);
      onStatusChange?.(s);
      // macOS 申请弹窗后用户操作完应再 check 一次确认实际状态
      if (s !== "granted") {
        setTimeout(refresh, 500);
      }
    } catch (e) {
      onError?.(t("settings.recordings.permission.checkFailed") + e);
    }
  }, [refresh, onError, onStatusChange, t]);

  const handleOpenSettings = useCallback(async () => {
    try {
      await invoke("open_privacy_settings", { section: "screenCapture" });
    } catch (e) {
      onError?.(t("settings.recordings.permission.checkFailed") + e);
    }
  }, [onError, t]);

  // granted 时无 banner，直接渲染 children
  if (status === "granted") {
    return <>{children}</>;
  }

  // not-determined / denied / loading 都渲染 banner，但仍渲染 children
  // （让用户能浏览已有录屏列表，仅阻止启动新录制——RecordingPanel 自身不启动录制）
  const denied = status === "denied";

  return (
    <>
      <div
        className={cn(
          "flex items-center gap-2 px-2.5 py-1.5 rounded-md border text-xs",
          denied
            ? "border-destructive/50 bg-destructive/10 text-destructive"
            : "border-amber-600/40 bg-amber-600/10 text-amber-700 dark:text-amber-500",
        )}
        role="alert"
      >
        {denied ? (
          <ShieldAlert className="w-3.5 h-3.5 flex-shrink-0" />
        ) : (
          <ShieldCheck className="w-3.5 h-3.5 flex-shrink-0" />
        )}
        <span className="flex-1 min-w-0">
          {denied
            ? t("settings.recordings.permission.denied")
            : t("settings.recordings.permission.notDetermined")}
        </span>
        <Button
          variant="outline"
          size="sm"
          onClick={denied ? handleOpenSettings : handleRequest}
          className="shrink-0"
        >
          {denied
            ? t("settings.recordings.permission.deniedAction")
            : t("settings.recordings.permission.notDeterminedAction")}
        </Button>
      </div>
      {children}
    </>
  );
}
