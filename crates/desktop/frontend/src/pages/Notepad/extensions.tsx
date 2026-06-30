import {
  useEditor,
  ReactNodeViewRenderer,
  type ReactNodeViewProps,
  type Editor,
} from "@tiptap/react";
import StarterKit from "@tiptap/starter-kit";
import Link from "@tiptap/extension-link";
import Image from "@tiptap/extension-image";
import { Markdown } from "tiptap-markdown";
import { useEffect, useState } from "react";
import { getNoteImage } from "@/lib/notepad";

/**
 * 自定义 Image NodeView：src 形如 `note-img:<hash>` 时，解析 hash → invoke get_note_image
 * 取 WebP data URL → 直接作 <img src>（data URL 无需 blob/revoke）。
 * getHTML 仍输出稳定的 `note-img:<hash>` 协议（不存临时 blob URL），笔记可持久化、跨会话还原。
 */
function NoteImageView({ node }: ReactNodeViewProps) {
  const src: string = (node.attrs.src as string) ?? "";
  const alt: string | null = (node.attrs.alt as string | null) ?? null;
  const [url, setUrl] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    const prefix = "note-img:";
    if (!src || !src.startsWith(prefix)) {
      setUrl(src || null); // 外部 URL 原样用
      return;
    }
    getNoteImage(src.slice(prefix.length))
      .then((dataUrl) => {
        if (!cancelled) setUrl(dataUrl);
      })
      .catch(() => {
        if (!cancelled) setUrl(null);
      });
    return () => {
      cancelled = true;
    };
  }, [src]);

  if (!url) {
    return (
      <span className="inline-block w-16 h-10 bg-muted rounded text-[10px] text-muted-foreground flex items-center justify-center">
        {alt || "[图片]"}
      </span>
    );
  }
  return <img src={url} alt={alt || ""} className="max-w-full rounded my-1" />;
}

/** Markdown 序列化存储类型（tiptap-markdown 提供）。 */
type MarkdownStorage = {
  getMarkdown(): string;
};

/** 创建编辑器实例。onUpdate 由调用方传入（debounce 后 update_note）。 */
export function useNoteEditor(
  content: string,
  onUpdate: (html: string) => void,
) {
  const editor = useEditor({
    immediatelyRender: false, // SSR/桌面端安全：返回 Editor | null
    extensions: [
      StarterKit,
      Link.configure({ openOnClick: false }),
      Image.extend({
        addNodeView() {
          return ReactNodeViewRenderer(NoteImageView);
        },
      }),
      Markdown, // md 序列化（editor.storage.markdown.getMarkdown()）
    ],
    content,
    onUpdate: ({ editor }) => onUpdate(editor.getHTML()),
  });

  // 切换 note 时重设 content（emitUpdate:false 避免触发自动保存）
  useEffect(() => {
    if (editor && !editor.isDestroyed) {
      editor.commands.setContent(content || "", { emitUpdate: false });
    }
  }, [content, editor]);

  return editor;
}

/** 从 editor.storage 取 markdown 文本（导出 .md 用）。 */
export function getMarkdownFromEditor(editor: Editor): string {
  const storage = editor.storage as unknown as Record<string, unknown>;
  const md = storage.markdown as MarkdownStorage | undefined;
  return md?.getMarkdown() ?? "";
}
