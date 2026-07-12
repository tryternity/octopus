import { useState } from "react";
import { cn } from "@/lib/utils";
import { useT } from "@/lib/i18n";
import EnvironmentTab from "./Models/EnvironmentTab";
import AsrTab from "./Models/AsrTab";
import LlmTab from "./Models/LlmTab";
import OcrTab from "./Models/OcrTab";
import TranslateTab from "./Models/TranslateTab";

const TABS = [
  { key: "env", labelKey: "settings.models.tab.env" },
  { key: "asr", labelKey: "settings.models.tab.asr" },
  { key: "llm", labelKey: "settings.models.tab.llm" },
  { key: "ocr", labelKey: "settings.models.tab.ocr" },
  { key: "tr", labelKey: "settings.models.tab.translate" },
] as const;

export default function ModelsPanel({ showToast }: { showToast: (msg: string) => void }) {
  const t = useT();
  const [activeTab, setActiveTab] = useState<string>("env");

  return (
    <div className="flex flex-col h-full">
      {/* Pill tab 条 */}
      <div className="flex gap-1 px-2 pt-1 pb-2 border-b border-border">
        {TABS.map((tab) => (
          <button
            key={tab.key}
            className={cn(
              "px-2.5 py-1 text-[11px] font-medium rounded-md transition-all duration-150",
              activeTab === tab.key
                ? "bg-foreground text-background"
                : "text-muted-foreground hover:text-foreground hover:bg-accent",
            )}
            onClick={() => setActiveTab(tab.key)}
          >
            {t(tab.labelKey)}
          </button>
        ))}
      </div>
      {/* Tab 内容 */}
      <div className="flex-1 overflow-y-auto px-3 py-2">
        {activeTab === "env" && <EnvironmentTab showToast={showToast} />}
        {activeTab === "asr" && <AsrTab showToast={showToast} />}
        {activeTab === "llm" && <LlmTab showToast={showToast} />}
        {activeTab === "ocr" && <OcrTab showToast={showToast} />}
        {activeTab === "tr" && <TranslateTab />}
      </div>
    </div>
  );
}
