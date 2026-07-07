import { cn } from "@/lib/utils";
import { LayoutGrid, ScanText } from "lucide-react";
import { SvgIcon, type IconName } from "@/components/SvgIcon";

const TABS = [
  { value: "all", icon: LayoutGrid, label: "全部", svg: undefined as string | undefined },
  { value: "favorite", icon: null, label: "收藏", svg: "favorite" },
  { value: "asr", icon: null, label: "语音", svg: "voice" },
  { value: "text", icon: null, label: "文本", svg: "text" },
  { value: "ocr", icon: ScanText, label: "OCR", svg: undefined },
  { value: "image", icon: null, label: "图片", svg: "images" },
  { value: "file", icon: null, label: "文件", svg: "files" },
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
            // SvgIcon 用 mask + currentColor：未选中随 button 的 text-muted-foreground 置灰，
            // 选中随 text-primary-foreground 变白，深色模式自动跟随主题（img 加载的 SVG 不响应
            // currentColor，未选中恒为黑色剪影，深色背景下不可见）。
            <SvgIcon name={svg as IconName} size={16} />
          ) : Icon ? (
            <Icon className="w-4 h-4" />
          ) : null}
          {tabValue === "all" && <span className="ml-1 text-xs">{label}</span>}
        </button>
      ))}
    </div>
  );
}
