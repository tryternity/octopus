const IPV4_RE = /^\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}(:\d+)?(\/.*)?$/;
const LOCALHOST_RE = /^localhost(:\d+)?(\/.*)?$/i;
// 常见文件扩展名——命中则不当 URL（防 readme.md / photo.jpg / data.csv 误判为域名）
const FILE_EXT_RE = /\.(jpg|jpeg|png|gif|bmp|svg|webp|ico|csv|txt|pdf|docx?|xlsx?|pptx?|zip|tar|gz|rar|7z|md|markdown|json|xml|html?|css|jsx?|tsx?|rs|go|py|java|cpp?|sh|bat|ps1|ya?ml|toml|ini|conf|log|mp[34]|mov|avi|wav|flac)$/i;

/** IPV4_RE 只校验 \d{1,3}，这里补 0-255 范围校验（剔 999.999.999.999 等无效 IP）。 */
function isValidIpv4Host(t: string): boolean {
  const host = t.split(":")[0].split("/")[0];
  return host.split(".").every((p) => /^\d+$/.test(p) && Number(p) <= 255);
}

/**
 * 宽松 URL 检测——比剪贴板 detectUrl 更宽松。
 * 三条路径：域名格式 / IP 地址 / localhost。
 */
export function detectActionUrl(text: string): { isUrl: boolean; url: string } {
  const t = text.trim();
  if (!t || t.includes(" ")) return { isUrl: false, url: "" };

  // 文件名（含常见扩展名）不当 URL
  if (FILE_EXT_RE.test(t)) return { isUrl: false, url: "" };

  // localhost（无 .）
  if (LOCALHOST_RE.test(t)) {
    return { isUrl: true, url: `http://${t}` };
  }

  // IP 地址（含 0-255 范围校验）
  if (IPV4_RE.test(t) && isValidIpv4Host(t)) {
    return { isUrl: true, url: `http://${t}` };
  }

  // 域名格式：含 . 且不以 . 开头/结尾 且 . 两侧至少一侧含字母
  if (t.includes(".") && !t.startsWith(".") && !t.endsWith(".")) {
    const dotIdx = t.indexOf(".");
    const before = t.substring(0, dotIdx);
    const after = t.substring(dotIdx + 1);
    if (/[a-zA-Z]/.test(before) || /[a-zA-Z]/.test(after)) {
      return { isUrl: true, url: `https://${t}` };
    }
  }

  return { isUrl: false, url: "" };
}
