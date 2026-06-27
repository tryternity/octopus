import { useState, useRef, useEffect } from "react";
import { cn } from "@/lib/utils";
import { invoke } from "@/lib/tauri";
import { Loader2, Check } from "lucide-react";

type ImageFormat = "jpeg" | "webp" | "png";

const FORMATS: { value: ImageFormat; label: string }[] = [
  { value: "jpeg", label: "JPEG" },
  { value: "webp", label: "WebP" },
  { value: "png", label: "PNG" },
];

export default function SaveImagePopover({
  id,
  onClose,
}: {
  id: number;
  onClose: () => void;
}) {
  const [format, setFormat] = useState<ImageFormat>("jpeg");
  const [quality, setQuality] = useState(85);
  const [saving, setSaving] = useState(false);
  const [done, setDone] = useState(false);
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const handler = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) onClose();
    };
    document.addEventListener("mousedown", handler);
    return () => document.removeEventListener("mousedown", handler);
  }, [onClose]);

  const lossy = format !== "png";

  const handleSave = async () => {
    setSaving(true);
    try {
      await invoke("save_image_item", {
        id,
        format,
        quality: lossy ? quality : null,
      });
      setSaving(false);
      setDone(true);
      setTimeout(onClose, 700);
    } catch (e) {
      setSaving(false);
      console.error(e);
      onClose();
    }
  };

  return (
    <div
      ref={ref}
      className="absolute right-0 top-full mt-1.5 z-50 w-[224px] bg-white rounded-xl border border-stone-200/80 shadow-[0_12px_40px_-6px_rgba(41,37,36,0.28),0_4px_12px_-4px_rgba(41,37,36,0.16)] p-3 space-y-3"
      onClick={(e) => e.stopPropagation()}
    >
      {/* 格式 — 分段控件 */}
      <div className="space-y-1.5">
        <div className="text-[9px] font-semibold uppercase tracking-[0.1em] text-stone-400">
          格式
        </div>
        <div className="flex p-0.5 bg-stone-100 rounded-lg gap-0.5">
          {FORMATS.map((f) => (
            <button
              key={f.value}
              className={cn(
                "flex-1 text-[11px] py-1.5 rounded-[6px] font-medium transition-all duration-150",
                format === f.value
                  ? "bg-white text-stone-900 shadow-[0_1px_3px_rgba(41,37,36,0.18)]"
                  : "text-stone-500 hover:text-stone-700",
              )}
              onClick={() => setFormat(f.value)}
            >
              {f.label}
            </button>
          ))}
        </div>
      </div>

      {/* 质量 — 细线滑轨 + 等宽数字（仅 WebP/JPEG） */}
      {lossy && (
        <div className="space-y-1.5">
          <div className="flex items-baseline justify-between">
            <span className="text-[9px] font-semibold uppercase tracking-[0.1em] text-stone-400">
              质量
            </span>
            <span className="text-[12px] font-semibold tabular-nums text-stone-800 leading-none">
              {quality}
            </span>
          </div>
          <input
            type="range"
            min={10}
            max={100}
            step={5}
            value={quality}
            onChange={(e) => setQuality(Number(e.target.value))}
            className="save-img-range w-full h-[3px] appearance-none bg-stone-200 rounded-full cursor-pointer
              [&::-webkit-slider-thumb]:appearance-none [&::-webkit-slider-thumb]:w-3 [&::-webkit-slider-thumb]:h-3
              [&::-webkit-slider-thumb]:rounded-full [&::-webkit-slider-thumb]:bg-stone-800
              [&::-webkit-slider-thumb]:shadow-sm [&::-webkit-slider-thumb]:cursor-grab
              [&::-webkit-slider-thumb]:active:cursor-grabbing"
            style={{
              background: `linear-gradient(to right, #292524 0%, #292524 ${((quality - 10) / 90) * 100}%, #e7e5e4 ${((quality - 10) / 90) * 100}%, #e7e5e4 100%)`,
            }}
          />
          <div className="flex justify-between text-[8px] text-stone-300 font-medium tabular-nums">
            <span>小</span>
            <span>大</span>
          </div>
        </div>
      )}

      {/* PNG 提示 */}
      {!lossy && (
        <div className="text-[10px] text-stone-400 leading-relaxed">
          PNG 为无损格式，不压缩
        </div>
      )}

      {/* 分隔线 */}
      <div className="h-px bg-stone-100" />

      {/* 保存按钮 */}
      <button
        className={cn(
          "w-full flex items-center justify-center gap-1.5 py-2 rounded-lg text-[11px] font-semibold transition-all duration-150",
          done
            ? "bg-emerald-600 text-white"
            : "bg-stone-900 text-white hover:bg-stone-800 active:scale-[0.98]",
        )}
        onClick={handleSave}
        disabled={saving || done}
      >
        {saving ? (
          <>
            <Loader2 className="w-3 h-3 animate-spin" />
            保存中
          </>
        ) : done ? (
          <>
            <Check className="w-3 h-3" />
            已保存到下载
          </>
        ) : (
          "保存到下载"
        )}
      </button>
    </div>
  );
}
