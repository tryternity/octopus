import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { AlertTriangle, Copy } from "lucide-react";
import { useT } from "@/lib/i18n";

/**
 * HealthReport —— 显示 vault 健康摘要。
 *
 * 数据来自后端 `vault_health_report`：
 *   weakCount + weakCipherIds       弱密码（strength score < 3）
 *   duplicate_groups                   重复密码组（password SHA-256 分组）
 *   totalLogins                       登录总数
 *   averageScore                      平均强度（0..4）
 *
 * 由于弱密码列表 / 重复组只暴露 cipher id（避免明文密码暴露给 UI 内存），
 * 这里只显示数量摘要——详细修复走 CipherList 检索。
 */

interface DuplicateGroup {
  password_hash: string;
  cipherIds: number[];
}
interface HealthReportDto {
  weakCount: number;
  weakCipherIds: number[];
  duplicate_groups: DuplicateGroup[];
  totalLogins: number;
  averageScore: number;
  // R-AVG-DENOM：averageScore 的真实分母（仅 password=Some 的 Login）。
  // 后端总会返回；optional 保持向后兼容（旧后端不发此字段时 undefined）。
  scoredCount?: number;
}

export default function HealthReport({ showToast }: { showToast: (msg: string) => void }) {
  const t = useT();
  const [report, setReport] = useState<HealthReportDto | null>(null);

  const refresh = useCallback(async () => {
    try {
      const r = await invoke<HealthReportDto>("vault_health_report");
      setReport(r);
    } catch (e) {
      showToast(String(e));
    }
  }, [showToast]);

  useEffect(() => {
    refresh();
  }, [refresh]);

  if (!report) {
    return (
      <div className="flex h-full items-center justify-center text-sm text-muted-foreground">
        {t("settings.loading")}
      </div>
    );
  }

  // 0..4 → 取整显示。score 越高越强。
  const score = Math.round(report.averageScore);
  const duplicateCount = report.duplicate_groups.length;
  const totalDupCiphers = report.duplicate_groups.reduce((sum, g) => sum + g.cipherIds.length, 0);

  return (
    <div className="space-y-3">
      {/* 摘要卡片 */}
      <div className="grid grid-cols-3 gap-3">
        <div className="rounded-lg border border-border/50 bg-muted/15 p-3">
          <p className="text-[11px] uppercase tracking-wide text-muted-foreground/70">
            {t("settings.vault.health.total", { count: report.totalLogins })}
          </p>
          <p className="mt-1 text-2xl font-semibold tabular-nums">{report.totalLogins}</p>
        </div>
        <div className="rounded-lg border border-amber-500/30 bg-amber-500/5 p-3">
          <p className="text-[11px] uppercase tracking-wide text-muted-foreground/70">
            {t("settings.vault.health.weak")}
          </p>
          <p className="mt-1 flex items-center gap-1.5 text-2xl font-semibold tabular-nums text-amber-600 dark:text-amber-400">
            <AlertTriangle className="size-4" />
            {report.weakCount}
          </p>
        </div>
        <div className="rounded-lg border border-border/50 bg-muted/15 p-3">
          <p className="text-[11px] uppercase tracking-wide text-muted-foreground/70">
            {t("settings.vault.health.duplicates")}
          </p>
          <p className="mt-1 flex items-center gap-1.5 text-2xl font-semibold tabular-nums">
            <Copy className="size-4" />
            {duplicateCount}
          </p>
        </div>
      </div>

      <p className="text-sm text-muted-foreground">
        {/* R-AVG-DENOM：当有 password=None 的 Login 时标明 averageScore 的真实分母，
            避免「N 个登录平均分 X」的误导（实际只算有密码项） */}
        {t("settings.vault.health.averageScore", { score })}
        {report.scoredCount !== undefined &&
          report.scoredCount < report.totalLogins && (
            <span className="ml-1 text-xs text-muted-foreground/60">
              ({t("settings.vault.health.scoredOf", { scored: report.scoredCount })})
            </span>
          )}
      </p>

      {/* 详细信息（折叠摘要） */}
      <div className="space-y-2 rounded-lg border border-border/50 bg-muted/15 p-4 text-sm">
        <div className="flex items-center justify-between">
          <span className="text-muted-foreground">{t("settings.vault.health.weakCount", { count: report.weakCount })}</span>
        </div>
        <div className="flex items-center justify-between">
          <span className="text-muted-foreground">
            {t("settings.vault.health.duplicatesCount", { count: duplicateCount })}
          </span>
          {totalDupCiphers > 0 && (
            <span className="text-xs text-muted-foreground/60">({totalDupCiphers} ciphers)</span>
          )}
        </div>
        {report.weakCipherIds.length > 0 && (
          <details className="pt-1">
            <summary className="cursor-pointer text-xs text-muted-foreground/70">
              weak cipher ids
            </summary>
            <p className="mt-1 font-mono text-[10px] text-muted-foreground/60">
              {report.weakCipherIds.join(", ")}
            </p>
          </details>
        )}
        {report.duplicate_groups.length > 0 && (
          <details className="pt-1">
            <summary className="cursor-pointer text-xs text-muted-foreground/70">
              duplicate groups
            </summary>
            <ul className="mt-1 space-y-0.5 font-mono text-[10px] text-muted-foreground/60">
              {report.duplicate_groups.map((g, i) => (
                <li key={i}>
                  [{i + 1}] ids={g.cipherIds.join(",")}
                </li>
              ))}
            </ul>
          </details>
        )}
      </div>
    </div>
  );
}
