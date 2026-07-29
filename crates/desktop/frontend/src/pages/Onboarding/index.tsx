/**
 * Onboarding —— 首次启动权限引导页。
 *
 * 3 个权限卡片（麦克风 / 辅助功能 / 屏幕录制），各显示状态 + 申请/打开系统设置按钮。
 * 底部「完成」按钮调 complete_onboarding 命令（写 DB flag + 关窗）。
 * 允许跳过（不强制全 granted）。
 *
 * 权限状态机（PermissionStatus）：
 * - granted：绿勾，无按钮
 * - denied：红色，[打开系统设置] 按钮
 * - not-determined：琥珀，[申请权限] 按钮（仅屏幕录制有此态；麦克风/AX 首次=denied）
 */
import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Mic, Accessibility, Monitor, CheckCircle2, XCircle, AlertCircle } from "lucide-react";
import { Button } from "@/components/ui/button";
import { useT } from "@/lib/i18n";
import { cn } from "@/lib/utils";

type PermissionStatus = "granted" | "denied" | "not-determined";

/** 权限类型 → 对应的 check/request 命令名 + i18n key + 图标 + PrivacySection。 */
interface PermissionDef {
  key: "microphone" | "accessibility" | "screen";
  icon: React.ComponentType<{ className?: string }>;
  checkCmd: string;
  requestCmd: string;
  /** open_privacy_settings 的 section 参数（PrivacySection lowercase variant）。 */
  privacySection: string;
}

const PERMISSIONS: PermissionDef[] = [
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
    privacySection: "screen_capture",
  },
];

/** 单个权限卡片——parametrize by PermissionDef，复用 PermissionGate 的 refresh/request 模式。 */
function PermissionCard({ def }: { def: PermissionDef }) {
  const t = useT();
  const [status, setStatus] = useState<PermissionStatus | null>(null);
  const Icon = def.icon;

  const refresh = useCallback(async () => {
    try {
      const s = await invoke<PermissionStatus>(def.checkCmd);
      setStatus(s);
    } catch {
      setStatus("not-determined");
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
      setStatus("not-determined");
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
        "flex items-center gap-3 px-4 py-3 rounded-lg border transition-colors",
        granted
          ? "border-emerald-600/30 bg-emerald-600/5"
          : denied
            ? "border-destructive/40 bg-destructive/5"
            : "border-amber-600/40 bg-amber-600/5",
      )}
    >
      <div className={cn("flex-shrink-0", granted ? "text-emerald-600" : denied ? "text-destructive" : "text-amber-600")}>
        <Icon className="w-6 h-6" />
      </div>
      <div className="flex-1 min-w-0">
        <div className="flex items-center gap-2">
          <span className="text-sm font-medium text-foreground">
            {t(`onboarding.permissions.${def.key}.title`)}
          </span>
          {/* 状态徽章 */}
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
      {/* 按钮：granted 无按钮；denied → 打开系统设置；not-determined → 申请权限 */}
      {!granted && (
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
    </div>
  );
}

export default function Onboarding() {
  const t = useT();

  const handleComplete = useCallback(async () => {
    try {
      await invoke("complete_onboarding");
    } catch (e) {
      console.error("complete_onboarding failed", e);
    }
  }, []);

  return (
    <div className="flex flex-col h-screen w-screen overflow-hidden bg-background">
      {/* 标题区 */}
      <div className="px-8 pt-10 pb-6 text-center">
        <h1 className="text-2xl font-semibold text-foreground">
          {t("onboarding.title")}
        </h1>
        <p className="text-sm text-muted-foreground mt-2">
          {t("onboarding.subtitle")}
        </p>
      </div>

      {/* 权限卡片列表 */}
      <div className="flex-1 px-8 space-y-3 overflow-y-auto min-h-0">
        {PERMISSIONS.map((def) => (
          <PermissionCard key={def.key} def={def} />
        ))}
      </div>

      {/* 底部操作区 */}
      <div className="flex justify-end gap-2 px-8 py-6 border-t border-border">
        <Button variant="ghost" size="sm" onClick={handleComplete}>
          {t("onboarding.actions.skip")}
        </Button>
        <Button size="sm" onClick={handleComplete}>
          {t("onboarding.actions.complete")}
        </Button>
      </div>
    </div>
  );
}
