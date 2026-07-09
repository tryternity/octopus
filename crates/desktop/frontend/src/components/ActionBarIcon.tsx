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

const svgCache: Record<string, string> = {};

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
  const [svgHtml, setSvgHtml] = useState<string | null>(null);

  useEffect(() => {
    if (!icon) { setSvgHtml(null); return; }

    if (icon.startsWith("<svg")) {
      setSvgHtml(icon);
      return;
    }

    if (icon.endsWith(".svg") || !LUCIDE_PATHS[icon]) {
      loadSvgFile(icon).then((content) => {
        if (!content) { setSvgHtml(null); return; }
        // 提取 SVG inner HTML + 原始 viewBox（保留原始比例，不强制 24×24）
        const match = content.match(/<svg[^>]*>([\s\S]*?)<\/svg>/i);
        const inner = match ? match[1] : "";
        const viewBoxMatch = content.match(/viewBox="([^"]+)"/i);
        const viewBox = viewBoxMatch ? viewBoxMatch[1] : "0 0 24 24";
        const hasStroke = content.includes("stroke");
        const hasMulticolor = /fill="#[^0]/.test(content);
        setSvgHtml(
          `<svg xmlns="http://www.w3.org/2000/svg" viewBox="${viewBox}" ` +
          (hasMulticolor
            ? '' // 品牌 SVG（Google/百度/Bing 多色）保持原始 fill 不覆盖
            : hasStroke
              ? 'fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"'
              : 'fill="currentColor"') +
          ` width="100%" height="100%">${inner}</svg>`
        );
      });
      return;
    }

    setSvgHtml(null);
  }, [icon]);

  // Lucide fallback
  if (!icon.endsWith(".svg") && !icon.startsWith("<svg") && LUCIDE_PATHS[icon] && !svgHtml) {
    return (
      <svg
        className={className}
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        strokeWidth={2}
        strokeLinecap="round"
        strokeLinejoin="round"
        style={{ width: "1em", height: "1em" }}
      >
        <g dangerouslySetInnerHTML={{ __html: LUCIDE_PATHS[icon] }} />
      </svg>
    );
  }

  if (!svgHtml) return <span className={className} style={{ width: "1em", height: "1em", display: "inline-block" }} />;

  return (
    <i
      className={className}
      style={{ display: "inline-flex", width: "1em", height: "1em", lineHeight: 0 }}
      dangerouslySetInnerHTML={{ __html: svgHtml }}
    />
  );
}
