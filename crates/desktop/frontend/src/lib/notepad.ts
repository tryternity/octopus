import { invoke } from "@/lib/tauri";
import type { Note, NoteListParams, NoteSource, NoteType } from "@/types/note";

export async function listNotes(params: NoteListParams): Promise<Note[]> {
  return invoke<Note[]>("list_notes", {
    source: params.source ?? null,
    noteType: params.noteType ?? null,
    favorite: params.favorite ?? false,
    pinned: params.pinned ?? false,
    search: params.search ?? null,
    limit: params.limit ?? 50,
    offset: params.offset ?? 0,
  });
}

export async function countNotes(params: NoteListParams): Promise<number> {
  return invoke<number>("count_notes", {
    source: params.source ?? null,
    noteType: params.noteType ?? null,
    favorite: params.favorite ?? false,
    pinned: params.pinned ?? false,
    search: params.search ?? null,
  });
}

export const getNote = (id: number) => invoke<Note | null>("get_note", { id });

export const createNote = (source: NoteSource, sourceRefId: number | null, body: string, noteType: NoteType) =>
  invoke<number>("create_note", { source, sourceRefId, body, noteType });

export const updateNote = (id: number, title: string, body: string, noteType: NoteType) =>
  invoke<void>("update_note", { id, title, body, noteType });

export const deleteNotes = (ids: number[]) => invoke<number>("delete_notes", { ids });

export const toggleNotePinned = (id: number) => invoke<void>("toggle_note_pinned", { id });
export const toggleNoteFavorite = (id: number) => invoke<void>("toggle_note_favorite", { id });

export const exportNote = (stem: string, ext: string, content: string) =>
  invoke<string>("export_note", { stem, ext, content });

export const importNoteFromFile = (path: string) =>
  invoke<string>("import_note_from_file", { path });

// 集成入口
export const currentTranscriptionId = () => invoke<number | null>("current_transcription_id");
export const saveTranscriptionToNote = (transcriptionId: number, text: string) =>
  invoke<number>("save_transcription_to_note", { transcriptionId, text });
export const saveOcrToNote = (text: string) => invoke<number>("save_ocr_to_note", { text });
