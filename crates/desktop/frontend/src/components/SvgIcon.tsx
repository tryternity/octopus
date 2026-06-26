import { cn } from "@/lib/utils";

const ICONS = {
  close: "/icons/close.svg",
  settings: "/icons/settings.svg",
  asr: "/icons/asr-engine.svg",
  denoise: "/icons/denoise.svg",
  llm: "/icons/llm-model.svg",
  polish: "/icons/polish-mode.svg",
  "polish-now": "/icons/polish-now.svg",
  edit: "/icons/edit.svg",
  save: "/icons/save.svg",
} as const;

export type IconName = keyof typeof ICONS;

export function SvgIcon({
  name,
  className,
  size = 14,
}: {
  name: IconName;
  className?: string;
  size?: number;
}) {
  return (
    <span
      className={cn("inline-block flex-shrink-0", className)}
      style={{
        width: size,
        height: size,
        backgroundColor: "currentColor",
        mask: `url(${ICONS[name]}) no-repeat center / contain`,
        WebkitMask: `url(${ICONS[name]}) no-repeat center / contain`,
      }}
    />
  );
}
