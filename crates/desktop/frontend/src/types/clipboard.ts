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

/**
 * 文件条目专用（不含时间）：
 * 单个 → 类型（如 png/txt）；多个 → 「N个 · 首个类型」。
 * type 缺失时退化为「N个」/ 空。
 */
export function fileMeta(item: ClipboardItem): string {
  const files = item.meta_info?.files;
  if (!files || files.length === 0) return "";
  const firstType = files.map((f) => f.type).find(Boolean);
  if (files.length === 1) return firstType || "";
  return firstType ? `${files.length}个 · ${firstType}` : `${files.length}个`;
}

/** 类型强调色 */
export const typeAccent: Record<ItemType, string> = {
  text: "text-stone-500",
  voice: "text-amber-600",
  ocr: "text-teal-600",
  image: "text-indigo-500",
  file: "text-emerald-600",
};

// ===== 无协议链接识别 =====

/**
 * 常用域名后缀（无协议链接识别用）。
 * 域名（小写）以其中任一后缀结尾、且后缀前至少还有一个 label，即判为公网链接（补 https://）。
 * 后缀自带前导「.」，dot 对齐，避免子串误命中（如 foocom ≠ .com）。
 * 追加新后缀直接加分号项即可，例如 ".dev" / ".io" / ".gov.cn"。
 */
export const COMMON_DOMAIN_SUFFIXES = ".com;.cn;.com.cn;.net;.org";
const COMMON_SUFFIX_LIST = COMMON_DOMAIN_SUFFIXES.split(";").filter(Boolean);

export interface DetectUrlResult {
  isLink: boolean;
  /** 打开用的完整 URL；无协议时已按规则补全 http(s):// */
  url: string;
}

/** 端口合法：1–65535 的数字。 */
function isPort(p: string): boolean {
  return /^\d{1,5}$/.test(p) && Number(p) >= 1 && Number(p) <= 65535;
}

/** 合法 IPv4：4 段点分，每段 0–255。 */
function isIPv4(h: string): boolean {
  const parts = h.split(".");
  if (parts.length !== 4) return false;
  return parts.every((s) => /^\d{1,3}$/.test(s) && Number(s) <= 255);
}

/** 域名 labels：≥2 段，每段 [A-Za-z0-9-]+ 且不以 - 开头/结尾。 */
function isDomainLabels(d: string): boolean {
  const parts = d.split(".");
  if (parts.length < 2) return false;
  return parts.every((s) => /^[A-Za-z0-9-]+$/.test(s) && !s.startsWith("-") && !s.endsWith("-"));
}

/**
 * 识别剪贴板文本是否为链接。
 * - 带协议（http(s)://）→ 原样
 * - localhost/IPv4 + 必带端口 → 补 http://
 * - 常用后缀域名 + 可选路径/端口 → 补 https://
 * - 句中片段（含空白）、纯 IP/localhost（无端口）、非常见后缀 → 非链接
 */
export function detectUrl(raw: string): DetectUrlResult {
  const s = raw.trim();
  if (!s) return { isLink: false, url: "" };
  if (/^https?:\/\//i.test(s)) return { isLink: true, url: s };
  if (/\s/.test(s)) return { isLink: false, url: "" };

  const hostSeg = s.split(/[/?#]/)[0];

  // 路径 B：localhost / IPv4 + 必带 :port → http://
  const portMatch = hostSeg.match(/:([^:/?#]+)$/);
  if (portMatch) {
    const port = portMatch[1];
    const hostname = hostSeg.slice(0, -portMatch[0].length); // 去掉 ":port"
    if (isPort(port) && (hostname.toLowerCase() === "localhost" || isIPv4(hostname))) {
      return { isLink: true, url: "http://" + s };
    }
  }

  // 路径 A：常用后缀域名 → https://
  const domainPart = hostSeg.split(":")[0];
  if (isDomainLabels(domainPart)) {
    const lower = domainPart.toLowerCase();
    if (COMMON_SUFFIX_LIST.some((suf) => lower.endsWith(suf) && lower.length > suf.length)) {
      return { isLink: true, url: "https://" + s };
    }
  }

  return { isLink: false, url: "" };
}

