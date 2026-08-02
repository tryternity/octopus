// hotwords.ts — 热词多命中候选段的前端纯逻辑。
//
// 后端 segments JSON（transcript.segments_json()）形如：
//   [{"kind":"raw","text":"需要你修正这个"},
//    {"kind":"hotwords","text":"注释","candidates":["注释","主意","注意"]},
//    {"kind":"raw","text":"修复下面的错误。"}]
//
// 段顺序拼接 == CM6 doc 文本；段边界 = 累积 char offset（注意是 UTF-16 code unit，
// 与 CM6 / JS string 一致，非 Unicode 码点—— astral 平面外字符 JS 已按 1 计）。
// 本文件提供：
//   - Segment / SegmentKind 类型
//   - parseSegments：JSON 字符串 → Segment[]（容错：坏数据返回 null）
//   - segmentsMatchText：段拼接 == doc 文本（失配则前端降级不渲染装饰，避免错位）
//   - hotwordRanges：遍历段产 hotwords 段的 [from, to, candidates]
//   - applyCandidate：选中候选 → 新 doc + dirtyRange（纯函数，供测试）

/** 段类型（对齐后端 SegmentKind：raw/polished/edited/hotwords）。 */
export type SegmentKind = "raw" | "polished" | "edited" | "hotwords";

/** 段（对齐后端 Segment serde JSON）。candidates 仅 hotwords 段含。 */
export interface Segment {
  kind: SegmentKind;
  text: string;
  /** hotwords 段的候选列表（得分降序，最多 5 个；text 是第一个 = 默认选择）。 */
  candidates?: string[];
}

/** hotwords 段在 doc 里的定位 + 候选（驱动 Decoration.mark + 浮层下拉）。 */
export interface HotwordRange {
  /** 段起始 char offset（含）。 */
  from: number;
  /** 段结束 char offset（不含）。 */
  to: number;
  /** 候选列表（text 是第一个）。 */
  candidates: string[];
  /** 该段在 segments 数组中的 index（稳定标识，区分多个相同 word 的 hotword 段）。 */
  segIndex: number;
}

/**
 * 解析后端 segments JSON 字符串。坏数据 / 非 hotwords 段 → 返回 null（前端降级为无装饰）。
 * 空字符串 / "[]" → null（无段信息，等同旧扁平 text 行为）。
 */
export function parseSegments(json: string | null | undefined): Segment[] | null {
  if (!json) return null;
  let arr: unknown;
  try {
    arr = JSON.parse(json);
  } catch {
    return null;
  }
  if (!Array.isArray(arr)) return null;
  const segments: Segment[] = [];
  for (const item of arr) {
    if (!item || typeof item !== "object") continue;
    const obj = item as Record<string, unknown>;
    const kind = obj.kind;
    const text = obj.text;
    if (typeof kind !== "string" || typeof text !== "string") continue;
    if (!["raw", "polished", "edited", "hotwords"].includes(kind)) continue;
    const seg: Segment = { kind: kind as SegmentKind, text };
    if (Array.isArray(obj.candidates) && obj.candidates.every((c) => typeof c === "string")) {
      seg.candidates = obj.candidates as string[];
    }
    segments.push(seg);
  }
  return segments.length > 0 ? segments : null;
}

/**
 * 段顺序拼接是否等于 doc 文本。
 * 失配（用户编辑后 / 流式追加未同步 / 后端 rebuild 差异）→ 返回 false → 前端降级不渲染装饰。
 * 这是 hotwords offset 正确性的硬约束：offset 错位会让下划线标到错位置。
 */
export function segmentsMatchText(segments: Segment[], doc: string): boolean {
  let acc = "";
  for (const seg of segments) {
    acc += seg.text;
  }
  return acc === doc;
}

/**
 * 遍历 segments 产 hotwords 段的 [from, to, candidates]。
 * 跳过无 candidates 或 candidates 为空的 hotwords 段（异常容错）。
 * 若 segments 拼接与 doc 失配，返回空数组（调用方应先 segmentsMatchText 校验）。
 */
export function hotwordRanges(segments: Segment[], doc: string): HotwordRange[] {
  if (!segmentsMatchText(segments, doc)) return [];
  const ranges: HotwordRange[] = [];
  let offset = 0;
  for (let i = 0; i < segments.length; i++) {
    const seg = segments[i];
    const len = seg.text.length;
    if (seg.kind === "hotwords" && seg.candidates && seg.candidates.length > 0) {
      ranges.push({ from: offset, to: offset + len, candidates: seg.candidates, segIndex: i });
    }
    offset += len;
  }
  return ranges;
}

/**
 * 选中候选 → 替换 doc 中 [from, to) 为 candidate，返回新 doc + dirtyRange。
 * 纯函数：不触碰 CM6 state，供测试 + AsrEditor 调用 view.dispatch。
 * dirtyRange 的 to 是替换后的新结束 offset（from + candidate.length）。
 */
export function applyCandidate(
  doc: string,
  from: number,
  to: number,
  candidate: string,
): { doc: string; dirtyRange: [number, number] } {
  // clamp 防越界（hotwordRanges 的 from/to 来自段拼接，理论上必在 doc 内，但防御）
  const f = Math.max(0, Math.min(from, doc.length));
  const t = Math.max(f, Math.min(to, doc.length));
  const newDoc = doc.slice(0, f) + candidate + doc.slice(t);
  return { doc: newDoc, dirtyRange: [f, f + candidate.length] };
}
