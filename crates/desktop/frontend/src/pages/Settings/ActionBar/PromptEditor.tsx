import { useState, useEffect, useMemo, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ChevronDown, FileText, Inbox } from "lucide-react";
import { useT } from "@/lib/i18n";
import { Segmented } from "@/components/ui/tabs";

/** list_prompt_files 返回的 prompt 文件信息 */
interface PromptFileInfo {
  name: string;
  fileName: string;
  preview: string;
}

interface PromptEditorProps {
  /** action_data 当前值（内联 prompt 或 @引用） */
  value: string;
  onChange: (v: string) => void;
  /** textarea 的 placeholder（来自 TYPE_META） */
  placeholder?: string;
}

/** 检测当前是否为引用模式（value trim 后以 @ 开头） */
function isReferenceMode(value: string): boolean {
  return value.trim().startsWith("@");
}

/** 从 @引用值提取文件名（@tolaria → tolaria） */
function extractFileName(value: string): string {
  return value.trim().replace(/^@/, "").replace(/\.md$/, "").trim();
}

/**
 * Prompt 编辑器：支持「内联编辑」和「引用文件」两种模式。
 *
 * - 内联模式：textarea 直接写 prompt（原有行为）
 * - 引用模式：下拉选 ~/.octopus/.sync/prompts/command/*.md 文件，action_data 存 @文件名
 *
 * 模式自动检测：value 以 @ 开头 = 引用模式，否则 = 内联模式。
 * 切换模式时保留原内容（内联→引用不清空文本，引用→内联展开 @ 为普通文本）。
 */
export default function PromptEditor({ value, onChange, placeholder }: PromptEditorProps) {
  const t = useT();
  const [files, setFiles] = useState<PromptFileInfo[]>([]);
  const [showPreview, setShowPreview] = useState(false);
  const hideTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const handleHoverEnter = () => {
    if (hideTimer.current) { clearTimeout(hideTimer.current); hideTimer.current = null; }
    setShowPreview(true);
  };
  const handleHoverLeave = () => {
    hideTimer.current = setTimeout(() => setShowPreview(false), 1000);
  };
  // 独立 mode state——切换时不改 value，避免「切不回」的死锁。
  // 初始值由 value 检测：以 @ 开头 = ref，否则 inline。
  // 切换不同菜单项时由父组件 key={editingId} 重新 mount，mode 自动从 value 重初始化。
  const [mode, setMode] = useState<string>(isReferenceMode(value) ? "ref" : "inline");

  // 加载 prompt 文件列表（mount 时一次性）
  useEffect(() => {
    invoke<PromptFileInfo[]>("list_prompt_files", { category: "command" })
      .then(setFiles)
      .catch((e) => console.error("list_prompt_files failed:", e));
  }, []);

  const selectedName = useMemo(() => extractFileName(value), [value]);
  const selectedFile = useMemo(
    () => files.find((f) => f.name === selectedName),
    [files, selectedName],
  );

  const switchMode = (newMode: string) => {
    if (newMode === mode) return;
    // 切换模式只改 mode state，不碰 value——避免触发父组件重渲染导致界面冻结。
    // 引用模式下 value 由用户选文件时写（selectFile）；内联模式下 value 由 textarea 编辑写。
    setMode(newMode);
  };

  const selectFile = (name: string) => {
    onChange(`@${name}`);
  };

  return (
    <div className="space-y-1.5">
      {/* 模式切换 + 标签 */}
      <div className="flex items-center justify-between">
        <label className="block text-[11px] font-medium uppercase tracking-wide text-muted-foreground/80">
          {t("settings.actionBar.contentLabel")}
        </label>
        <Segmented
          items={[
            { key: "inline", label: t("settings.actionBar.promptInline") },
            { key: "ref", label: t("settings.actionBar.promptRef") },
          ]}
          active={mode}
          onChange={switchMode}
          className="text-[11px]"
        />
      </div>

      {/* 内联模式：textarea */}
      {mode === "inline" && (
        <textarea
          className="w-full min-h-[190px] resize-y bg-background border border-border rounded-md px-3 py-2 font-mono text-xs leading-relaxed outline-none transition-all focus:border-voice/50 focus:ring-2 focus:ring-voice/15"
          placeholder={placeholder}
          value={value}
          onChange={(e) => onChange(e.target.value)}
        />
      )}

      {/* 引用模式：文件选择 + 预览 */}
      {mode === "ref" && (
        <div className="space-y-1.5">
          {files.length === 0 ? (
            <div className="flex flex-col items-center gap-1.5 rounded-md border border-dashed border-border py-6 text-center">
              <Inbox className="h-5 w-5 text-muted-foreground/40" />
              <p className="text-xs text-muted-foreground/60">
                {t("settings.actionBar.promptDirEmpty")}
              </p>
              <code className="text-[10px] text-muted-foreground/40">~/.octopus/.sync/prompts/command/*.md</code>
            </div>
          ) : (
            <>
              {/* 文件下拉 */}
              <div className="relative">
                <FileText className="pointer-events-none absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground/50" />
                <select
                  className="w-full appearance-none bg-background border border-border rounded-md pl-8 pr-8 py-2 text-sm outline-none transition-all focus:border-voice/50 focus:ring-2 focus:ring-voice/15"
                  value={selectedName}
                  onChange={(e) => selectFile(e.target.value)}
                >
                  <option value="">{t("settings.actionBar.promptSelectFile")}</option>
                  {files.map((f) => (
                    <option key={f.name} value={f.name}>
                      {f.fileName}
                    </option>
                  ))}
                </select>
                <ChevronDown className="pointer-events-none absolute right-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground/50" />
              </div>

              {/* 文件路径 */}
              {selectedName && (
                <p className="text-[11px] text-muted-foreground/60">
                  <code className="text-muted-foreground/80">~/.octopus/.sync/prompts/command/{selectedName}.md</code>
                  {!selectedFile && (
                    <span className="ml-1.5 text-amber-600 dark:text-amber-400">
                      {t("settings.actionBar.promptFileMissing")}
                    </span>
                  )}
                </p>
              )}

              {/* hover 预览浮层 + 查看更多 */}
              {selectedFile && (
                <div
                  className="relative w-fit"
                  onMouseEnter={handleHoverEnter}
                  onMouseLeave={handleHoverLeave}
                >
                  <button
                    onClick={() => invoke("open_file_in_editor", { name: selectedFile.name, category: "command" })}
                    className="flex items-center gap-1 text-[11px] text-muted-foreground/70 transition-colors hover:text-foreground"
                  >
                    <FileText className="h-3 w-3" />
                    {t("settings.actionBar.promptPreview")}
                  </button>
                  {showPreview && (
                    <div className="absolute bottom-full left-0 z-50 mb-1 w-96 max-w-[calc(100vw-2rem)] rounded-md border border-input bg-background p-2.5 shadow-lg">
                      <pre className="max-h-64 overflow-y-auto text-[11px] leading-relaxed text-muted-foreground whitespace-pre-wrap">{selectedFile.preview}</pre>
                      {selectedFile.preview.length >= 500 && (
                        <button
                          onClick={() => invoke("open_file_in_editor", { name: selectedFile.name, category: "command" })}
                          className="mt-1.5 flex w-full items-center justify-center gap-1 rounded border-t border-border pt-1.5 text-[11px] text-voice/80 transition-colors hover:text-voice"
                        >
                          {t("settings.actionBar.promptViewMore")}
                        </button>
                      )}
                    </div>
                  )}
                </div>
              )}
            </>
          )}
        </div>
      )}
    </div>
  );
}
