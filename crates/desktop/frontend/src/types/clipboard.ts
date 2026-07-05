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
  asr_mode?: string;
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

/**
 * 按类型生成元数据片段（不含时间）：
 * text/ocr: "N字"
 * voice:    "N字 · Xs"
 * image:    ""（WxH·size 由组件放在缩略图旁）
 * file:     ""
 */
export function metaParts(item: ClipboardItem): string {
  const m = item.meta_info;
  switch (item.item_type) {
    case "text":
    case "ocr":
      return m?.char_count ? `${m.char_count}字` : "";
    case "voice": {
      const parts: string[] = [];
      if (m?.char_count) parts.push(`${m.char_count}字`);
      if (m?.duration_ms) parts.push(`${(m.duration_ms / 1000).toFixed(1)}s`);
      return parts.join(" · ");
    }
    default:
      return "";
  }
}

/** 图片条目专用：WxH · size */
export function imageMeta(item: ClipboardItem): string {
  const m = item.meta_info;
  if (!m) return "";
  const parts: string[] = [];
  if (m.w && m.h) parts.push(`${m.w}×${m.h}`);
  if (m.size) parts.push(m.size);
  return parts.join(" · ");
}

/** 类型强调色 */
export const typeAccent: Record<ItemType, string> = {
  text: "text-stone-500",
  voice: "text-amber-600",
  ocr: "text-teal-600",
  image: "text-indigo-500",
  file: "text-emerald-600",
};

