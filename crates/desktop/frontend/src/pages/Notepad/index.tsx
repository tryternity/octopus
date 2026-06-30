import { useState } from "react";
import NoteList from "./NoteList";
import NoteEditor from "./NoteEditor";

export default function Notepad() {
  const [selectedId, setSelectedId] = useState<number | null>(null);
  return (
    <div className="flex h-screen w-screen overflow-hidden bg-background text-foreground">
      <div className="w-64 flex-shrink-0">
        <NoteList selectedId={selectedId} onSelect={setSelectedId} />
      </div>
      <NoteEditor noteId={selectedId} />
    </div>
  );
}
