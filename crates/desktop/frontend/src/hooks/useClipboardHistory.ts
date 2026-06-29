import { useState, useEffect } from "react";
import { invoke } from "@/lib/tauri";
import { useTauriEvent } from "@/hooks/useTauriEvent";
import type { ClipboardItem } from "@/types/clipboard";

export function useClipboardHistory(filter: string, search: string) {
  const [items, setItems] = useState<ClipboardItem[]>([]);
  const [total, setTotal] = useState(0);

  const debouncedSearch = useDebouncedValue(search, 300);

  const fetchItems = async () => {
    try {
      const result = await invoke<ClipboardItem[]>("query_clipboard_history", {
        filter,
        search: debouncedSearch || null,
        page: 1,
        size: 50,
      });
      setItems(result);
      const count = await invoke<number>("clipboard_stats", { filter, search: debouncedSearch || null });
      setTotal(count);
    } catch (e) {
      console.error("Failed to fetch clipboard history:", e);
    }
  };

  useEffect(() => {
    fetchItems();
  }, [filter, debouncedSearch]);

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
