export type NoteSource = "asr" | "ocr" | "clipboard" | "manual";

/** 笔记内容格式（后端 notes.type 列）。html=富文本 / text=纯文本 / markdown=md 源码。 */
export type NoteType = "html" | "text" | "markdown";

export interface Note {
  id: number;
  title: string | null;
  content_html: string;
  content_text: string;
  note_type: NoteType;
  source: NoteSource;
  source_ref_id: number | null;
  is_pinned: boolean;
  is_favorite: boolean;
  created_at: string;
  updated_at: string;
}

export interface NoteListParams {
  source?: NoteSource | null;
  noteType?: NoteType | null;
  favorite?: boolean;
  pinned?: boolean;
  search?: string | null;
  limit?: number;
  offset?: number;
}
