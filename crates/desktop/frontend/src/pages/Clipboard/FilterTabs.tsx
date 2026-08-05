import { cn } from "@/lib/utils";
import { LayoutGrid, ScanText, Layers } from "lucide-react";
import { SvgIcon, type IconName } from "@/components/SvgIcon";
import { useT } from "@/lib/i18n";

// 顺序必须与 index.tsx TABS_VALUES 一致——Cmd+N / Ctrl+N 按 tab 在数组中的序号映射。
const TAB_DEFS = [
  { value: "all", icon: LayoutGrid, labelKey: "clipboard.filter.all", svg: undefined as string | undefined },
  { value: "favorite", icon: null, labelKey: "clipboard.filter.favorite", svg: "favorite" },
  { value: "asr", icon: null, labelKey: "clipboard.filter.voice", svg: "voice" },
  { value: "text", icon: null, labelKey: "clipboard.filter.text", svg: "text" },
  { value: "ocr", icon: ScanText, labelKey: "OCR", svg: undefined },
  { value: "image", icon: null, labelKey: "clipboard.filter.image", svg: "images" },
  { value: "file", icon: null, labelKey: "clipboard.filter.file", svg: "files" },
  { value: "queue", icon: Layers, labelKey: "clipboard.filter.queue", svg: undefined },
] as const;

// 文字显隐阈值：tab 数 > COMPACT_THRESHOLD 时所有 tab 只显图标（含「全部」），
// 靠 title 属性出 tooltip。2026-08-05：tab 数达 8 个后「全部 + 图标」组合挤到换行。
const COMPACT_THRESHOLD = 6;

export default function FilterTabs({
  value,
  onChange,
}: {
  value: string;
  onChange: (v: string) => void;
}) {
  const t = useT();
  // tab 数超过阈值 → 紧凑模式（仅图标），否则「全部」tab 显文字。
  const compact = TAB_DEFS.length > COMPACT_THRESHOLD;
  return (
    <div className="flex items-center gap-0.5">
      {TAB_DEFS.map(({ value: tabValue, icon: Icon, labelKey, svg }, i) => {
        const label = labelKey === "OCR" ? "OCR" : t(labelKey);
        // 紧凑模式：全部 tab 只图标；非紧凑：「全部」显图标 + 文字（旧行为）。
        const showLabel = !compact && tabValue === "all";
        return (
        <button
          key={tabValue}
          data-tab-index={i}
          title={label}
          className={cn(
            "flex items-center justify-center px-2 py-1 rounded transition-all",
            value === tabValue
              ? "bg-primary text-primary-foreground"
              : "text-muted-foreground hover:text-foreground hover:bg-accent",
          )}
          onClick={() => onChange(tabValue)}
        >
          {svg ? (
            <SvgIcon name={svg as IconName} size={16} />
          ) : Icon ? (
            <Icon className="w-4 h-4" />
          ) : null}
          {showLabel && <span className="ml-1 text-xs">{label}</span>}
        </button>
        );
      })}
    </div>
  );
}
