import { useState, useEffect, useCallback } from "react";
import { listNotes, countNotes } from "@/lib/notepad";
import { useTauriEvent } from "@/hooks/useTauriEvent";
import { useDebouncedValue } from "@/hooks/useClipboardHistory";
import type { Note, NoteType } from "@/types/note";

const PAGE_SIZE = 30;

export function useNotes(noteType: NoteType | null, search: string, favoriteOnly: boolean) {
  const [items, setItems] = useState<Note[]>([]);
  const [total, setTotal] = useState(0);
  const [offset, setOffset] = useState(0); // 加载更多累计 offset
  const debouncedSearch = useDebouncedValue(search, 300);

  const fetchFirst = useCallback(async () => {
    const [rows, count] = await Promise.all([
      listNotes({ noteType, search: debouncedSearch || null, favorite: favoriteOnly, limit: PAGE_SIZE, offset: 0 }),
      countNotes({ noteType, search: debouncedSearch || null, favorite: favoriteOnly }),
    ]);
    setItems(rows);
    setTotal(count);
    setOffset(PAGE_SIZE);
  }, [noteType, debouncedSearch, favoriteOnly]);

  useEffect(() => {
    fetchFirst().catch(console.error);
  }, [fetchFirst]);

  // notepad://changed → 刷新（保存/编辑/删除后后端 emit）
  useTauriEvent("notepad://changed", () => {
    fetchFirst().catch(console.error);
  });

  const loadMore = useCallback(async () => {
    const rows = await listNotes({ noteType, search: debouncedSearch || null, favorite: favoriteOnly, limit: PAGE_SIZE, offset });
    setItems((prev) => [...prev, ...rows]);
    setOffset((o) => o + PAGE_SIZE);
  }, [noteType, debouncedSearch, favoriteOnly, offset]);

  return { items, total, refresh: fetchFirst, loadMore, hasMore: items.length < total };
}
