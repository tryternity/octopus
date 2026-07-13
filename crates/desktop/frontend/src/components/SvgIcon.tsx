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
  "cancel-editor": "/icons/cancel-edit.svg",
  note: "/icons/note.svg",
  "expand-edit": "/icons/expand-edit.svg",
  "minimize": "/icons/minimize.svg",
  "translate": "/icons/action-translate.svg",
  "redo": "/icons/redo.svg",
  // FilterTabs 类型图标（Font Awesome 单色剪影，mask 法跟随 currentColor）
  voice: "/icons/voice.svg",
  text: "/icons/text.svg",
  images: "/icons/images.svg",
  files: "/icons/files.svg",
  favorite: "/icons/favorite.svg",
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
