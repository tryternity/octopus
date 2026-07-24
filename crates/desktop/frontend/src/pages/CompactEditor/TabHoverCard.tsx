import { useT } from "@/lib/i18n";
import type { Tab } from "./index";

/** source → 显示名 + 色条颜色 */
const SOURCE_META: Record<string, { labelKey: string; color: string }> = {
  clipboard: { labelKey: "tab.sourceClipboard", color: "bg-muted-foreground" },
  transcription: { labelKey: "tab.sourceTranscription", color: "bg-violet-500" },
  temp: { labelKey: "tab.sourceTemp", color: "bg-amber-500" },
  file: { labelKey: "tab.sourceFile", color: "bg-emerald-500" },
};

/** meta 行：label : value */
function MetaRow({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex items-baseline gap-1.5 text-[11px] leading-relaxed">
      <span className="shrink-0 text-muted-foreground/60">{label}</span>
      <span className="truncate text-foreground/90">{value}</span>
    </div>
  );
}

interface TabHoverCardProps {
  tab: Tab;
}

/**
 * tab hover 浮层——展示该 tab 对应内容的 meta 信息。
 * 纯展示，不查 DB（用 Tab 对象已有字段）。
 */
export default function TabHoverCard({ tab }: TabHoverCardProps) {
  const t = useT();
  const meta = SOURCE_META[tab.source] ?? SOURCE_META.clipboard;

  return (
    <div className="absolute bottom-full left-0 z-50 mb-1.5 min-w-[180px] max-w-[280px] rounded-md border border-input bg-background p-2.5 shadow-lg">
      {/* source 色条 + 标题 */}
      <div className="mb-1.5 flex items-center gap-1.5">
        <span className={`h-2 w-2 rounded-full ${meta.color}`} />
        <span className="text-[11px] font-medium text-foreground">{t(meta.labelKey)}</span>
      </div>

      {/* 各 source 特定 meta */}
      <div className="space-y-0.5">
        {tab.source === "file" && tab.filePath && (
          <>
            <MetaRow label={t("tab.metaPath")} value={tab.filePath} />
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
    </div>
  );
}
