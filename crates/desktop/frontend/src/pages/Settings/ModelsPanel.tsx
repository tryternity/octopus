import { useState } from "react";
import { cn } from "@/lib/utils";
import EnvironmentTab from "./Models/EnvironmentTab";
import AsrTab from "./Models/AsrTab";
import LlmTab from "./Models/LlmTab";
import OcrTab from "./Models/OcrTab";
import TranslateTab from "./Models/TranslateTab";

const TABS = [
  { name: "常量", icon: "var" },
  { name: "语音识别", icon: "asr" },
  { name: "文本模型", icon: "llm" },
  { name: "扫描识别", icon: "ocr" },
  { name: "翻译模型", icon: "tr" },
] as const;

export default function ModelsPanel({ showToast }: { showToast: (msg: string) => void }) {
  const [activeTab, setActiveTab] = useState<string>("常量");

  return (
    <div className="flex flex-col h-full">
      {/* Pill tab 条 */}
      <div className="flex gap-1 px-2 pt-1 pb-2 border-b border-border">
        {TABS.map((tab) => (
          <button
            key={tab.name}
            className={cn(
              "px-2.5 py-1 text-[11px] font-medium rounded-md transition-all duration-150",
              activeTab === tab.name
                ? "bg-foreground text-background"
                : "text-muted-foreground hover:text-foreground hover:bg-accent",
            )}
            onClick={() => setActiveTab(tab.name)}
          >
            {tab.name}
          </button>
        ))}
      </div>
      {/* Tab 内容 */}
      <div className="flex-1 overflow-y-auto px-3 py-2">
        {activeTab === "常量" && <EnvironmentTab showToast={showToast} />}
        {activeTab === "语音识别" && <AsrTab showToast={showToast} />}
        {activeTab === "文本模型" && <LlmTab showToast={showToast} />}
        {activeTab === "扫描识别" && <OcrTab showToast={showToast} />}
        {activeTab === "翻译模型" && <TranslateTab />}
      </div>
    </div>
  );
}
