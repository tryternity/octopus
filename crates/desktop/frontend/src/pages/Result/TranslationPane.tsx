import { useEffect, useRef } from "react";
import { EditorState } from "@codemirror/state";
import { EditorView, keymap, drawSelection } from "@codemirror/view";
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { useT } from "@/lib/i18n";

interface TranslationPaneProps {
  text: string;
  translating: boolean;
  onChange?: (s: string) => void;
}

function buildTheme() {
  return EditorView.theme({
    "&": {
      height: "100%",
      backgroundColor: "transparent",
      color: "var(--color-foreground)",
      fontFamily: "-apple-system, BlinkMacSystemFont, sans-serif",
      fontSize: "14px",
    },
    ".cm-scroller": {
      lineHeight: "1.6",
      padding: "4px 14px 8px",
      fontFamily: "inherit",
    },
    ".cm-gutters": { display: "none" },
    ".cm-activeLine": { backgroundColor: "transparent" },
    ".cm-activeLineGutter": { backgroundColor: "transparent" },
    "&.cm-focused": { outline: "none" },
    "&.cm-focused .cm-selectionBackground, ::selection, .cm-selectionBackground": {
      backgroundColor: "color-mix(in srgb, var(--color-voice, #d97706) 22%, transparent)",
    },
    ".cm-cursor, .cm-dropCursor": {
      borderLeftColor: "var(--color-voice, #d97706)",
      borderLeftWidth: "1.5px",
    },
    ".cm-line": { padding: "0" },
  }, { dark: false });
}

export function TranslationPane({ text, translating, onChange }: TranslationPaneProps) {
  const t = useT();
  const hostRef = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView | null>(null);
  const onChangeRef = useRef(onChange);
  onChangeRef.current = onChange;

  useEffect(() => {
    if (!hostRef.current) return;

    const state = EditorState.create({
      doc: text,
      extensions: [
        history(),
        drawSelection(),
        EditorView.lineWrapping,
        keymap.of([...defaultKeymap, ...historyKeymap, indentWithTab]),
        buildTheme(),
        EditorView.updateListener.of((update) => {
          if (update.docChanged) {
            onChangeRef.current?.(update.state.doc.toString());
          }
        }),
      ],
    });

    const view = new EditorView({ state, parent: hostRef.current });
    viewRef.current = view;

    return () => {
      view.destroy();
      viewRef.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    const view = viewRef.current;
    if (!view) return;
    const current = view.state.doc.toString();
    if (text === current) return;
    const len = view.state.doc.length;
    view.dispatch({ changes: { from: 0, to: len, insert: text } });
  }, [text]);

  return (
    <div className="relative h-full flex flex-col">
      {translating && (
        <div className="flex-shrink-0 px-3.5 pt-1 text-[11px]" style={{ color: "var(--color-tool-icon)", opacity: 0.5 }}>
          {t("result.translating")}
        </div>
      )}
      <div ref={hostRef} className="flex-1 overflow-hidden" style={{ minHeight: 0 }} />
    </div>
  );
}
