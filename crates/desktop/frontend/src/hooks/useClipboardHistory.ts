import { useState, useEffect, useRef, useCallback } from "react";
import { invoke } from "@/lib/tauri";
import { useTauriEvent } from "@/hooks/useTauriEvent";
import type { ClipboardItem } from "@/types/clipboard";

export function useClipboardHistory(filter: string, search: string) {
  const [items, setItems] = useState<ClipboardItem[]>([]);
  const [total, setTotal] = useState(0);

  const debouncedSearch = useDebouncedValue(search, 300);

  // 竞态哨兵：filter 切换 / 搜索防抖落地 / clipboard://changed 事件三路都会
  // 触发 fetchItems，慢请求晚到会覆盖最新状态（清空搜索后残留旧结果，
  // 或 items 与 total 跨请求交叉不一致）。每次发起递增 reqId，两次 await
  // 回来后比对——已有更新的请求在途即废弃本次陈旧结果。
  const reqIdRef = useRef(0);

  // useCallback 稳定引用：作为 onChanged 透传给 memo 化的行组件，selectedId 变化
  // （父重绘）时 refresh 不换新引用 → 行 props 浅比较不变 → 不重绘。依赖 filter/
  // debouncedSearch，二者变化时引用才变（届时 items 也换、行本就要重绘）。
  const fetchItems = useCallback(async () => {
    // filter="queue" 时使用 QueueListView（peek_paste_stack），此处不查 history。
    // 后端 build_where 把未知 filter 退化为 "all"，会返回全部历史条目，
    // 污染 selectedIndex→previewItem effect（误把 history[0] 设为预览项）。
    if (filter === "queue") {
      setItems([]);
      setTotal(0);
      return;
    }
    const myId = ++reqIdRef.current;
    try {
      const result = await invoke<ClipboardItem[]>("query_clipboard_history", {
        filter,
        search: debouncedSearch || null,
        page: 1,
        size: 200,
      });
      if (myId !== reqIdRef.current) return;
      setItems(result);
      const count = await invoke<number>("clipboard_stats", { filter, search: debouncedSearch || null });
      if (myId !== reqIdRef.current) return;
      setTotal(count);
    } catch (e) {
      console.error("Failed to fetch clipboard history:", e);
    }
  }, [filter, debouncedSearch]);

  useEffect(() => {
    fetchItems();
  }, [fetchItems]);

  useTauriEvent("clipboard://changed", () => {
    fetchItems();
  });

  return { items, total, refresh: fetchItems };
}

export function useDebouncedValue<T>(value: T, delay: number): T {
  const [debounced, setDebounced] = useState(value);
  useEffect(() => {
    const timer = setTimeout(() => setDebounced(value), delay);
    return () => clearTimeout(timer);
  }, [value, delay]);
  return debounced;
}
