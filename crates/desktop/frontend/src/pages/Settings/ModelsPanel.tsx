import { useState } from "react";
import { useT } from "@/lib/i18n";
import { PillTabs } from "@/components/ui/tabs";
import AsrTab from "./Models/AsrTab";
import LlmTab from "./Models/LlmTab";
import OcrTab from "./Models/OcrTab";
import TranslateTab from "./Models/TranslateTab";

const TAB_KEYS = ["asr", "llm", "ocr", "tr"] as const;
const TAB_LABEL_KEYS: Record<string, string> = {
  asr: "settings.models.tab.asr",
  llm: "settings.models.tab.llm",
  ocr: "settings.models.tab.ocr",
  tr: "settings.models.tab.translate",
};

export default function ModelsPanel({ showToast }: { showToast: (msg: string) => void }) {
  const t = useT();
  const [activeTab, setActiveTab] = useState<string>("asr");

  const tabs = TAB_KEYS.map((key) => ({ key, label: t(TAB_LABEL_KEYS[key]) }));

  return (
    <div className="flex flex-col h-full">
      <PillTabs items={tabs} active={activeTab} onChange={setActiveTab} />
      {/* Tab 内容 */}
      <div className="flex-1 overflow-y-auto px-3 py-2">
        {activeTab === "asr" && <AsrTab showToast={showToast} />}
        {activeTab === "llm" && <LlmTab showToast={showToast} />}
        {activeTab === "ocr" && <OcrTab showToast={showToast} />}
        {activeTab === "tr" && <TranslateTab showToast={showToast} />}
      </div>
    </div>
  );
}
