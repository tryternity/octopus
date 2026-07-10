import { Languages, ArrowRight } from "lucide-react";

export default function TranslateTab() {
  const engines = [
    { name: "DeepL", desc: "高质量商业翻译 API" },
    { name: "Google Translate", desc: "免费通用翻译" },
    { name: "百度翻译", desc: "国内免费翻译" },
    { name: "Argos Translate", desc: "本地离线翻译" },
  ];

  return (
    <div className="max-w-[560px]">
      <div className="flex flex-col items-center gap-2 py-8 px-4 rounded-lg border border-dashed border-border/60">
        <Languages className="w-6 h-6 text-muted-foreground/40" />
        <span className="text-xs font-medium text-muted-foreground">翻译模型配置即将支持</span>
      </div>
      <div className="flex flex-col gap-1 mt-3">
        {engines.map((e) => (
          <div
            key={e.name}
            className="flex items-center gap-2 py-2 px-3 rounded-md border-l-2 border-border/40 opacity-50"
          >
            <span className="text-xs font-medium">{e.name}</span>
            <ArrowRight className="w-2.5 h-2.5 text-muted-foreground/30" />
            <span className="text-[11px] text-muted-foreground/60">{e.desc}</span>
          </div>
        ))}
      </div>
    </div>
  );
}
