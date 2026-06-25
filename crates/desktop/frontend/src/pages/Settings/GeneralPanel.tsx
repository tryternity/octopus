import { useState, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { cn } from "@/lib/utils";
import type { ConfigResponse } from "./index";

interface GeneralPanelProps {
  configResp: ConfigResponse;
  setVal: (key: string, value: string | number | boolean) => Promise<void>;
  showToast: (msg: string) => void;
  refreshConfig: () => Promise<void>;
}

function Card({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <div className="bg-card border border-border rounded-lg p-4 mb-4">
      <h3 className="text-sm font-semibold mb-3">{title}</h3>
      {children}
    </div>
  );
}

function Row({ label, hint, effect, children }: { label: string; hint?: string; effect?: string; children: React.ReactNode }) {
  return (
    <div className="flex items-center justify-between py-2 border-b border-border last:border-0 gap-2">
      <div className="flex flex-col gap-0.5 flex-1 min-w-0">
        <span className="text-sm">{label} {effect && <span className="text-[11px] text-muted-foreground">{effect}</span>}</span>
        {hint && <span className="text-xs text-muted-foreground">{hint}</span>}
      </div>
      {children}
    </div>
  );
}

function Toggle({ on, onClick }: { on: boolean; onClick: () => void }) {
  return (
    <button
      className={cn("relative w-11 h-6 rounded-full transition-colors flex-shrink-0", on ? "bg-green-500" : "bg-muted")}
      onClick={onClick}
    >
      <span className={cn("absolute top-0.5 left-0.5 w-5 h-5 bg-white rounded-full transition-transform shadow-sm", on && "translate-x-5")} />
    </button>
  );
}

const selectClass = "px-2.5 py-1.5 border border-border rounded-md text-sm bg-white min-w-[180px] max-w-[220px]";

export default function GeneralPanel({ configResp, setVal, showToast, refreshConfig }: GeneralPanelProps) {
  const { config: cfg, asr_engines, llm_models, prompts, active_prompt_id, microphones } = configResp;
  const [capturingKey, setCapturingKey] = useState<string | null>(null);

  const toggleVal = useCallback(async (key: string) => {
    const current = cfg[key] as boolean;
    await setVal(key, !current);
  }, [cfg, setVal]);

  const setActivePrompt = useCallback(async (id: number) => {
    try {
      await invoke("set_active_prompt", { id });
      await refreshConfig();
    } catch (e) { showToast("设置失败：" + e); }
  }, [refreshConfig, showToast]);

  const startShortcutCapture = useCallback((configKey: string) => {
    setCapturingKey(configKey);
    const handler = async (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      if (e.key === "Escape") {
        setCapturingKey(null);
        cleanup();
        return;
      }
      const parts: string[] = [];
      if (e.metaKey || e.ctrlKey) parts.push("CmdOrCtrl");
      if (e.altKey) parts.push("Alt");
      if (e.shiftKey) parts.push("Shift");
      const keyName = e.code.startsWith("Key") ? e.code.slice(3) : e.code;
      parts.push(keyName);
      const shortcut = parts.join("+");
      try {
        await invoke("check_shortcut", { shortcut });
        await setVal(configKey, shortcut);
      } catch (err) {
        showToast("" + err);
      }
      setCapturingKey(null);
      cleanup();
    };
    const cleanup = () => {
      document.removeEventListener("keydown", handler, true);
    };
    document.addEventListener("keydown", handler, true);
  }, [setVal, showToast]);

  return (
    <div>
      <Card title="交互">
        <Row label="麦克风设备" effect="(下次录音)">
          <select className={selectClass} value={cfg.microphone as string} onChange={(e) => setVal("microphone", e.target.value)}>
            <option value="">系统默认</option>
            {microphones.map((m) => <option key={m} value={m}>{m}</option>)}
          </select>
        </Row>
        <Row label="降噪模式" effect="(立即)">
          <select className={selectClass} value={cfg.denoise_mode as number} onChange={(e) => setVal("denoise_mode", parseInt(e.target.value))}>
            <option value={0}>无</option><option value={1}>轻度</option><option value={2}>深度</option>
          </select>
        </Row>
        <Row label="激活/关闭快捷键" effect="(立即)">
          <button
            className={cn("px-3.5 py-1.5 border rounded-md text-sm min-w-[180px] max-w-[220px] text-center cursor-pointer transition-colors hover:border-primary",
              capturingKey === "asr_shortcut" && "border-primary text-muted-foreground")}
            onClick={() => startShortcutCapture("asr_shortcut")}
          >
            {capturingKey === "asr_shortcut" ? "按下快捷键..." : cfg.asr_shortcut as string}
          </button>
        </Row>
        <Row label="编辑快捷键" effect="(立即)" hint="结果窗聚焦时进入编辑模式">
          <button
            className={cn("px-3.5 py-1.5 border rounded-md text-sm min-w-[180px] max-w-[220px] text-center cursor-pointer transition-colors hover:border-primary",
              capturingKey === "edit_shortcut" && "border-primary text-muted-foreground")}
            onClick={() => startShortcutCapture("edit_shortcut")}
          >
            {capturingKey === "edit_shortcut" ? "按下快捷键..." : cfg.edit_shortcut as string}
          </button>
        </Row>
        <Row label="工具栏自动隐藏" effect="(立即)" hint="开启=自动隐藏，关闭=始终显示">
          <Toggle on={cfg.hide_toolbar as boolean} onClick={() => toggleVal("hide_toolbar")} />
        </Row>
      </Card>

      <Card title="识别">
        <Row label="语音识别引擎" effect="(下次录音)">
          <select className={selectClass} value={cfg.asr_engine as string} onChange={(e) => setVal("asr_engine", e.target.value)}>
            {asr_engines.map((e) => <option key={e.name} value={e.name}>{e.label}</option>)}
          </select>
        </Row>
        <Row label="语音识别语言" effect="(下次录音)">
          <select className={selectClass} value={cfg.language as string} onChange={(e) => setVal("language", e.target.value)}>
            <option value="auto">自动</option><option value="zh">中文</option><option value="en">英语</option>
          </select>
        </Row>
        <Row label="硬件加速" effect="(下次录音)">
          <Toggle on={cfg.asr_hardware_accelerated as boolean} onClick={() => toggleVal("asr_hardware_accelerated")} />
        </Row>
        <Row label="语音识别纠错" effect="(立即)" hint="拼音映射 + bigram 校正">
          <Toggle on={cfg.asr_correct as boolean} onClick={() => toggleVal("asr_correct")} />
        </Row>
        <Row label="简繁输出" effect="(立即)" hint="开启=简体，关闭=繁体">
          <Toggle on={cfg.output_simplified as boolean} onClick={() => toggleVal("output_simplified")} />
        </Row>
        <Row label="句间停顿" effect="(毫秒，下次录音)" hint="说话停顿多久算一句话结束">
          <select className={selectClass} value={cfg.segment_silence as number} onChange={(e) => setVal("segment_silence", parseFloat(e.target.value))}>
            {[300, 400, 500, 600].map((v) => <option key={v} value={v}>{v}ms</option>)}
          </select>
        </Row>
      </Card>

      <Card title="润色">
        <Row label="文本润色模型" effect="(立即)">
          <select className={selectClass} value={cfg.polish_llm as string} onChange={(e) => setVal("polish_llm", e.target.value)}>
            {llm_models.map((m) => <option key={m.name} value={m.name}>{m.label}</option>)}
          </select>
        </Row>
        <Row label="文本润色模式" effect="(立即)">
          <select className={selectClass} value={cfg.polish_mode as number} onChange={(e) => setVal("polish_mode", parseInt(e.target.value))}>
            <option value={0}>关闭</option><option value={1}>仅最终润色</option><option value={2}>中间+最终润色</option>
          </select>
        </Row>
        <Row label="润色提示词" effect="(立即)" hint="选择不同风格 prompt">
          <select className={selectClass} value={active_prompt_id} onChange={(e) => setActivePrompt(parseInt(e.target.value))}>
            {prompts.map((p) => <option key={p.id} value={p.id}>{p.title}{p.is_system ? "（内置）" : ""}</option>)}
          </select>
        </Row>
        <Row label="文本润色间隔" effect="(下次录音)">
          <select className={selectClass} value={cfg.polish_min_interval as number} onChange={(e) => setVal("polish_min_interval", parseFloat(e.target.value))}>
            <option value={0}>仅最后</option>
            {[3, 4, 5, 6, 7, 8].map((v) => <option key={v} value={v}>每{v}秒</option>)}
          </select>
        </Row>
        <Row label="润色停顿阈值" effect="(下次录音)" hint="停顿超过此值时触发一次中间润色">
          <select className={selectClass} value={cfg.pause_polish_threshold_ms as number} onChange={(e) => setVal("pause_polish_threshold_ms", parseFloat(e.target.value))}>
            {[600, 700, 800, 900, 1000].map((v) => <option key={v} value={v}>{v}ms</option>)}
          </select>
        </Row>
      </Card>

      <Card title="引擎接入模式">
        <Row label="引擎接入模式" effect="(重启)" hint="embedded=本地推理">
          <select className={selectClass} value={cfg.engine_mode as string} onChange={(e) => setVal("engine_mode", e.target.value)}>
            <option value="embedded">embedded</option>
            <option value="websocket">websocket</option>
            <option value="grpc">grpc</option>
          </select>
        </Row>
      </Card>
    </div>
  );
}
