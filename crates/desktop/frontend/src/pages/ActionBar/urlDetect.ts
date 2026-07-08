const IPV4_RE = /^\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}(:\d+)?(\/.*)?$/;
const LOCALHOST_RE = /^localhost(:\d+)?(\/.*)?$/i;

/**
 * 宽松 URL 检测——比剪贴板 detectUrl 更宽松。
 * 三条路径：域名格式 / IP 地址 / localhost。
 */
export function detectActionUrl(text: string): { isUrl: boolean; url: string } {
  const t = text.trim();
  if (!t || t.includes(" ")) return { isUrl: false, url: "" };

  // localhost（无 .）
  if (LOCALHOST_RE.test(t)) {
    return { isUrl: true, url: `http://${t}` };
  }

  // IP 地址
  if (IPV4_RE.test(t)) {
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
