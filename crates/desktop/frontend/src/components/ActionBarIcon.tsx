import { useState, useEffect } from "react";

interface ActionBarIconProps {
  icon: string;
  className?: string;
}

const LUCIDE_PATHS: Record<string, string> = {
  pencil: '<path d="M21.174 6.812a1 1 0 0 0-3.986-3.987L3.842 16.174a2 2 0 0 0-.5.83l-1.321 4.352a.5.5 0 0 0 .623.622l4.353-1.32a2 2 0 0 0 .83-.497z"/><path d="m15 5 4 4"/>',
  "file-text": '<path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z"/><path d="M14 2v4a2 2 0 0 0 2 2h4"/><path d="M16 13H8"/><path d="M16 17H8"/><path d="M10 9H8"/>',
  lightbulb: '<path d="M15 14c.2-1 .7-1.7 1.5-2.5 1-.9 1.5-2.2 1.5-3.5A6 6 0 0 0 6 8c0 1 .2 2.2 1.5 3.5.7.7 1.3 1.5 1.5 2.5"/><path d="M9 18h6"/><path d="M10 22h4"/>',
};

// 缓存已加载的 SVG 文件内容（避免重复 fetch）
const svgCache: Record<string, string> = {};

/** 从 /icons/ 目录加载 SVG 文件内容 */
function loadSvgFile(filename: string): Promise<string | null> {
  const name = filename.replace(/\.svg$/, "");
  const path = `/icons/${name}.svg`;
  if (svgCache[path]) return Promise.resolve(svgCache[path]);
  return fetch(path)
    .then((r) => (r.ok ? r.text() : null))
    .then((text) => {
      if (text) svgCache[path] = text;
      return text;
    })
    .catch(() => null);
}

export function ActionBarIcon({ icon, className }: ActionBarIconProps) {
  const [svgContent, setSvgContent] = useState<string | null>(null);

  useEffect(() => {
    if (!icon) return;
    if (icon.startsWith("<svg")) {
      setSvgContent(icon);
      return;
    }
    // 文件名 → 从 /icons/ 加载
    if (icon.endsWith(".svg") || !LUCIDE_PATHS[icon]) {
      loadSvgFile(icon).then(setSvgContent);
      return;
    }
    // Lucide 预置路径
    setSvgContent(null);
  }, [icon]);

  // 1. 内联 SVG 或文件 SVG → 直接渲染
  if (svgContent) {
    return (
      <span
        className={className}
        style={{ display: "inline-flex", alignItems: "center", justifyContent: "center", width: "1em", height: "1em" }}
        dangerouslySetInnerHTML={{ __html: svgContent }}
      />
    );
  }

  // 2. Lucide 预置路径 → 组装完整 SVG
  const lucidePath = LUCIDE_PATHS[icon];
  if (lucidePath) {
    return (
      <span
        className={className}
        style={{ display: "inline-flex", alignItems: "center", justifyContent: "center", width: "1em", height: "1em" }}
        dangerouslySetInnerHTML={{
          __html: `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" width="100%" height="100%">${lucidePath}</svg>`,
        }}
      />
    );
  }

  return null;
}
