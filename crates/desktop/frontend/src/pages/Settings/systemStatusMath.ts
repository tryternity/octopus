// 系统状态页的纯逻辑：字节格式化、sparkline 点映射、快照去重。
// 刻意抽出纯函数以便单测（仓库惯例：纯逻辑 + colocated *.test.ts）。

/** 人类可读字节：null/undefined → "?"，自动选 B/KB/MB/GB 单位。 */
export function fmtBytes(n: number | null | undefined): string {
  if (n == null) return "?";
  if (n < 1024) return n + " B";
  if (n < 1048576) return (n / 1024).toFixed(1) + " KB";
  if (n < 1073741824) return (n / 1048576).toFixed(1) + " MB";
  return (n / 1073741824).toFixed(2) + " GB";
}

/** sparkline 点串：把数值序列映射进 viewBox(0..w, 0..h)。data.length<2 返回空串（调用方显示「采集中」）。 */
export function sparklinePoints(
  data: number[],
  opts?: { w?: number; h?: number; max?: number },
): string {
  if (data.length < 2) return "";
  const w = opts?.w ?? 100;
  const h = opts?.h ?? 32;
  const hi = opts?.max ?? Math.max(...data, 1);
  const lo = Math.min(...data, 0);
  const span = Math.max(hi - lo, 1);
  return data
    .map((v, i) => {
      const x = (i / (data.length - 1)) * w;
      const y = h - ((v - lo) / span) * h;
      return `${x.toFixed(1)},${y.toFixed(1)}`;
    })
    .join(" ");
}

/** 快照去重：仅当 next.sampled_at 严格大于 prev 才替换；prev 为 null 或更旧时取 next。 */
export function newerSnapshot<T extends { sampled_at: number }>(prev: T | null, next: T): T {
  if (!prev) return next;
  return next.sampled_at > prev.sampled_at ? next : prev;
}
