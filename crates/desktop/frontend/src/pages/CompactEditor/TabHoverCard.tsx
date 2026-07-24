import { createPortal } from "react-dom";
import { useT } from "@/lib/i18n";
import type { Tab } from "./index";

/** source → 显示名 + 色条颜色 */
const SOURCE_META: Record<string, { labelKey: string; color: string }> = {
  clipboard: { labelKey: "tab.sourceClipboard", color: "bg-muted-foreground" },
  transcription: { labelKey: "tab.sourceTranscription", color: "bg-violet-500" },
  temp: { labelKey: "tab.sourceTemp", color: "bg-amber-500" },
  file: { labelKey: "tab.sourceFile", color: "bg-emerald-500" },
};

/** meta 行：label : value。wrap=true 时 value 换行展示（用于长路径）。 */
function MetaRow({ label, value, wrap }: { label: string; value: string; wrap?: boolean }) {
  return (
    <div className={`flex gap-1.5 text-[11px] leading-relaxed ${wrap ? "items-start" : "items-baseline"}`}>
      <span className="shrink-0 text-muted-foreground/60">{label}</span>
      <span className={`text-foreground/90 ${wrap ? "break-all font-mono" : "truncate"}`}>{value}</span>
    </div>
  );
}

interface TabHoverCardProps {
  tab: Tab;
  /** tab 元素的 BoundingClientRect——用 fixed 定位避开父容器 overflow 裁剪 */
  rect: DOMRect;
}

/**
 * tab hover 浮层——展示该 tab 对应内容的 meta 信息。
 * 用 Portal 渲染到 body + position:fixed，避免被 tab 栏 overflow-x-auto 裁剪。
 * 纯展示，不查 DB（用 Tab 对象已有字段）。
 */
export default function TabHoverCard({ tab, rect }: TabHoverCardProps) {
  const t = useT();
  const meta = SOURCE_META[tab.source] ?? SOURCE_META.clipboard;

  // 浮层在 tab 下方（tab 栏在顶部，下方内容区有空间），6px 间距
  const top = rect.bottom + 6;
  const left = rect.left;

  return createPortal(
    <div
      className="fixed z-[9999] min-w-[180px] max-w-[280px] rounded-md border border-input bg-background p-2.5 shadow-lg"
      style={{
        top: `${top}px`,
        left: `${left}px`,
      }}
    >
      {/* source 色点 + 来源名 */}
      <div className="mb-1.5 flex items-center gap-1.5">
        <span className={`h-2 w-2 rounded-full ${meta.color}`} />
        <span className="text-[11px] font-medium text-foreground">{t(meta.labelKey)}</span>
      </div>

      {/* 各 source 特定 meta */}
      <div className="space-y-0.5">
        {tab.source === "file" && tab.filePath && (
          <>
            <MetaRow label={t("tab.metaPath")} value={tab.filePath} wrap />
            <MetaRow
              label={t("tab.metaStatus")}
              value={
                tab.text !== tab.originalText
                  ? t("tab.statusEdited")
                  : t("tab.statusSaved")
              }
            />
          </>
        )}

        {tab.source === "clipboard" && (
          <>
            <MetaRow label="ID" value={`#${tab.itemId}`} />
            {tab.itemType === "image" ? (
              <MetaRow
                label={t("tab.metaSize")}
                value={`${tab.imgWidth || 0} × ${tab.imgHeight || 0}`}
              />
            ) : (
              <MetaRow
                label={t("tab.metaChars")}
                value={`${(tab.text || "").length} ${t("tab.unitChars")}`}
              />
            )}
          </>
        )}

        {tab.source === "transcription" && (
          <>
            <MetaRow label="ID" value={`#${tab.itemId}`} />
            <MetaRow label={t("tab.metaStatus")} value={t("tab.statusReadonly")} />
          </>
        )}

        {tab.source === "temp" && (
          <MetaRow label={t("tab.metaStatus")} value={t("tab.statusUnsaved")} />
        )}
      </div>
    </div>,
    document.body,
  );
}
