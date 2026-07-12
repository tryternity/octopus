import { Languages, ArrowRight } from "lucide-react";
import { useT } from "@/lib/i18n";

export default function TranslateTab() {
  const t = useT();
  const engines = [
    { name: "DeepL", desc: t("settings.models.translate.descHighQuality") },
    { name: "Google Translate", desc: t("settings.models.translate.descFree") },
    { name: "百度翻译", desc: t("settings.models.translate.descCn") },
    { name: "Argos Translate", desc: t("settings.models.translate.descLocal") },
  ];

  return (
    <div className="max-w-[560px]">
      <div className="flex flex-col items-center gap-2 py-8 px-4 rounded-lg border border-dashed border-border/60">
        <Languages className="w-6 h-6 text-muted-foreground/40" />
        <span className="text-xs font-medium text-muted-foreground">{t("settings.models.translate.comingSoon")}</span>
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
