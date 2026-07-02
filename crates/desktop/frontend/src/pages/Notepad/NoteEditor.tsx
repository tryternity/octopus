import {
  useState,
  useEffect,
  useCallback,
  useRef,
  type ComponentType,
} from "react";
import { cn } from "@/lib/utils";
import {
  Bold,
  Italic,
  Heading1,
  Heading2,
  List,
  ListOrdered,
  Quote,
  Code,
  Minus,
  Link as LinkIcon,
  Undo,
  Redo,
  Download,
  Upload,
  Star,
  Pin,
  type LucideProps,
} from "lucide-react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import type { Note, NoteType } from "@/types/note";
import {
  getNote,
  updateNote,
  toggleNotePinned,
  toggleNoteFavorite,
  exportNote,
  importNoteFromFile,
} from "@/lib/notepad";
import { useNoteEditor, getMarkdownFromEditor } from "./extensions";
import { EditorContent } from "@tiptap/react";
import MarkdownEditor from "./MarkdownEditor";

export default function NoteEditor({ noteId }: { noteId: number | null }) {
  const [note, setNote] = useState<Note | null>(null);
  const [title, setTitle] = useState("");
  // text/markdown 的 body（html 用 TipTap editor，不经此 state）
  const [textBody, setTextBody] = useState("");
  const [toast, setToast] = useState<string | null>(null);
  // 链接 URL 输入：WKWebView 默认禁用 window.prompt()，改用内联输入框
  const [linkInput, setLinkInput] = useState<string | null>(null);
  const saveTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const currentId = useRef<number | null>(null);

  const noteType: NoteType = note?.note_type ?? "html";

  // 加载笔记
  useEffect(() => {
    if (noteId == null) {
      setNote(null);
      return;
    }
    currentId.current = noteId;
    getNote(noteId).then((n) => {
      if (currentId.current !== noteId) return; // 切换防竞态
      setNote(n ?? null);
      setTitle(n?.title ?? "");
      setTextBody(n?.content_text ?? ""); // text/markdown body；html 不用
    });
  }, [noteId]);

  const onHtmlUpdate = useCallback(
    (html: string) => {
      if (noteType !== "html") return; // 非 html 笔记不经 TipTap 保存
      const id = currentId.current;
      if (id == null) return;
      // debounce 800ms 自动保存
      if (saveTimer.current) clearTimeout(saveTimer.current);
      saveTimer.current = setTimeout(
        () => updateNote(id, title, html, noteType).catch(console.error),
        800,
      );
    },
    [noteType, title],
  );

  const editor = useNoteEditor(note?.content_html ?? "", onHtmlUpdate);

  // 标题 / text body 变更 debounce 保存（按 type）
  useEffect(() => {
    if (noteId == null) return;
    const t = setTimeout(() => {
      if (noteType === "html") {
        if (editor && !editor.isDestroyed) {
          updateNote(noteId, title, editor.getHTML(), noteType).catch(console.error);
        }
      } else {
        updateNote(noteId, title, textBody, noteType).catch(console.error);
      }
    }, 800);
    return () => clearTimeout(t);
  }, [title, textBody, noteId, noteType, editor]);

  const flash = (msg: string) => {
    setToast(msg);
    setTimeout(() => setToast(null), 2000);
  };

  if (!note) {
    return (
      <div className="flex-1 flex items-center justify-center text-muted-foreground text-sm">
        选择或新建一条笔记
      </div>
    );
  }

  const exec = (fn: () => void) => () => {
    if (editor && !editor.isDestroyed) fn();
  };

  // 导出：按 noteType 决定格式（html→md 富文本序列化，text→txt，markdown→md 源码）
  const doExport = async () => {
    const stem = (
      title ||
      (noteType === "html" ? note.content_text.slice(0, 20) : textBody.slice(0, 20)) ||
      "note"
    ).replace(/\s+/g, "_");

    if (noteType === "html") {
      if (!editor) return;
      const content = getMarkdownFromEditor(editor) || editor.getText();
      const path = await exportNote(stem, "md", content);
      flash("已导出: " + path);
      return;
    }
    if (noteType === "markdown") {
      const path = await exportNote(stem, "md", textBody);
      flash("已导出: " + path);
      return;
    }
    // text
    const path = await exportNote(stem, "txt", textBody);
    flash("已导出: " + path);
  };

  // 导入：仅 html（依赖 TipTap 解析 md）
  const doImport = async () => {
    const selected = await openDialog({
      filters: [{ name: "Markdown", extensions: ["md", "txt"] }],
    });
    if (!selected) return;
    const md = await importNoteFromFile(selected);
    // tiptap-markdown 解析 md → setContent（emitUpdate:false 避免立即触发自动保存）
    editor?.commands.setContent(md, { emitUpdate: false });
    flash("已导入");
  };

  const tools: { icon: ComponentType<LucideProps>; title: string; onClick: () => void }[] = [
    {
      icon: Bold,
      title: "粗体",
      onClick: exec(() => editor?.chain().focus().toggleBold().run()),
    },
    {
      icon: Italic,
      title: "斜体",
      onClick: exec(() => editor?.chain().focus().toggleItalic().run()),
    },
    {
      icon: Heading1,
      title: "标题1",
      onClick: exec(() =>
        editor?.chain().focus().toggleHeading({ level: 1 }).run(),
      ),
    },
    {
      icon: Heading2,
      title: "标题2",
      onClick: exec(() =>
        editor?.chain().focus().toggleHeading({ level: 2 }).run(),
      ),
    },
    {
      icon: List,
      title: "无序列表",
      onClick: exec(() => editor?.chain().focus().toggleBulletList().run()),
    },
    {
      icon: ListOrdered,
      title: "有序列表",
      onClick: exec(() => editor?.chain().focus().toggleOrderedList().run()),
    },
    {
      icon: Quote,
      title: "引用",
      onClick: exec(() => editor?.chain().focus().toggleBlockquote().run()),
    },
    {
      icon: Code,
      title: "代码块",
      onClick: exec(() => editor?.chain().focus().toggleCodeBlock().run()),
    },
    {
      icon: Minus,
      title: "分割线",
      onClick: exec(() => editor?.chain().focus().setHorizontalRule().run()),
    },
    {
      icon: LinkIcon,
      title: "链接",
      onClick: exec(() => {
        // WKWebView 禁用 window.prompt()，改弹内联输入框（预填当前 href）
        const prev = editor?.getAttributes("link").href;
        setLinkInput(typeof prev === "string" ? prev : "");
      }),
    },
    {
      icon: Undo,
      title: "撤销",
      onClick: exec(() => editor?.chain().focus().undo().run()),
    },
    {
      icon: Redo,
      title: "重做",
      onClick: exec(() => editor?.chain().focus().redo().run()),
    },
  ];

  return (
    <div className="flex-1 flex flex-col bg-background relative">
      {/* 顶部栏：html=富文本工具；text/markdown=类型标签 */}
      <div className="flex items-center gap-0.5 px-2 py-1 border-b border-border flex-wrap">
        {noteType === "html" ? (
          tools.map(({ icon: Icon, title, onClick }, i) => (
            <button
              key={i}
              className="p-1 rounded hover:bg-accent text-muted-foreground hover:text-foreground"
              title={title}
              onClick={onClick}
            >
              <Icon className="w-4 h-4" />
            </button>
          ))
        ) : (
          <span className="px-2 py-0.5 text-[11px] rounded bg-muted text-muted-foreground">
            {noteType === "markdown" ? "Markdown" : "纯文本"}
          </span>
        )}
        <div className="ml-auto flex items-center gap-0.5">
          {noteType === "html" && (
            <button
              className="p-1 rounded hover:bg-accent text-muted-foreground"
              title="导入 md"
              onClick={doImport}
            >
              <Upload className="w-4 h-4" />
            </button>
          )}
          <button
            className="p-1 rounded hover:bg-accent text-muted-foreground"
            title="导出"
            onClick={doExport}
          >
            <Download className="w-4 h-4" />
          </button>
          <button
            className={cn(
              "p-1 rounded",
              note.is_favorite
                ? "text-amber-400"
                : "text-muted-foreground hover:bg-accent",
            )}
            title="收藏"
            onClick={async () => {
              await toggleNoteFavorite(note.id);
              const n = await getNote(note.id);
              if (n) setNote(n);
            }}
          >
            <Star className={cn("w-4 h-4", note.is_favorite && "fill-amber-400")} />
          </button>
          <button
            className={cn(
              "p-1 rounded",
              note.is_pinned
                ? "text-foreground"
                : "text-muted-foreground hover:bg-accent",
            )}
            title="置顶"
            onClick={async () => {
              await toggleNotePinned(note.id);
              const n = await getNote(note.id);
              if (n) setNote(n);
            }}
          >
            <Pin className={cn("w-4 h-4", note.is_pinned && "fill-foreground")} />
          </button>
        </div>
      </div>
      {/* 标题 */}
      <input
        className="px-4 pt-3 pb-1 text-lg font-semibold bg-transparent focus:outline-none"
        placeholder="无标题"
        value={title}
        onChange={(e) => setTitle(e.target.value)}
      />
      {/* 编辑区：按 type 分发 */}
      {noteType === "html" && (
        <div className="flex-1 overflow-y-auto px-4 pb-4">
          <div className="prose prose-sm max-w-none [&_img]:max-w-full">
            <EditorContent editor={editor} />
          </div>
        </div>
      )}
      {noteType === "text" && (
        <textarea
          className="flex-1 mx-4 mb-4 mt-1 resize-none bg-transparent focus:outline-none text-sm leading-relaxed font-mono"
          placeholder="输入纯文本…"
          value={textBody}
          onChange={(e) => setTextBody(e.target.value)}
        />
      )}
      {noteType === "markdown" && (
        <div className="flex-1 mx-4 mb-4 mt-1">
          <MarkdownEditor value={textBody} onChange={setTextBody} />
        </div>
      )}
      {linkInput !== null && (
        <div className="absolute top-12 right-3 z-20 flex items-center gap-1 p-2 rounded-md border border-border bg-background shadow-md">
          <input
            autoFocus
            className="px-2 py-1 text-xs border border-border rounded bg-background w-48 focus:outline-none"
            placeholder="https:// 链接地址"
            value={linkInput}
            onChange={(e) => setLinkInput(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                const url = linkInput.trim();
                if (url) editor?.chain().focus().setLink({ href: url }).run();
                setLinkInput(null);
              } else if (e.key === "Escape") {
                setLinkInput(null);
              }
            }}
          />
          <button
            className="px-2 py-1 text-xs rounded bg-primary text-primary-foreground"
            onClick={() => {
              const url = linkInput.trim();
              if (url) editor?.chain().focus().setLink({ href: url }).run();
              setLinkInput(null);
            }}
          >
            确定
          </button>
          {editor?.isActive("link") && (
            <button
              className="px-2 py-1 text-xs rounded hover:bg-accent"
              onClick={() => {
                editor?.chain().focus().unsetLink().run();
                setLinkInput(null);
              }}
            >
              移除
            </button>
          )}
        </div>
      )}
      {toast && (
        <div className="absolute bottom-3 right-3 px-3 py-1.5 rounded bg-foreground text-background text-xs">
          {toast}
        </div>
      )}
    </div>
  );
}
