import { useMemo, useState } from "react";
import { marked } from "marked";
import { Eye, Pencil } from "lucide-react";
import { cn } from "@/lib/utils";

interface MarkdownEditorProps {
  value: string;
  onChange: (v: string) => void;
  placeholder?: string;
}

/** Markdown 编辑器：编辑（textarea，等宽字体）+ 预览（marked 渲染）切换。
 * body 存 md 源码（后端 type=markdown 直存 content_text），预览端实时渲染。 */
export default function MarkdownEditor({
  value,
  onChange,
  placeholder,
}: MarkdownEditorProps) {
  const [mode, setMode] = useState<"edit" | "preview">("edit");
  const html = useMemo(() => {
    if (mode !== "preview") return "";
    // marked 同步解析（async:false 保证返回 string，非 Promise）
    return marked.parse(value || "", { async: false }) as string;
  }, [mode, value]);

  return (
    <div className="flex flex-col h-full">
      {/* 编辑/预览切换 */}
      <div className="flex items-center gap-1 px-2 py-1 border-b border-border">
        <button
          className={cn(
            "p-1 rounded",
            mode === "edit"
              ? "text-foreground bg-accent"
              : "text-muted-foreground hover:bg-accent",
          )}
          title="编辑"
          onClick={() => setMode("edit")}
        >
          <Pencil className="w-4 h-4" />
        </button>
        <button
          className={cn(
            "p-1 rounded",
            mode === "preview"
              ? "text-foreground bg-accent"
              : "text-muted-foreground hover:bg-accent",
          )}
          title="预览"
          onClick={() => setMode("preview")}
        >
          <Eye className="w-4 h-4" />
        </button>
      </div>
      {mode === "edit" ? (
        <textarea
          className="flex-1 w-full resize-none bg-transparent focus:outline-none px-4 py-3 text-sm font-mono leading-relaxed"
          placeholder={placeholder ?? "输入 Markdown…"}
          value={value}
          onChange={(e) => onChange(e.target.value)}
        />
      ) : (
        <div className="flex-1 overflow-y-auto px-4 py-3">
          <div
            className="prose prose-sm max-w-none [&_img]:max-w-full"
            dangerouslySetInnerHTML={{ __html: html }}
          />
        </div>
      )}
    </div>
  );
}
