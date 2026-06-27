import { useState, useRef, useEffect } from "react";
import { cn } from "@/lib/utils";
import { invoke } from "@/lib/tauri";
import { Loader2 } from "lucide-react";

export type ImageFormat = "png" | "webp" | "jpeg";

const FORMATS: { value: ImageFormat; label: string }[] = [
  { value: "webp", label: "WebP" },
  { value: "jpeg", label: "JPEG" },
  { value: "png", label: "PNG" },
];

export default function SaveImagePopover({
  id,
  onClose,
}: {
  id: number;
  onClose: () => void;
}) {
  const [format, setFormat] = useState<ImageFormat>("webp");
  const [quality, setQuality] = useState(90);
  const [saving, setSaving] = useState(false);
  const ref = useRef<HTMLDivElement>(null);

  // 点击外部关闭
  useEffect(() => {
    const handler = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) onClose();
    };
    document.addEventListener("mousedown", handler);
    return () => document.removeEventListener("mousedown", handler);
  }, [onClose]);

  const handleSave = async () => {
    setSaving(true);
    try {
      const lossy = format !== "png";
      await invoke("save_image_item", {
        id,
        format,
        quality: lossy ? quality : null,
      });
      onClose();
    } catch (e) {
      if (e !== "用户取消" && String(e) !== "用户取消") console.error(e);
      onClose();
    }
  };

  const lossy = format !== "png";

  return (
    <div
      ref={ref}
      className="absolute right-0 top-full mt-1 z-50 w-[220px] bg-popover border border-border rounded-lg shadow-xl p-2.5 space-y-2.5"
      onClick={(e) => e.stopPropagation()}
    >
      {/* 格式选择 */}
      <div className="flex gap-1">
        {FORMATS.map((f) => (
          <button
            key={f.value}
            className={cn(
              "flex-1 text-[10px] py-1 rounded font-medium transition-colors",
              format === f.value
                ? "bg-primary text-primary-foreground"
                : "bg-muted text-muted-foreground hover:bg-accent hover:text-foreground",
            )}
            onClick={() => setFormat(f.value)}
          >
            {f.label}
          </button>
        ))}
      </div>

      {/* 质量滑块（仅 WebP/JPEG） */}
      {lossy && (
        <div className="space-y-1">
          <div className="flex justify-between text-[10px] text-muted-foreground">
            <span>质量</span>
            <span className="tabular-nums font-medium text-foreground">{quality}</span>
          </div>
          <input
            type="range"
            min={10}
            max={100}
            value={quality}
            onChange={(e) => setQuality(Number(e.target.value))}
            className="w-full h-1 accent-primary cursor-pointer"
          />
        </div>
      )}

      {/* 保存按钮 */}
      <button
        className="w-full flex items-center justify-center gap-1 py-1.5 rounded text-[11px] font-medium bg-primary text-primary-foreground hover:bg-primary/90 transition-colors disabled:opacity-50"
        onClick={handleSave}
        disabled={saving}
      >
        {saving ? <Loader2 className="w-3 h-3 animate-spin" /> : null}
        保存
      </button>
    </div>
  );
}
