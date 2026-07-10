import { useState } from "react";
import { cn } from "@/lib/utils";
import EnvironmentTab from "./Models/EnvironmentTab";
import AsrTab from "./Models/AsrTab";
import LlmTab from "./Models/LlmTab";
import OcrTab from "./Models/OcrTab";
import TranslateTab from "./Models/TranslateTab";

const TABS = ["常量", "语音识别", "文本模型", "扫描识别", "翻译模型"] as const;
type TabName = typeof TABS[number];

export default function ModelsPanel({ showToast }: { showToast: (msg: string) => void }) {
  const [activeTab, setActiveTab] = useState<TabName>("常量");

  return (
    <div className="flex flex-col h-full">
      <div className="flex gap-1 border-b border-border px-2">
        {TABS.map((tab) => (
          <button
            key={tab}
            className={cn(
              "px-3 py-1.5 text-xs font-medium transition-colors border-b-2 -mb-px",
              activeTab === tab
                ? "text-voice border-voice"
                : "text-muted-foreground hover:text-foreground border-transparent",
            )}
            onClick={() => setActiveTab(tab)}
          >
            {tab}
          </button>
        ))}
      </div>
      <div className="flex-1 overflow-y-auto p-3">
        {activeTab === "常量" && <EnvironmentTab showToast={showToast} />}
        {activeTab === "语音识别" && <AsrTab showToast={showToast} />}
        {activeTab === "文本模型" && <LlmTab showToast={showToast} />}
        {activeTab === "扫描识别" && <OcrTab showToast={showToast} />}
        {activeTab === "翻译模型" && <TranslateTab />}
      </div>
    </div>
  );
}
