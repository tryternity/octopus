import { useState, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { cn } from "@/lib/utils";
import { Mic, Volume2, Sparkles, Keyboard, ClipboardList, Layers } from "lucide-react";
import type { ConfigResponse } from "./index";

interface GeneralPanelProps {
  configResp: ConfigResponse;
  setVal: (key: string, value: string | number | boolean) => Promise<void>;
  showToast: (msg: string) => void;
  refreshConfig: () => Promise<void>;
}

function Card({ icon: Icon, title, children }: { icon: React.ElementType; title: string; children: React.ReactNode }) {
  return (
    <div className="mb-3 border border-border rounded-lg overflow-hidden bg-background">
      <div className="flex items-center gap-2 px-4 py-2.5 bg-muted/40 border-b border-border">
        <Icon className="w-4 h-4 text-muted-foreground" />
        <h3 className="text-sm font-semibold">{title}</h3>
      </div>
      <div className="px-4 py-1">{children}</div>
    </div>
  );
}

function Row({ label, hint, effect, children }: { label: string; hint?: string; effect?: string; children: React.ReactNode }) {
  return (
    <div className="flex items-center justify-between py-2.5 border-b border-border/40 last:border-0 gap-3">
      <div className="flex flex-col gap-0.5 flex-1 min-w-0">
        <span className="text-sm">{label}{effect && <span className="ml-1.5 text-[10px] text-muted-foreground/60 px-1 py-0.5 rounded bg-muted">{effect}</span>}</span>
        {hint && <span className="text-xs text-muted-foreground/60">{hint}</span>}
      </div>
      {children}
    </div>
  );
}

function Toggle({ on, onClick }: { on: boolean; onClick: () => void }) {
  return (
    <button
      className={cn(
        "relative w-10 h-[22px] rounded-full transition-colors flex-shrink-0",
        on ? "bg-voice" : "bg-muted-foreground/25",
      )}
      onClick={onClick}
    >
      <span className={cn(
        "absolute top-0.5 left-0.5 w-[18px] h-[18px] bg-white rounded-full transition-transform shadow-sm",
        on && "translate-x-[18px]",
      )} />
    </button>
  );
}

function ShortcutButton({ shortcut, capturing, onClick }: { shortcut: string; capturing: boolean; onClick: () => void }) {
  if (capturing) {
    return (
      <button
        className="px-3 py-1.5 rounded-md text-xs font-medium text-voice bg-voice/5 border border-voice/40 cursor-pointer animate-pulse"
        onClick={onClick}
      >
        按下快捷键…（Esc 取消）
      </button>
    );
  }
  const keys = shortcut.split("+");
  return (
    <button
      className="flex items-center gap-1 px-2.5 py-1.5 rounded-md border border-border bg-stone-50 hover:border-foreground/30 cursor-pointer transition-colors group"
      onClick={onClick}
    >
      {keys.map((k, i) => (
        <span key={i} className="flex items-center gap-1">
          {i > 0 && <span className="text-muted-foreground/40 text-[10px]">+</span>}
          <kbd className="min-w-[20px] px-1.5 py-0.5 text-[11px] font-medium text-stone-700 bg-white rounded border border-stone-200 shadow-sm group-hover:border-stone-300 transition-colors">
            {k === "CmdOrCtrl" ? "⌘" : k === "Alt" ? "⌥" : k === "Shift" ? "⇧" : k}
          </kbd>
        </span>
      ))}
    </button>
  );
}

const selectClass = "px-2.5 py-1.5 border border-border rounded-md text-sm bg-background min-w-[160px] max-w-[200px] cursor-pointer hover:border-foreground/30 transition-colors outline-none focus:border-voice/40";

export default function GeneralPanel({ configResp, setVal, showToast, refreshConfig }: GeneralPanelProps) {
  const { config: cfg, asr_engines, llm_models, ocr_models, prompts, active_prompt_id, microphones } = configResp;
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
      if (e.key === "Escape") { setCapturingKey(null); cleanup(); return; }
      // 纯修饰键不触发，等待用户按实际键
      if (e.key === "Alt" || e.key === "Shift" || e.key === "Control" || e.key === "Meta") return;
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
      } catch (err) { showToast("" + err); }
      setCapturingKey(null);
      cleanup();
    };
    const cleanup = () => document.removeEventListener("keydown", handler, true);
    document.addEventListener("keydown", handler, true);
  }, [setVal, showToast]);

  return (
    <div className="max-w-[640px]">
      <Card icon={Mic} title="交互">
        <Row label="麦克风设备" effect="下次录音">
          <select className={selectClass} value={cfg.microphone as string} onChange={(e) => setVal("microphone", e.target.value)}>
            <option value="">系统默认</option>
            {microphones.map((m) => <option key={m} value={m}>{m}</option>)}
          </select>
        </Row>
        <Row label="降噪模式" effect="立即">
          <select className={selectClass} value={cfg.denoise_mode as number} onChange={(e) => setVal("denoise_mode", parseInt(e.target.value))}>
            <option value={0}>无</option><option value={1}>轻度</option><option value={2}>深度</option>
          </select>
        </Row>
        <Row label="识别工具栏自动隐藏" effect="立即" hint="关闭后始终显示">
          <Toggle on={cfg.hide_toolbar as boolean} onClick={() => toggleVal("hide_toolbar")} />
        </Row>
      </Card>

      <Card icon={Layers} title="模型选择">
        <Row label="语音识别模型" effect="下次录音">
          {/* 后端 asr_engine 存 3-part spec（"provider:category:name"），option value 是裸名，
              直接用 cfg.asr_engine 匹配不上 → 必须从 asr_engines 的 current 项取裸名（同润色模型行） */}
          <select className={selectClass}
            value={asr_engines.find((e) => e.current)?.name ?? ""}
            onChange={(e) => setVal("asr_engine", e.target.value)}>
            {asr_engines.map((e) => <option key={e.name} value={e.name}>{e.label}</option>)}
          </select>
        </Row>
        <Row label="润色模型" effect="立即">
          <select className={selectClass}
            value={llm_models.find((m) => m.current)?.name ?? ""}
            onChange={(e) => setVal("polish_llm", e.target.value)}>
            {llm_models.map((m) => <option key={m.name} value={m.name}>{m.label}</option>)}
          </select>
        </Row>
        <Row label="OCR 模型" effect="下次启动" hint="截图识别用，改后重启生效">
          <select className={selectClass} value={cfg.ocr_model as string} onChange={(e) => setVal("ocr_model", e.target.value)}>
            {ocr_models.map((m) => <option key={m.name} value={m.name}>{m.label}</option>)}
          </select>
        </Row>
      </Card>

      <Card icon={Keyboard} title="快捷键">
        <Row label="语音识别" effect="立即">
          <ShortcutButton shortcut={cfg.asr_shortcut as string} capturing={capturingKey === "asr_shortcut"} onClick={() => startShortcutCapture("asr_shortcut")} />
        </Row>
        <Row label="立即润色" effect="立即" hint="对当前识别结果立即润色">
          <ShortcutButton shortcut={cfg.polish_global_shortcut as string} capturing={capturingKey === "polish_global_shortcut"} onClick={() => startShortcutCapture("polish_global_shortcut")} />
        </Row>
        <Row label="语音编辑" effect="立即" hint="任意应用聚焦时唤起结果窗并编辑">
          <ShortcutButton shortcut={cfg.edit_global_shortcut as string} capturing={capturingKey === "edit_global_shortcut"} onClick={() => startShortcutCapture("edit_global_shortcut")} />
        </Row>
        <Row label="剪贴板浮窗" effect="立即">
          <ShortcutButton shortcut={cfg.clipboard_shortcut as string} capturing={capturingKey === "clipboard_shortcut"} onClick={() => startShortcutCapture("clipboard_shortcut")} />
        </Row>
      </Card>

      <Card icon={Volume2} title="语音识别">
        <Row label="识别语言" effect="下次录音">
          <select className={selectClass} value={cfg.language as string} onChange={(e) => setVal("language", e.target.value)}>
            <option value="auto">自动</option><option value="zh">中文</option><option value="en">英语</option>
          </select>
        </Row>
        <Row label="硬件加速" effect="下次录音">
          <Toggle on={cfg.asr_hardware_accelerated as boolean} onClick={() => toggleVal("asr_hardware_accelerated")} />
        </Row>
        <Row label="拼音纠错" effect="立即" hint="拼音映射 + bigram 校正">
          <Toggle on={cfg.asr_correct as boolean} onClick={() => toggleVal("asr_correct")} />
        </Row>
        <Row label="简繁输出" effect="立即" hint="开启 = 简体">
          <Toggle on={cfg.output_simplified as boolean} onClick={() => toggleVal("output_simplified")} />
        </Row>
        <Row label="句间停顿" effect="下次录音" hint="说话停顿多久算一句话结束">
          <select className={selectClass} value={cfg.segment_silence as number} onChange={(e) => setVal("segment_silence", parseFloat(e.target.value))}>
            {[300, 400, 500, 600].map((v) => <option key={v} value={v}>{v}ms</option>)}
          </select>
        </Row>
      </Card>

      <Card icon={Sparkles} title="语音识别润色">
        <Row label="润色模式" effect="立即">
          <select className={selectClass} value={cfg.polish_mode as number} onChange={(e) => setVal("polish_mode", parseInt(e.target.value))}>
            <option value={0}>关闭</option><option value={1}>仅最终润色</option><option value={2}>中间 + 最终</option>
          </select>
        </Row>
        <Row label="润色提示词" effect="立即" hint="选择不同风格 prompt">
          <select className={selectClass} value={active_prompt_id} onChange={(e) => setActivePrompt(parseInt(e.target.value))}>
            {prompts.map((p) => <option key={p.id} value={p.id}>{p.title}{p.is_system ? "（内置）" : ""}</option>)}
          </select>
        </Row>
        <Row label="润色间隔" effect="下次录音">
          <select className={selectClass} value={cfg.polish_min_interval as number} onChange={(e) => setVal("polish_min_interval", parseFloat(e.target.value))}>
            <option value={0}>仅最后</option>
            {[3, 4, 5, 6, 7, 8].map((v) => <option key={v} value={v}>每 {v} 秒</option>)}
          </select>
        </Row>
        <Row label="润色停顿阈值" effect="下次录音" hint="超过此值触发中间润色">
          <select className={selectClass} value={cfg.pause_polish_threshold_ms as number} onChange={(e) => setVal("pause_polish_threshold_ms", parseFloat(e.target.value))}>
            {[600, 700, 800, 900, 1000].map((v) => <option key={v} value={v}>{v}ms</option>)}
          </select>
        </Row>
      </Card>

      <Card icon={ClipboardList} title="剪贴板">
        <Row label="最大保留条数" effect="下次启动" hint="不含收藏，超出自动清理">
          <select className={selectClass} value={cfg.clipboard_max_items as number} onChange={(e) => setVal("clipboard_max_items", parseInt(e.target.value))}>
            {[100, 200, 300, 500, 1000].map((v) => <option key={v} value={v}>{v} 条</option>)}
          </select>
        </Row>
        <Row label="自动清理天数" effect="下次启动" hint="超过此天数的非收藏记录自动删除">
          <select className={selectClass} value={cfg.clipboard_max_age_days as number} onChange={(e) => setVal("clipboard_max_age_days", parseInt(e.target.value))}>
            {[1, 3, 7, 15, 30].map((v) => <option key={v} value={v}>{v} 天</option>)}
          </select>
        </Row>
      </Card>
    </div>
  );
}
