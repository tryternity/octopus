import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { MemoryStick, Cpu, Boxes, type LucideIcon } from "lucide-react";
import {
  fmtBytes,
  sparklinePoints,
  newerSnapshot,
  sparklineDataFromNullable,
} from "./systemStatusMath";

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
  const pts = sparklinePoints(data, { max });
  if (!pts) return <div className="h-8 text-[10px] text-muted-foreground/50">采集中…</div>;
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

function Card({
  icon: Icon,
  title,
  children,
}: {
  icon: LucideIcon;
  title: string;
  children: React.ReactNode;
}) {
  return (
    <div className="border border-border rounded-lg overflow-hidden bg-background">
      <div className="flex items-center gap-2 px-4 py-2.5 bg-muted/40 border-b border-border">
        <Icon className="w-4 h-4 text-muted-foreground" />
        <h3 className="text-sm font-semibold">{title}</h3>
      </div>
      <div className="px-4 py-3">{children}</div>
    </div>
  );
}

export default function SystemPanel({ showToast }: { showToast: (msg: string) => void }) {
  const [snap, setSnap] = useState<SystemStatusSnapshot | null>(null);

  useEffect(() => {
    invoke<SystemStatusSnapshot>("get_system_status")
      .then(setSnap)
      .catch((e) => showToast("加载状态失败：" + e));
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
      <div className="flex items-center justify-center h-full text-muted-foreground">加载中...</div>
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
              <span>进程内存 {fmtBytes(snap.process.real_bytes)}</span>
              <span className="text-muted-foreground">
                RSS {fmtBytes(snap.process.rss_bytes)}
              </span>
            </>
          ) : (
            <span>进程总内存 {fmtBytes(snap.process.rss_bytes)}</span>
          )}
        </span>
        <span className="text-xs text-muted-foreground/70">
          系统 CPU {snap.system.cpu_percent.toFixed(1)}%
        </span>
      </div>

      {/* 内存 / CPU 并排（布局 B） */}
      <div className="grid grid-cols-2 gap-3">
        <Card icon={MemoryStick} title={hasReal ? "内存（实际占用）" : "内存（进程 RSS）"}>
          <div className="text-lg font-semibold mb-1">
            {fmtBytes(memMain)}
            {hasReal && (
              <span className="ml-2 text-xs text-muted-foreground font-normal">
                RSS {fmtBytes(snap.process.rss_bytes)}
              </span>
            )}
          </div>
          <Sparkline data={realSeries} color="#6ab0f3" max={realMax} />
        </Card>
        <Card icon={Cpu} title="CPU（进程）">
          <div className="text-lg font-semibold mb-1">
            {snap.process.cpu_percent.toFixed(1)}%
          </div>
          <Sparkline data={snap.history.cpu} color="#f3a96a" />
        </Card>
      </div>

      {/* 模型列表 */}
      <Card icon={Boxes} title="模型（估算）">
        {snap.models.length === 0 ? (
          <div className="text-xs text-muted-foreground/60">暂无已加载模型</div>
        ) : (
          <div className="flex flex-col gap-1.5">
            {snap.models.map((m) => (
              <div key={m.id} className="flex items-center justify-between text-sm">
                <div className="flex items-center gap-1.5">
                  <span className="text-[10px] text-muted-foreground/60 px-1.5 py-0.5 rounded bg-muted">
                    {m.kind}
                  </span>
                  <span>{m.display_name}</span>
                </div>
                <span className="text-xs text-muted-foreground/70">
                  约 {fmtBytes(m.estimated_bytes)}
                </span>
              </div>
            ))}
          </div>
        )}
        <div className="mt-2 text-[10px] text-muted-foreground/50">
          模型内存为「加载前后进程内存差值」估算（macOS 用 phys_footprint、其他平台用 RSS；同进程 ort 无法精确拆分），仅供参考。
        </div>
      </Card>
    </div>
  );
}
