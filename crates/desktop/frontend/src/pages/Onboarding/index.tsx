/**
 * Onboarding —— 首次启动权限引导页。
 *
 * 3 个权限卡片（麦克风 / 辅助功能 / 屏幕录制），各显示状态 + 申请/打开系统设置按钮。
 * 底部「完成」按钮调 complete_onboarding 命令（写 DB flag + 关窗）。
 * 允许跳过（不强制全 granted）。
 *
 * PermissionCard 组件抽到 @/components/PermissionCard（Onboarding + Settings tab 共用）。
 */
import { useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Button } from "@/components/ui/button";
import { PermissionCard, PERMISSIONS } from "@/components/PermissionCard";
import { useT } from "@/lib/i18n";

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
