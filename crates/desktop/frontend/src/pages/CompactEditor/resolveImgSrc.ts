/**
 * 预览图片 src 解析（spec §5）：仅相对路径（非 http/data/asset/绝对 scheme）经 baseUrl join
 * 后交 convert 转换（生产 convert = convertFileSrc；测试注入 identity）。
 * md 源与保存零影响——本函数只用于渲染 DOM。
 */
export function resolveImgSrc(src: string, baseUrl: string | undefined, convert: (abs: string) => string): string {
  if (!baseUrl) return src;
  if (/^(https?:|data:|asset:|blob:|tci:)/i.test(src)) return src;
  if (src.startsWith("/")) return src; // 绝对路径不经 join（站点语义，保留）
  const joined = baseUrl.replace(/\/+$/, "") + "/" + src.replace(/^\.?\//, "");
  return convert(joined);
}
