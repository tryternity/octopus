/// 把 pending tabs 合并进 existing tabs（纯函数，便于单测）。
///
/// 背景：CompactEditor 首次开窗时，URL 注入首个 tab 占位（text=""，避免超长 URL），
/// mount 后 get_pending_compact_tabs 返回含真实 text 的 pending。两者同 key。
/// pending 是完整数据，existing 占位缺 text → 同 key 时 pending 须覆盖占位。
///
/// 图片 tab 上限（P2 2026-08-18）：emit 路径经 loadAndAddTab 逐事件强制，但 pending
/// 批量路径（关窗状态拖 N 张图 → mount 一次合并）绕过检查——此处按同语义补强制
///（超限挤掉最旧图片 tab，文本不受影响）。常量由此导出，index.tsx 复用。
export const MAX_IMAGE_TABS = 5;

export function mergePendingTabs<T extends { key: string; itemType?: string }>(
  existing: T[],
  pending: T[],
): T[] {
  const pendingByKey = new Map(pending.map((p) => [p.key, p]));
  const existingKeys = new Set(existing.map((t) => t.key));
  // 同 key：pending 是完整数据（URL 占位缺 text）→ 覆盖 existing 占位；
  // 旧逻辑 `continue` 跳过 pending，占位 text="" 永久保留 → 首个文本 tab 空白。
  const replaced = existing.map((tab) => pendingByKey.get(tab.key) ?? tab);
  // 新 key：追加
  const added = pending.filter((p) => !existingKeys.has(p.key));
  const merged = [...replaced, ...added];

  // 图片 tab ≤ MAX_IMAGE_TABS：挤掉最旧的（与 loadAndAddTab 的逐事件语义一致）
  const imageIdx = merged.map((t, i) => (t.itemType === "image" ? i : -1)).filter((i) => i >= 0);
  if (imageIdx.length > MAX_IMAGE_TABS) {
    const drop = new Set(imageIdx.slice(0, imageIdx.length - MAX_IMAGE_TABS));
    return merged.filter((_, i) => !drop.has(i));
  }
  return merged;
}
