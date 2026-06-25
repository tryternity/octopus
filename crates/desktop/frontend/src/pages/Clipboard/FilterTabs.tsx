import { cn } from "@/lib/utils";
import { LayoutGrid, Mic, Type, Image as ImageIcon, FileText, Star } from "lucide-react";

const TABS = [
  { value: "all", icon: LayoutGrid, label: "全部" },
  { value: "asr", icon: Mic, label: "ASR" },
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
    <div className="flex items-center gap-1 px-3 py-1.5 border-b border-border overflow-x-auto">
      {TABS.map(({ value: tabValue, icon: Icon, label }) => (
        <button
          key={tabValue}
          className={cn(
            "flex items-center gap-1 px-2.5 py-1 rounded-md text-xs transition-colors whitespace-nowrap",
            value === tabValue
              ? "bg-primary text-primary-foreground"
              : "text-muted-foreground hover:bg-accent hover:text-accent-foreground",
          )}
          onClick={() => onChange(tabValue)}
        >
          <Icon className="w-3.5 h-3.5" />
          <span>{label}</span>
        </button>
      ))}
    </div>
  );
}
