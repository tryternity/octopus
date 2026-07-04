export type ItemType = "text" | "voice" | "ocr" | "image" | "file";

export interface MetaInfo {
  // image
  w?: number;
  h?: number;
  size?: string;
  // voice / ocr
  engine?: string;
  model?: string;
  duration_ms?: number;
  char_count?: number;
  engine_mode?: string;
  polish_model?: string;
  polished?: boolean;
  // file
  files?: Array<{ size?: string; type?: string }>;
}

export interface ClipboardItem {
  id: number;
  item_type: ItemType;
  content: string;
  ref_data?: string;
  meta_info?: MetaInfo;
  is_favorite: boolean;
  created_at: string;
  is_rich: boolean;
  has_thumbnail: boolean;
  segments?: string;
}
