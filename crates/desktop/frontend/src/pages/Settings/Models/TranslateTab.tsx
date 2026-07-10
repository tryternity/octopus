import { Languages } from "lucide-react";

export default function TranslateTab() {
  return (
    <div className="flex flex-col items-center justify-center h-full gap-2 text-muted-foreground/50 py-16">
      <Languages className="w-8 h-8" />
      <span className="text-xs">翻译模型配置即将支持</span>
      <span className="text-[10px]">未来可接入 DeepL / Google / 百度 / 本地 Argos 等翻译引擎</span>
    </div>
  );
}
