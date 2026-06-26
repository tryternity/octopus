import { cn } from "@/lib/utils";
import { LayoutGrid, Mic, Type, Image as ImageIcon, FileText, Star } from "lucide-react";

const TABS = [
  { value: "all", icon: LayoutGrid, label: "全部" },
  { value: "asr", icon: Mic, label: "语音" },
  { value: "text", icon: Type, label: "文本" },
  { value: "image", icon: ImageIcon, label: "图片" },
  { value: "file", icon: FileText, label: "文件" },
  { value: "favorite", icon: Star, label: "收藏" },
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
      {TABS.map(({ value: tabValue, icon: Icon, label }) => (
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
          <Icon className="w-4 h-4" />
          {tabValue === "all" && <span className="ml-1 text-xs">{label}</span>}
        </button>
      ))}
    </div>
  );
}
