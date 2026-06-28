import { cn } from "@/lib/utils";
import { LayoutGrid } from "lucide-react";

const TABS = [
  { value: "all", icon: LayoutGrid, label: "全部", svg: undefined as string | undefined },
  { value: "asr", icon: null, label: "语音", svg: "voice" },
  { value: "text", icon: null, label: "文本", svg: "text" },
  { value: "image", icon: null, label: "图片", svg: "images" },
  { value: "file", icon: null, label: "文件", svg: "files" },
  { value: "favorite", icon: null, label: "收藏", svg: "favorite" },
] as const;

export default function FilterTabs({
  value,
  onChange,
}: {
  value: string;
  onChange: (v: string) => void;
}) {
  return (
    <div className="flex items-center gap-0.5">
      {TABS.map(({ value: tabValue, icon: Icon, label, svg }) => (
        <button
          key={tabValue}
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
            <img src={`icons/${svg}.svg`} alt={label} className="w-4 h-4" style={{ filter: value === tabValue ? "brightness(0) invert(1)" : "none" }} />
          ) : Icon ? (
            <Icon className="w-4 h-4" />
          ) : null}
          {tabValue === "all" && <span className="ml-1 text-xs">{label}</span>}
        </button>
      ))}
    </div>
  );
}
