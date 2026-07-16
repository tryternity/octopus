import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { MemoryStick, Cpu, Boxes } from "lucide-react";
import {
  fmtBytes,
  sparklinePoints,
  newerSnapshot,
  sparklineDataFromNullable,
} from "./systemStatusMath";
import { useT } from "@/lib/i18n";
import { Card, CardHeader, CardTitle } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";

/** SystemPanel 专用卡片：头部图标+标题，内容区用宽松 padding（py-3）放图表。 */
function StatCard({
  icon: Icon,
  title,
  children,
}: {
  icon: React.ElementType;
  title: string;
  children: React.ReactNode;
}) {
  return (
    <Card>
      <CardHeader>
        <Icon className="w-4 h-4 text-muted-foreground" />
        <CardTitle>{title}</CardTitle>
      </CardHeader>
      <div className="px-4 py-3">{children}</div>
    </Card>
  );
}

export interface ProcessStats {
  rss_bytes: number;
  real_bytes: number | null; // macOS=phys_footprint，其他平台=null
  cpu_percent: number;
}
export interface SystemStats {
  total_memory_bytes: number;
  used_memory_bytes: number;
  cpu_percent: number;
}
export interface TimeSeries {
  rss: number[];
  real: (number | null)[]; // 新增：macOS 全非 null，其他平台含 null
  cpu: number[];
  timestamps: number[];
}
export interface ModelMemory {
  id: string;
  kind: string;
  display_name: string;
  estimated_bytes: number | null;
}
export interface SystemStatusSnapshot {
  sampled_at: number;
  process: ProcessStats;
  system: SystemStats;
  history: TimeSeries;
  models: ModelMemory[];
}

function Sparkline({ data, color, max }: { data: number[]; color: string; max?: number }) {
  const t = useT();
  const pts = sparklinePoints(data, { max });
  if (!pts) return <div className="h-8 text-[10px] text-muted-foreground/50">{t("settings.system.collecting")}</div>;
  return (
    <svg viewBox="0 0 100 32" preserveAspectRatio="none" className="w-full h-8">
      <polyline
        points={pts}
        fill="none"
        stroke={color}
        strokeWidth="1.5"
        vectorEffect="non-scaling-stroke"
      />
    </svg>
  );
}

export default function SystemPanel({ showToast }: { showToast: (msg: string) => void }) {
  const t = useT();
  const [snap, setSnap] = useState<SystemStatusSnapshot | null>(null);

  useEffect(() => {
    invoke<SystemStatusSnapshot>("get_system_status")
      .then(setSnap)
      .catch((e) => showToast(t("settings.system.loadFailed") + e));
    let unlisten: UnlistenFn;
    let cancelled = false;
    listen<SystemStatusSnapshot>("system-status", (e) => {
      setSnap((prev) => newerSnapshot(prev, e.payload));
    }).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [showToast]);

  if (!snap) {
    return (
      <div className="flex items-center justify-center h-full text-muted-foreground">{t("settings.system.loading")}</div>
    );
  }

  // 双指标：macOS 主用 real（phys_footprint，更接近真实占用），其他平台退 RSS。
  const hasReal = snap.process.real_bytes != null;
  const memMain = hasReal ? snap.process.real_bytes! : snap.process.rss_bytes;
  const realSeries = sparklineDataFromNullable(snap.history.real, snap.history.rss);
  const realMax = Math.max(
    ...realSeries,
    snap.process.real_bytes ?? snap.process.rss_bytes,
    1,
  );

  return (
    <div className="max-w-[640px] space-y-3">
      {/* 顶部汇总 */}
      <div className="flex items-center justify-between px-4 py-2.5 rounded-lg bg-muted/40 border border-border">
        {/* 双指标：macOS 主显 real（phys_footprint，真实占用），RSS 作辅；其他平台只显 RSS。 */}
        <span className="text-sm font-medium flex items-center gap-3">
          {snap.process.real_bytes != null ? (
            <>
              <span>{t("settings.system.processMem")} {fmtBytes(snap.process.real_bytes)}</span>
              <span className="text-muted-foreground">
                {t("settings.system.resident")} {fmtBytes(snap.process.rss_bytes)}
              </span>
            </>
          ) : (
            <span>{t("settings.system.processTotalMem")} {fmtBytes(snap.process.rss_bytes)}</span>
          )}
        </span>
        <span className="text-xs text-muted-foreground/70">
          {t("settings.system.systemCpu")} {snap.system.cpu_percent.toFixed(1)}%
        </span>
      </div>

      {/* 内存 / CPU 并排（布局 B） */}
      <div className="grid grid-cols-2 gap-3">
        <StatCard icon={MemoryStick} title={hasReal ? t("settings.system.memActual") : t("settings.system.memResident")}>
          <div className="text-lg font-semibold mb-1">
            {fmtBytes(memMain)}
            {hasReal && (
              <span className="ml-2 text-xs text-muted-foreground font-normal">
                {t("settings.system.resident")} {fmtBytes(snap.process.rss_bytes)}
              </span>
            )}
          </div>
          <Sparkline data={realSeries} color="var(--color-info)" max={realMax} />
        </StatCard>
        <StatCard icon={Cpu} title={t("settings.system.cpuProcess")}>
          <div className="text-lg font-semibold mb-1">
            {snap.process.cpu_percent.toFixed(1)}%
          </div>
          <Sparkline data={snap.history.cpu} color="var(--color-warning)" />
        </StatCard>
      </div>

      {/* 模型列表 */}
      <StatCard icon={Boxes} title={t("settings.system.modelEstimate")}>
        {snap.models.length === 0 ? (
          <div className="text-xs text-muted-foreground/60">{t("settings.system.noModels")}</div>
        ) : (
          <div className="flex flex-col gap-1.5">
            {snap.models.map((m) => (
              <div key={m.id} className="flex items-center justify-between text-sm">
                <div className="flex items-center gap-1.5">
                  <Badge size="sm">{m.kind}</Badge>
                  <span>{m.display_name}</span>
                </div>
                <span className="text-xs text-muted-foreground/70">
                  {t("settings.system.approx")} {fmtBytes(m.estimated_bytes)}
                </span>
              </div>
            ))}
          </div>
        )}
        <div className="mt-2 text-[10px] text-muted-foreground/50">
          {t("settings.system.modelMemHint")}
        </div>
        <div className="text-[10px] text-muted-foreground/50">
          {t("settings.system.ocrIdleHint")}
        </div>
      </StatCard>
    </div>
  );
}
