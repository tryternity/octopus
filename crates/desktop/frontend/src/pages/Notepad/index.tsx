import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import NoteList from "./NoteList";
import NoteEditor from "./NoteEditor";
import { invoke } from "@/lib/tauri";

export default function Notepad() {
  const [selectedId, setSelectedId] = useState<number | null>(null);

  // mount：取 PENDING（OCR 等场景「存笔记 + 打开并选中」推来的 noteId）
  // + 监听已开窗时的并发选中事件
  useEffect(() => {
    invoke<number | null>("get_pending_note").then((id) => {
      if (id != null) setSelectedId(id);
    });
    const unlisten = listen<number>("notepad://select-note", (e) => {
      setSelectedId(e.payload);
    });
    return () => { unlisten.then((f) => f()); };
  }, []);

  return (
    <div className="flex h-screen w-screen overflow-hidden bg-background text-foreground">
      <div className="w-64 flex-shrink-0">
        <NoteList selectedId={selectedId} onSelect={setSelectedId} />
      </div>
      <NoteEditor noteId={selectedId} />
    </div>
  );
}
