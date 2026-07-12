import { cn } from "@/lib/utils";
import { LayoutGrid, ScanText } from "lucide-react";
import { SvgIcon, type IconName } from "@/components/SvgIcon";
import { useT } from "@/lib/i18n";

const TAB_DEFS = [
  { value: "all", icon: LayoutGrid, labelKey: "clipboard.filter.all", svg: undefined as string | undefined },
  { value: "favorite", icon: null, labelKey: "clipboard.filter.favorite", svg: "favorite" },
  { value: "asr", icon: null, labelKey: "clipboard.filter.voice", svg: "voice" },
  { value: "text", icon: null, labelKey: "clipboard.filter.text", svg: "text" },
  { value: "ocr", icon: ScanText, labelKey: "OCR", svg: undefined },
  { value: "image", icon: null, labelKey: "clipboard.filter.image", svg: "images" },
  { value: "file", icon: null, labelKey: "clipboard.filter.file", svg: "files" },
] as const;

export default function FilterTabs({
  value,
  onChange,
}: {
  value: string;
  onChange: (v: string) => void;
}) {
  const t = useT();
  return (
    <div className="flex items-center gap-0.5">
      {TAB_DEFS.map(({ value: tabValue, icon: Icon, labelKey, svg }, i) => {
        const label = labelKey === "OCR" ? "OCR" : t(labelKey);
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
          {tabValue === "all" && <span className="ml-1 text-xs">{label}</span>}
        </button>
        );
      })}
    </div>
  );
}
