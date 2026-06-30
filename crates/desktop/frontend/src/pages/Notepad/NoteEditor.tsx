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
  Image as ImageIcon,
  Undo,
  Redo,
  Download,
  Upload,
  Star,
  Pin,
  type LucideProps,
} from "lucide-react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import type { Note } from "@/types/note";
import {
  getNote,
  updateNote,
  toggleNotePinned,
  toggleNoteFavorite,
  exportNote,
  importNoteFromFile,
  insertNoteImage,
  getNoteImage,
} from "@/lib/notepad";
import { useNoteEditor, getMarkdownFromEditor } from "./extensions";
import { EditorContent } from "@tiptap/react";

export default function NoteEditor({ noteId }: { noteId: number | null }) {
  const [note, setNote] = useState<Note | null>(null);
  const [title, setTitle] = useState("");
  const [toast, setToast] = useState<string | null>(null);
  const saveTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const currentId = useRef<number | null>(null);

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
    });
  }, [noteId]);

  const doSave = useCallback(
    (html: string) => {
      const id = currentId.current;
      if (id == null) return;
      updateNote(id, title, html).catch(console.error);
    },
    [title],
  );

  const editor = useNoteEditor(note?.content_html ?? "", (html) => {
    // debounce 800ms 自动保存
    if (saveTimer.current) clearTimeout(saveTimer.current);
    saveTimer.current = setTimeout(() => doSave(html), 800);
  });

  // 标题变更也 debounce 保存
  useEffect(() => {
    if (noteId == null) return;
    const t = setTimeout(() => {
      if (editor && !editor.isDestroyed) {
        updateNote(noteId, title, editor.getHTML()).catch(console.error);
      }
    }, 800);
    return () => clearTimeout(t);
  }, [title, noteId, editor]);

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

  const insertImage = async () => {
    const selected = await openDialog({
      filters: [{ name: "图片", extensions: ["png", "jpg", "jpeg", "webp"] }],
    });
    if (!selected) return;
    try {
      const hash = await insertNoteImage(selected);
      editor
        ?.chain()
        .focus()
        .setImage({ src: `note-img:${hash}`, alt: "图片" })
        .run();
    } catch (e) {
      flash("插入图片失败: " + String(e));
    }
  };

  const doExport = async (ext: "md" | "txt" | "html") => {
    if (!editor) return;
    let content: string;
    if (ext === "md") {
      content = getMarkdownFromEditor(editor) || editor.getText();
    } else if (ext === "txt") {
      content = editor.getText();
    } else {
      // html：把 note-img:<hash> 替换为 data URL（自包含）
      content = editor.getHTML();
      const re = /note-img:([a-f0-9]+)/g;
      const hashes = [
        ...new Set([...content.matchAll(re)].map((m) => m[1])),
      ];
      for (const h of hashes) {
        try {
          const dataUrl = await getNoteImage(h);
          content = content.split(`note-img:${h}`).join(dataUrl);
        } catch {
          /* 替换失败保留占位 */
        }
      }
    }
    const stem = (title || note.content_text.slice(0, 20) || "note").replace(
      /\s+/g,
      "_",
    );
    const path = await exportNote(stem, ext, content);
    flash("已导出: " + path);
  };

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
        const url = prompt("链接 URL");
        if (url) editor?.chain().focus().setLink({ href: url }).run();
      }),
    },
    { icon: ImageIcon, title: "图片", onClick: insertImage },
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
      {/* 工具栏 */}
      <div className="flex items-center gap-0.5 px-2 py-1 border-b border-border flex-wrap">
        {tools.map(({ icon: Icon, title, onClick }, i) => (
          <button
            key={i}
            className="p-1 rounded hover:bg-accent text-muted-foreground hover:text-foreground"
            title={title}
            onClick={onClick}
          >
            <Icon className="w-4 h-4" />
          </button>
        ))}
        <div className="ml-auto flex items-center gap-0.5">
          <button
            className="p-1 rounded hover:bg-accent text-muted-foreground"
            title="导入 md"
            onClick={doImport}
          >
            <Upload className="w-4 h-4" />
          </button>
          <button
            className="p-1 rounded hover:bg-accent text-muted-foreground"
            title="导出 md"
            onClick={() => doExport("md")}
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
      {/* 编辑器 */}
      <div className="flex-1 overflow-y-auto px-4 pb-4">
        <div className="prose prose-sm max-w-none [&_img]:max-w-full">
          <EditorContent editor={editor} />
        </div>
      </div>
      {toast && (
        <div className="absolute bottom-3 right-3 px-3 py-1.5 rounded bg-foreground text-background text-xs">
          {toast}
        </div>
      )}
    </div>
  );
}
