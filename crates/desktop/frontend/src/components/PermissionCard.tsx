/**
 * PermissionCard —— 通用权限卡片（麦克风/辅助功能/屏幕录制）。
 *
 * 从 Onboarding 抽取的共享组件，Onboarding 引导页 + Settings 隐私与权限 tab 复用。
 * 复用 PermissionGate 的 refresh/request/openSettings 模式，参数化 by PermissionDef。
 *
 * 权限状态（PermissionStatus，camelCase，与后端 record::protocol::PermissionStatus 一致）：
 * - granted：绿勾，无按钮
 * - denied：红色，[打开系统设置] 按钮
 * - notDetermined：琥珀，[申请权限] 按钮
 */
import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Mic, Accessibility, Monitor, Bot, CheckCircle2, XCircle, AlertCircle } from "lucide-react";
import { Button } from "@/components/ui/button";
import { useT } from "@/lib/i18n";
import { cn } from "@/lib/utils";

export type PermissionStatus = "granted" | "denied" | "notDetermined";

/** 权限类型 → 对应的 check/request 命令名 + i18n key + 图标 + PrivacySection。 */
export interface PermissionDef {
  key: "microphone" | "accessibility" | "screen" | "automation";
  icon: React.ComponentType<{ className?: string }>;
  checkCmd: string;
  requestCmd: string;
  /** open_privacy_settings 的 section 参数（PrivacySection camelCase variant）。 */
  privacySection: string;
}

/** 4 个 macOS 权限定义（Onboarding + Settings tab 共用）。 */
export const PERMISSIONS: PermissionDef[] = [
  {
    key: "microphone",
    icon: Mic,
    checkCmd: "check_microphone_permission",
    requestCmd: "request_microphone_permission",
    privacySection: "microphone",
  },
  {
    key: "accessibility",
    icon: Accessibility,
    checkCmd: "check_accessibility_permission",
    requestCmd: "request_accessibility_permission",
    privacySection: "accessibility",
  },
  {
    key: "screen",
    icon: Monitor,
    checkCmd: "check_record_permission",
    requestCmd: "request_screen_record_permission",
    privacySection: "screenCapture",
  },
  {
    key: "automation",
    icon: Bot,
    checkCmd: "check_automation_permission",
    requestCmd: "request_automation_permission",
    privacySection: "automation",
  },
];

/** 单个权限卡片——parametrize by PermissionDef。 */
export function PermissionCard({ def }: { def: PermissionDef }) {
  const t = useT();
  const [status, setStatus] = useState<PermissionStatus | null>(null);
  const Icon = def.icon;

  const refresh = useCallback(async () => {
    try {
      const s = await invoke<PermissionStatus>(def.checkCmd);
      setStatus(s);
    } catch {
      setStatus("notDetermined");
    }
  }, [def.checkCmd]);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const handleRequest = useCallback(async () => {
    try {
      const s = await invoke<PermissionStatus>(def.requestCmd);
      setStatus(s);
      // macOS TCC 弹窗异步——用户操作完重检确认实际状态
      if (s !== "granted") {
        setTimeout(refresh, 800);
      }
    } catch {
      setStatus("notDetermined");
    }
  }, [def.requestCmd, refresh]);

  const handleOpenSettings = useCallback(async () => {
    try {
      await invoke("open_privacy_settings", { section: def.privacySection });
      // 用户从系统设置回来后重检
      setTimeout(refresh, 1500);
    } catch {
      // ignore
    }
  }, [def.privacySection, refresh]);

  const granted = status === "granted";
  const denied = status === "denied";

  return (
    <div
      className={cn(
        "group relative flex items-center gap-3 px-4 py-3 rounded-lg transition-colors",
        granted
          ? "hover:bg-muted/40"
          : denied
            ? "bg-destructive/5 hover:bg-destructive/10"
            : "bg-amber-600/5 hover:bg-amber-600/10",
      )}
    >
      <div className={cn("flex-shrink-0", granted ? "text-emerald-600" : denied ? "text-destructive" : "text-amber-600")}>
        <Icon className="w-5 h-5" />
      </div>
      <div className="flex-1 min-w-0">
        <div className="flex items-center gap-2">
          <span className="text-sm font-medium text-foreground">
            {t(`onboarding.permissions.${def.key}.title`)}
          </span>
          {granted ? (
            <span className="inline-flex items-center gap-0.5 text-[10px] text-emerald-600">
              <CheckCircle2 className="w-3 h-3" />
              {t("onboarding.status.granted")}
            </span>
          ) : denied ? (
            <span className="inline-flex items-center gap-0.5 text-[10px] text-destructive">
              <XCircle className="w-3 h-3" />
              {t("onboarding.status.denied")}
            </span>
          ) : (
            <span className="inline-flex items-center gap-0.5 text-[10px] text-amber-600">
              <AlertCircle className="w-3 h-3" />
              {t("onboarding.status.notDetermined")}
            </span>
          )}
        </div>
        <p className="text-xs text-muted-foreground mt-0.5">
          {t(`onboarding.permissions.${def.key}.description`)}
        </p>
      </div>
      {granted ? (
        <Button
          variant="ghost"
          size="sm"
          onClick={handleOpenSettings}
          className="shrink-0 text-muted-foreground"
        >
          {t("onboarding.actions.view")}
        </Button>
      ) : (
        <Button
          variant="outline"
          size="sm"
          onClick={denied ? handleOpenSettings : handleRequest}
          className="shrink-0"
        >
          {denied
            ? t("onboarding.actions.openSettings")
            : t("onboarding.actions.request")}
        </Button>
      )}
      {/* hover 详细说明（权限作用 + 使用场景 + 操作指引），参考豆包隐私页设计。
          升级提示（upgradeNote）不在此显示——已在设置页权限 tab 底部统一展示。 */}
      <div className="pointer-events-none absolute left-1/2 -translate-x-1/2 top-full mt-1 z-50
                      opacity-0 group-hover:opacity-100 transition-opacity
                      w-72 px-3 py-2.5 rounded-md bg-popover border border-border shadow-md space-y-1.5">
        <p className="text-xs text-popover-foreground whitespace-normal">
          {t(`onboarding.permissions.${def.key}.usage`)}
        </p>
        <div className="border-t border-border/50 pt-1.5">
          <p className="text-[11px] text-muted-foreground whitespace-normal">
            {granted
              ? t("onboarding.permissions.operationGranted")
              : denied
                ? t("onboarding.permissions.operationDenied")
                : t(`onboarding.permissions.${def.key}.operation`)}
          </p>
        </div>
      </div>
    </div>
  );
}
