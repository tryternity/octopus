/**
 * RecordConfig —— 录屏配置浮窗（spec §8.1）。
 *
 * 按 Cmd+Shift+R 由后端 record_window::show_record_window 显示。
 * 用户选源（display/window/area）+ 音频开关，点「开始录制」调 record_start。
 *
 * 视觉：沿用 octopus 既有浮窗规范（password-generator/overlay 风格）——
 * shadcn-style token + lucide-react + text-xs/sm 字号阶梯 + rounded-md。
 * 透明浮窗（transparent:true），html/body 不设背景色（由内层卡片提供不透明层）。
 *
 * 区域录制（Task C 后补）：tab=area 时显示「选择区域」按钮，点击后浮窗全屏化
 * 让用户拖框，完成后恢复小窗 + 显示选区摘要。
 */
import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Monitor, AppWindow, Square, Circle, X, Volume2, Mic, Check, ChevronDown } from "lucide-react";
import { useT } from "@/lib/i18n";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import { listen } from "@tauri-apps/api/event";

// ── 后端类型镜像 ────────────────────────────────────────────────

interface DisplayInfo {
  id: number;
  name: string;
  width: number;
  height: number;
  is_primary: boolean;
}

interface WindowInfo {
  id: number;
  title: string | null;
  app_name: string | null;
  width: number;
  height: number;
}

type Tab = "display" | "window" | "area";

// ── 默认视频/音频配置（从 DB record_* seed 派生，与后端 build_default_config 对齐）────

// ── 主组件 ──────────────────────────────────────────────────────

export default function RecordConfig() {
  const t = useT();
  const [tab, setTab] = useState<Tab>("display");
  const [displays, setDisplays] = useState<DisplayInfo[]>([]);
  const [windows, setWindows] = useState<WindowInfo[]>([]);
  const [selectedDisplayId, setSelectedDisplayId] = useState<number | null>(null);
  const [selectedWindowId, setSelectedWindowId] = useState<number | null>(null);
  // area 选区（picker 拖框后由后端 emit record-area://selected 推回）
  const [areaSelection, setAreaSelection] = useState<{
    display_id: number;
    x: number;
    y: number;
    width: number;
    height: number;
  } | null>(null);

  // 监听 picker 选区完成事件（picker 关闭后浮窗重新 show，payload 是物理像素）
  useEffect(() => {
    const unlisten = listen<{
      display_id: number;
      x: number;
      y: number;
      width: number;
      height: number;
    }>("record-area://selected", (event) => {
      setAreaSelection(event.payload);
      // 选区回来后切到 area tab（用户可能切到别的 tab 调起 picker）
      setTab("area");
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);
  const [systemAudio, setSystemAudio] = useState(true);
  const [microphone, setMicrophone] = useState(false);
  const [starting, setStarting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // ── 高级（编码参数，默认收起）──
  // 不持久化到 DB（避免改 seed 影响其他路径），只在当前 RecordConfig session 用。
  const [showAdvanced, setShowAdvanced] = useState(false);
  const [fps, setFps] = useState<15 | 30 | 60>(30);
  const [codec, setCodec] = useState<"h264" | "hevc">("h264");
  const [hideCursor, setHideCursor] = useState(false);

  // ── 拉取源列表（浮窗 show 时 + tab 切换时）──────────────────────
  const refreshSources = useCallback(async () => {
    try {
      if (tab === "display") {
        const list = await invoke<DisplayInfo[]>("list_record_displays");
        setDisplays(list);
        // 默认选主屏
        if (selectedDisplayId === null) {
          const primary = list.find((d) => d.is_primary) ?? list[0];
          if (primary) setSelectedDisplayId(primary.id);
        }
      } else if (tab === "window") {
        const list = await invoke<WindowInfo[]>("list_record_windows");
        setWindows(list);
        if (selectedWindowId === null && list.length > 0) {
          setSelectedWindowId(list[0].id);
        }
      }
    } catch (e) {
      setError(t("recordConfig.loadFailed") + String(e));
    }
  }, [tab, selectedDisplayId, selectedWindowId, t]);

  useEffect(() => {
    refreshSources();
  }, [refreshSources]);

  // ── Esc 取消浮窗 ──────────────────────────────────────────────
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        getCurrentWindow().hide();
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, []);

  // ── 开始录制 ──────────────────────────────────────────────────
  const handleStart = useCallback(async () => {
    setError(null);

    // 组装 source（按 tab）
    let source;
    let videoW: number;
    let videoH: number;
    if (tab === "display") {
      const d = displays.find((x) => x.id === selectedDisplayId);
      if (!d) {
        setError(t("recordConfig.noSource"));
        return;
      }
      source = { type: "display" as const, display_id: d.id };
      videoW = d.width;
      videoH = d.height;
    } else if (tab === "window") {
      const w = windows.find((x) => x.id === selectedWindowId);
      if (!w) {
        setError(t("recordConfig.noSource"));
        return;
      }
      source = { type: "window" as const, window_id: w.id };
      videoW = w.width;
      videoH = w.height;
    } else {
      // area
      if (!areaSelection) {
        setError(t("recordConfig.areaNotSelected"));
        return;
      }
      source = { type: "area" as const, ...areaSelection };
      videoW = areaSelection.width;
      videoH = areaSelection.height;
    }

    setStarting(true);
    try {
      await invoke("record_start", {
        config: {
          source,
          video: {
            fps,
            width: videoW,
            height: videoH,
            codec,
            bitrate: null, // None = helper 自动按分辨率×fps 算
            hide_system_cursor: hideCursor,
          },
          audio: {
            system: { enabled: systemAudio, excludes_current_process: true },
            microphone: { enabled: microphone, device_id: null, device_name: null },
          },
        },
      });
      // 成功——隐藏浮窗
      await getCurrentWindow().hide();
    } catch (e) {
      setError(t("recordConfig.startFailed") + String(e));
    } finally {
      setStarting(false);
    }
  }, [
    tab,
    displays,
    windows,
    selectedDisplayId,
    selectedWindowId,
    areaSelection,
    systemAudio,
    microphone,
    fps,
    codec,
    hideCursor,
    t,
  ]);

  // ── 取消（关闭按钮）──────────────────────────────────────────
  const handleCancel = useCallback(async () => {
    await getCurrentWindow().hide();
  }, []);

  return (
    <div className="w-full h-full flex items-center justify-center p-2">
      {/* 卡片层（透明浮窗内提供不透明背景）*/}
      <div className="w-full rounded-lg border border-border bg-background shadow-lg overflow-hidden">
        {/* ── 标题栏（可拖动）────────────────────────────────── */}
        <div
          data-tauri-drag-region="deep"
          className="flex items-center justify-between px-3 py-2 border-b border-border bg-muted/50"
        >
          <span className="text-xs font-semibold text-foreground">
            {t("recordConfig.title")}
          </span>
          <button
            onClick={handleCancel}
            className="text-muted-foreground hover:text-foreground transition-colors"
            aria-label={t("recordConfig.cancel")}
          >
            <X className="w-3.5 h-3.5" />
          </button>
        </div>

        {/* ── Tab 切换 ──────────────────────────────────────── */}
        <div className="flex gap-1 px-3 pt-3">
          {(
            [
              { id: "display", icon: Monitor, label: t("recordConfig.tabDisplay") },
              { id: "window", icon: AppWindow, label: t("recordConfig.tabWindow") },
              { id: "area", icon: Square, label: t("recordConfig.tabArea") },
            ] as const
          ).map(({ id, icon: Icon, label }) => (
            <button
              key={id}
              onClick={() => setTab(id)}
              className={cn(
                "flex items-center gap-1 px-2.5 py-1.5 rounded-md text-[11px] font-medium transition-colors",
                tab === id
                  ? "bg-primary text-primary-foreground"
                  : "bg-muted text-muted-foreground hover:text-foreground",
              )}
            >
              <Icon className="w-3 h-3" />
              {label}
            </button>
          ))}
        </div>

        {/* ── 源列表（display / window）────────────────────── */}
        <div className="px-3 py-3 min-h-[140px] max-h-[200px] overflow-y-auto thin-scrollbar">
          {tab === "display" && (
            <DisplayList
              displays={displays}
              selectedId={selectedDisplayId}
              onSelect={setSelectedDisplayId}
            />
          )}
          {tab === "window" && (
            <WindowList
              windows={windows}
              selectedId={selectedWindowId}
              onSelect={setSelectedWindowId}
            />
          )}
          {tab === "area" && (
            <AreaPanel
              selection={areaSelection}
              onChange={setAreaSelection}
              displays={displays}
            />
          )}
        </div>

        {/* ── 音频开关 ─────────────────────────────────────── */}
        <div className="px-3 py-2 border-t border-border flex items-center gap-4">
          <ToggleRow
            icon={Volume2}
            label={t("recordConfig.systemAudio")}
            checked={systemAudio}
            onChange={setSystemAudio}
          />
          <ToggleRow
            icon={Mic}
            label={t("recordConfig.microphone")}
            checked={microphone}
            onChange={setMicrophone}
          />
        </div>

        {/* ── 高级（编码参数，默认收起）────────────────────────── */}
        <div className="px-3 py-1.5 border-t border-border">
          <button
            className="flex items-center gap-1 text-[11px] text-muted-foreground hover:text-foreground transition-colors"
            onClick={() => setShowAdvanced(!showAdvanced)}
          >
            <ChevronDown className={cn("w-3 h-3 transition-transform", !showAdvanced && "-rotate-90")} />
            {t("recordConfig.advanced")}
          </button>
          {showAdvanced && (
            <div className="mt-1.5 space-y-1.5">
              {/* FPS */}
              <div className="flex items-center justify-between">
                <span className="text-[11px] text-muted-foreground">{t("recordConfig.fps")}</span>
                <div className="flex gap-1">
                  {([15, 30, 60] as const).map((f) => (
                    <button
                      key={f}
                      className={cn(
                        "px-2 py-0.5 rounded text-[10px] transition-colors",
                        fps === f
                          ? "bg-primary text-primary-foreground"
                          : "bg-muted text-muted-foreground hover:text-foreground",
                      )}
                      onClick={() => setFps(f)}
                    >
                      {f}
                    </button>
                  ))}
                </div>
              </div>
              {/* Codec */}
              <div className="flex items-center justify-between">
                <span className="text-[11px] text-muted-foreground">{t("recordConfig.codec")}</span>
                <div className="flex gap-1">
                  {(["h264", "hevc"] as const).map((c) => (
                    <button
                      key={c}
                      className={cn(
                        "px-2 py-0.5 rounded text-[10px] uppercase transition-colors",
                        codec === c
                          ? "bg-primary text-primary-foreground"
                          : "bg-muted text-muted-foreground hover:text-foreground",
                      )}
                      onClick={() => setCodec(c)}
                    >
                      {c}
                    </button>
                  ))}
                </div>
              </div>
              {/* Hide cursor */}
              <ToggleRow
                icon={X}
                label={t("recordConfig.hideCursor")}
                checked={hideCursor}
                onChange={setHideCursor}
              />
            </div>
          )}
        </div>

        {/* ── 错误提示 ─────────────────────────────────────── */}
        {error && (
          <div className="mx-3 mb-2 px-2 py-1.5 rounded-md bg-destructive/10 text-destructive text-[10px]">
            {error}
          </div>
        )}

        {/* ── 底部按钮 ─────────────────────────────────────── */}
        <div className="px-3 py-3 border-t border-border flex gap-2">
          <Button
            variant="primary"
            size="sm"
            onClick={handleStart}
            disabled={starting}
            className="flex-1 gap-1.5"
          >
            <Circle className="w-2.5 h-2.5 fill-current" />
            {starting ? t("recordConfig.starting") : t("recordConfig.startBtn")}
          </Button>
          <Button variant="outline" size="sm" onClick={handleCancel}>
            {t("recordConfig.cancel")}
          </Button>
        </div>
      </div>
    </div>
  );
}

// ── 子组件：display 列表 ────────────────────────────────────────

function DisplayList({
  displays,
  selectedId,
  onSelect,
}: {
  displays: DisplayInfo[];
  selectedId: number | null;
  onSelect: (id: number) => void;
}) {
  if (displays.length === 0) {
    return <EmptyHint text="recordConfig.noDisplays" />;
  }
  return (
    <div className="space-y-1">
      {displays.map((d) => (
        <button
          key={d.id}
          onClick={() => onSelect(d.id)}
          className={cn(
            "w-full flex items-center gap-2 px-2.5 py-2 rounded-md border text-left transition-colors",
            selectedId === d.id
              ? "border-primary bg-primary/5"
              : "border-border hover:bg-muted",
          )}
        >
          <Monitor className="w-3.5 h-3.5 text-muted-foreground flex-shrink-0" />
          <div className="flex-1 min-w-0">
            <div className="text-xs text-foreground truncate">
              {d.name || `Display ${d.id}`}
              {d.is_primary && (
                <span className="ml-1 text-[10px] text-muted-foreground">
                  （主屏）
                </span>
              )}
            </div>
            <div className="text-[10px] text-muted-foreground">
              {d.width}×{d.height}
            </div>
          </div>
          {selectedId === d.id && (
            <Check className="w-4 h-4 text-emerald-500 flex-shrink-0" strokeWidth={3} />
          )}
        </button>
      ))}
    </div>
  );
}

// ── 子组件：window 列表 ────────────────────────────────────────

function WindowList({
  windows,
  selectedId,
  onSelect,
}: {
  windows: WindowInfo[];
  selectedId: number | null;
  onSelect: (id: number) => void;
}) {
  if (windows.length === 0) {
    return <EmptyHint text="recordConfig.noWindows" />;
  }
  return (
    <div className="space-y-1">
      {windows.map((w) => (
        <button
          key={w.id}
          onClick={() => onSelect(w.id)}
          className={cn(
            "w-full flex items-center gap-2 px-2.5 py-2 rounded-md border text-left transition-colors",
            selectedId === w.id
              ? "border-primary bg-primary/5"
              : "border-border hover:bg-muted",
          )}
        >
          <AppWindow className="w-3.5 h-3.5 text-muted-foreground flex-shrink-0" />
          <div className="flex-1 min-w-0">
            <div className="text-xs text-foreground truncate">
              {w.title || w.app_name || `Window ${w.id}`}
            </div>
            <div className="text-[10px] text-muted-foreground truncate">
              {w.app_name ? `${w.app_name} · ` : ""}
              {w.width}×{w.height}
            </div>
          </div>
          {selectedId === w.id && (
            <Check className="w-4 h-4 text-emerald-500 flex-shrink-0" strokeWidth={3} />
          )}
        </button>
      ))}
    </div>
  );
}

// ── 子组件：area 选区（拖框选区域，调起 picker）──────────────────

function AreaPanel({
  selection,
  onChange,
  displays,
}: {
  selection: { display_id: number; x: number; y: number; width: number; height: number } | null;
  onChange: (s: { display_id: number; x: number; y: number; width: number; height: number } | null) => void;
  displays: DisplayInfo[];
}) {
  const t = useT();

  // 查选区所在显示器的名字（摘要显示用）
  const displayName = selection
    ? displays.find((d) => d.id === selection.display_id)?.name || `Display ${selection.display_id}`
    : "";

  const handlePick = async () => {
    try {
      await invoke("start_record_area_picker");
    } catch (e) {
      console.error("[record-config] start picker failed:", e);
    }
  };

  if (!selection) {
    // 无选区：显示「选择区域」按钮
    return (
      <div className="flex flex-col items-center justify-center py-8 gap-3">
        <Square className="w-8 h-8 text-muted-foreground" />
        <Button variant="outline" size="sm" onClick={handlePick} className="gap-1.5">
          <Square className="w-3 h-3" />
          {t("recordConfig.areaPick")}
        </Button>
        <p className="text-[10px] text-muted-foreground text-center max-w-[240px]">
          {t("recordConfig.areaPlaceholder")}
        </p>
      </div>
    );
  }

  // 有选区：显示摘要 + 重新选择 / 清除
  return (
    <div className="flex flex-col gap-2 py-2">
      <div className="flex items-center gap-2 px-2.5 py-2 rounded-md border border-primary bg-primary/5">
        <Square className="w-3.5 h-3.5 text-primary flex-shrink-0" />
        <div className="flex-1 min-w-0">
          <div className="text-xs text-foreground truncate">{displayName}</div>
          <div className="text-[10px] text-muted-foreground">
            {selection.width}×{selection.height}
            <span className="ml-1">({t("recordConfig.areaSelected")})</span>
          </div>
        </div>
      </div>
      <div className="flex gap-2">
        <Button variant="outline" size="sm" onClick={handlePick} className="flex-1 text-[10px]">
          {t("recordConfig.areaReselect")}
        </Button>
        <Button variant="ghost" size="sm" onClick={() => onChange(null)} className="text-[10px]">
          {t("recordConfig.areaClear")}
        </Button>
      </div>
    </div>
  );
}

// ── 通用：toggle 行 ─────────────────────────────────────────────

function ToggleRow({
  icon: Icon,
  label,
  checked,
  onChange,
}: {
  icon: React.ComponentType<{ className?: string }>;
  label: string;
  checked: boolean;
  onChange: (v: boolean) => void;
}) {
  return (
    <button
      onClick={() => onChange(!checked)}
      className={cn(
        "flex items-center gap-1.5 text-[11px] transition-colors",
        checked ? "text-foreground" : "text-muted-foreground",
      )}
    >
      <Icon className={cn("w-3.5 h-3.5", checked && "text-primary")} />
      <span>{label}</span>
      <span
        className={cn(
          "ml-1 w-7 h-3.5 rounded-full relative transition-colors",
          checked ? "bg-primary" : "bg-muted",
        )}
      >
        <span
          className={cn(
            "absolute top-0.5 w-2.5 h-2.5 rounded-full bg-background transition-all",
            checked ? "left-4" : "left-0.5",
          )}
        />
      </span>
    </button>
  );
}

// ── 通用：空提示 ────────────────────────────────────────────────

function EmptyHint({ text }: { text: string }) {
  const t = useT();
  return (
    <div className="flex items-center justify-center py-6 text-[10px] text-muted-foreground">
      {t(text)}
    </div>
  );
}
