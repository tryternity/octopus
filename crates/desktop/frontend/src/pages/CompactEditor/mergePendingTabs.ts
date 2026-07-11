/// 把 pending tabs 合并进 existing tabs（纯函数，便于单测）。
///
/// 背景：CompactEditor 首次开窗时，URL 注入首个 tab 占位（text=""，避免超长 URL），
/// mount 后 get_pending_compact_tabs 返回含真实 text 的 pending。两者同 key。
/// pending 是完整数据，existing 占位缺 text → 同 key 时 pending 须覆盖占位。
export function mergePendingTabs<T extends { key: string }>(existing: T[], pending: T[]): T[] {
  const pendingByKey = new Map(pending.map((p) => [p.key, p]));
  const existingKeys = new Set(existing.map((t) => t.key));
  // 同 key：pending 是完整数据（URL 占位缺 text）→ 覆盖 existing 占位；
  // 旧逻辑 `continue` 跳过 pending，占位 text="" 永久保留 → 首个文本 tab 空白。
  const replaced = existing.map((tab) => pendingByKey.get(tab.key) ?? tab);
  // 新 key：追加
  const added = pending.filter((p) => !existingKeys.has(p.key));
  return [...replaced, ...added];
}
