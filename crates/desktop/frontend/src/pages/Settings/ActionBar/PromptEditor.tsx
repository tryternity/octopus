import { useState, useEffect, useMemo, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { FileText, Plus } from "lucide-react";
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
  // 初始值由 value 检测：以 @ 开头 = ref，否则 = inline。
  // 切换不同菜单项时由父组件 key={editingId} 重新 mount，mode 自动从 value 重初始化。
  const [mode, setMode] = useState<string>(isReferenceMode(value) ? "ref" : "inline");
  // input 草稿：用户自由输入的内容（可能匹配已有文件，也可能是新文件名）。
  // 与 selectedName（派生自 props value，= 已提交的引用）区分——
  // input 显示草稿，Enter/加号提交或匹配已有文件时才写回 props。
  const [selectedInput, setSelectedInput] = useState(extractFileName(value));

  // 加载 prompt 文件列表（mount 时一次性）
  useEffect(() => {
    invoke<PromptFileInfo[]>("list_prompt_files", { category: "command" })
      .then(setFiles)
      .catch((e) => console.error("list_prompt_files failed:", e));
  }, []);

  // 外部 value 变化时同步草稿（如父组件重置 / 切换菜单项 remount 之外的途径）。
  // 避免外部清空 value 后草稿仍显示旧文件名。
  const selectedName = useMemo(() => extractFileName(value), [value]);
  useEffect(() => {
    setSelectedInput(selectedName);
  }, [selectedName]);
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

  const createNewFile = async (name: string) => {
    const trimmed = name.trim();
    if (!trimmed) return;
    try {
      await invoke("create_prompt_file", { category: "command", name: trimmed });
      // 刷新文件列表
      const updated = await invoke<PromptFileInfo[]>("list_prompt_files", { category: "command" });
      setFiles(updated);
      // 选中新建的文件
      onChange(`@${trimmed}`);
      setSelectedInput(trimmed);
      // 打开编辑器
      invoke("open_file_in_editor", { name: trimmed, category: "command" }).catch(() => {});
    } catch (e) {
      console.error("create_prompt_file failed:", e);
    }
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
          autoCapitalize="off"
          autoCorrect="off"
          spellCheck={false}
        />
      )}

      {/* 引用模式：可编辑下拉 + 加号创建 */}
      {mode === "ref" && (
        <div className="space-y-1.5">
          {/* 可编辑输入框 + datalist（可选已有文件，也可输入新文件名）+ 加号按钮 */}
          {/* input value 绑定草稿 selectedInput（非派生 selectedName），用户可自由输入；
              匹配已有文件或 Enter/加号提交时才写回 props value。 */}
          <div className="flex items-center gap-1.5">
            <div className="relative flex-1">
              <FileText className="pointer-events-none absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground/50" />
              <input
                type="text"
                autoCapitalize="off"
                autoCorrect="off"
                spellCheck={false}
                list="prompt-file-list"
                className="w-full bg-background border border-border rounded-md pl-8 pr-3 py-2 text-sm font-mono outline-none transition-all focus:border-voice/50 focus:ring-2 focus:ring-voice/15"
                placeholder={t("settings.actionBar.promptSelectFile")}
                value={selectedInput}
                onChange={(e) => {
                  const v = e.target.value;
                  setSelectedInput(v);
                  // 匹配已有文件 → 即时选中（写回 props，触发预览/路径更新）
                  if (files.some((f) => f.name === v)) {
                    selectFile(v);
                  }
                }}
                onKeyDown={(e) => {
                  // Enter → 输入非空且不匹配已有文件时，创建新文件（即时提交草稿为新引用）
                  if (e.key === "Enter" && selectedInput.trim() && !files.some((f) => f.name === selectedInput.trim())) {
                    createNewFile(selectedInput.trim());
                  }
                }}
              />
              <datalist id="prompt-file-list">
                {files.map((f) => (
                  <option key={f.name} value={f.name}>{f.fileName}</option>
                ))}
              </datalist>
            </div>
            {/* 加号：输入了非空草稿 → 创建新文件并引用 */}
            <button
              onClick={() => {
                const trimmed = selectedInput.trim();
                if (trimmed && !files.some((f) => f.name === trimmed)) {
                  createNewFile(trimmed);
                }
              }}
              className="shrink-0 rounded p-1.5 text-muted-foreground/60 transition-colors hover:bg-voice/10 hover:text-voice"
              title={t("settings.actionBar.promptNewFile")}
            >
              <Plus className="h-4 w-4" />
            </button>
          </div>

              {/* 文件路径：跟草稿 selectedInput 走（用户当前输入的内容），不跟已提交的 selectedName。
                  草稿非空即显示路径 + 存在性提示（已有=无提示，新文件名=黄字"文件不存在"待创建）。 */}
              {selectedInput && (
                <p className="text-[11px] text-muted-foreground/60">
                  <code className="text-muted-foreground/80">~/.octopus/.sync/prompts/command/{selectedInput}.md</code>
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
        </div>
      )}
    </div>
  );
}
