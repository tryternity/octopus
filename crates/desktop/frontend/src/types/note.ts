export type NoteSource = "asr" | "ocr" | "clipboard" | "manual";

export interface Note {
  id: number;
  title: string | null;
  content_html: string;
  content_text: string;
  source: NoteSource;
  source_ref_id: number | null;
  is_pinned: boolean;
  is_favorite: boolean;
  created_at: string;
  updated_at: string;
}

export interface NoteListParams {
  source?: NoteSource | null;
  favorite?: boolean;
  pinned?: boolean;
  search?: string | null;
  limit?: number;
  offset?: number;
}
