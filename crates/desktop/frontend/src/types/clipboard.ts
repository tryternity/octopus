export type ItemType = "text" | "image" | "file";
export type Source = "clipboard" | "asr" | "ocr";

export interface ImageMeta {
  blob_hash: string;
  width: number;
  height: number;
  has_thumbnail: boolean;
  size: number;
}

export interface FileMeta {
  file_count: number;
  paths: string[];
}

export interface AsrMeta {
  transcription_id: number;
  polish_status: string;
  engine: string;
  model: string;
}

export interface OcrMeta {
  engine: string;
  model: string;
}

export interface ClipboardItem {
  id: number;
  item_type: ItemType;
  source: Source;
  content: string;
  is_favorite: boolean;
  created_at: string;
  image_meta?: ImageMeta;
  file_meta?: FileMeta;
  asr_meta?: AsrMeta;
  ocr_meta?: OcrMeta;
  is_rich: boolean;
}
