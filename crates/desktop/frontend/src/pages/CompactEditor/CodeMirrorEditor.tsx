import { useEffect, useRef, type RefObject } from "react";
import { Compartment, EditorState } from "@codemirror/state";
import { EditorView, keymap, lineNumbers, highlightActiveLine, drawSelection } from "@codemirror/view";
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { HighlightStyle, syntaxHighlighting, bracketMatching } from "@codemirror/language";
import { search, searchKeymap } from "@codemirror/search";
import { markdown } from "@codemirror/lang-markdown";
import { tags as t } from "@lezer/highlight";

const mdHighlight = HighlightStyle.define([
  { tag: t.heading1, fontSize: "1.35em", fontWeight: "600", color: "var(--color-foreground)" },
  { tag: t.heading2, fontSize: "1.18em", fontWeight: "600", color: "var(--color-foreground)" },
  { tag: t.heading3, fontSize: "1.08em", fontWeight: "600", color: "var(--color-foreground)" },
  { tag: [t.heading4, t.heading5, t.heading6], fontWeight: "600", color: "var(--color-foreground)" },
  { tag: t.strong, fontWeight: "600", color: "var(--color-foreground)" },
  { tag: t.emphasis, fontStyle: "italic", color: "var(--color-foreground)" },
  { tag: t.strikethrough, textDecoration: "line-through", color: "var(--color-muted-foreground)" },
  { tag: t.link, color: "var(--color-voice, #d97706)", textDecoration: "none" },
  { tag: t.url, color: "var(--color-voice, #d97706)" },
  { tag: t.quote, color: "var(--color-muted-foreground)", fontStyle: "italic" },
  { tag: t.monospace, color: "var(--color-voice, #d97706)" },
  { tag: t.list, color: "var(--color-voice, #d97706)" },
  { tag: t.contentSeparator, color: "var(--color-muted-foreground)" },
  { tag: t.meta, color: "var(--color-muted-foreground)" },
  { tag: t.processingInstruction, color: "var(--color-muted-foreground)" },
]);

function buildTheme(fontSize: number) {
  return EditorView.theme(
    {
      "&": {
        height: "100%",
        backgroundColor: "transparent",
        color: "var(--color-foreground)",
        fontFamily: "ui-monospace, SFMono-Regular, Menlo, monospace",
        fontSize: `${fontSize}px`,
      },
      ".cm-scroller": {
        fontFamily: "ui-monospace, SFMono-Regular, Menlo, monospace",
        lineHeight: "1.6",
        padding: "16px 20px 60px 0",
      },
      ".cm-content": {
        caretColor: "var(--color-voice, #d97706)",
        paddingLeft: "10px",
      },
      ".cm-cursor, .cm-dropCursor": {
        borderLeftColor: "var(--color-voice, #d97706)",
        borderLeftWidth: "1.5px",
      },
      ".cm-gutters": {
        backgroundColor: "transparent",
        color: "var(--color-muted-foreground)",
        border: "none",
        fontFamily: "ui-monospace, SFMono-Regular, Menlo, monospace",
        fontSize: "11px",
        margin: "0",
        padding: "0",
        boxShadow: "none",
      },
      ".cm-lineNumbers": {
        boxSizing: "border-box",
        borderRight: "1px solid var(--color-border)",
        minWidth: "36px",
        width: "36px",
      },
      ".cm-lineNumbers .cm-gutterElement": {
        boxSizing: "border-box",
        minWidth: "36px",
        padding: "0 8px 0 0",
        textAlign: "right",
      },
      ".cm-activeLine": { backgroundColor: "transparent" },
      ".cm-activeLineGutter": { backgroundColor: "transparent", color: "var(--color-foreground)" },
      "&.cm-focused": { outline: "none" },
      "&.cm-focused .cm-selectionBackground, ::selection, .cm-selectionBackground": {
        backgroundColor: "color-mix(in srgb, var(--color-voice, #d97706) 22%, transparent)",
      },
      ".cm-line": { padding: "0" },
      ".cm-panels": {
        backgroundColor: "var(--color-muted)",
        color: "var(--color-foreground)",
        borderColor: "var(--color-border)",
      },
      ".cm-panels.cm-panels-top": { borderBottom: "1px solid var(--color-border)" },
      ".cm-textfield": {
        background: "var(--color-background)",
        color: "var(--color-foreground)",
        border: "1px solid var(--color-border)",
        borderRadius: "4px",
        padding: "3px 8px",
        fontFamily: "ui-monospace, monospace",
        fontSize: "12px",
        outline: "none",
      },
      ".cm-button": {
        background: "var(--color-background)",
        color: "var(--color-foreground)",
        border: "1px solid var(--color-border)",
        borderRadius: "4px",
        padding: "3px 8px",
        fontFamily: "inherit",
        fontSize: "11px",
        cursor: "pointer",
        backgroundImage: "none",
      },
      ".cm-searchMatch": {
        backgroundColor: "color-mix(in srgb, var(--color-voice, #d97706) 22%, transparent)",
        borderRadius: "2px",
      },
      ".cm-searchMatch-selected": {
        backgroundColor: "color-mix(in srgb, var(--color-voice, #d97706) 45%, transparent)",
      },
    },
    { dark: false },
  );
}

interface CodeMirrorEditorProps {
  value: string;
  readOnly: boolean;
  fontSize: number;
  onChange: (next: string) => void;
  viewRef?: RefObject<EditorView | null>;
}

export function CodeMirrorEditor({ value, readOnly, fontSize, onChange, viewRef: externalViewRef }: CodeMirrorEditorProps) {
  const hostRef = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView | null>(null);
  const onChangeRef = useRef(onChange);
  onChangeRef.current = onChange;
  const themeCompartment = useRef(new Compartment());
  const readOnlyCompartment = useRef(new Compartment());

  useEffect(() => {
    if (!hostRef.current) return;

    const state = EditorState.create({
      doc: value,
      extensions: [
        lineNumbers(),
        history(),
        drawSelection(),
        highlightActiveLine(),
        bracketMatching(),
        syntaxHighlighting(mdHighlight, { fallback: true }),
        markdown(),
        EditorView.lineWrapping,
        search({ top: true }),
        keymap.of([...defaultKeymap, ...historyKeymap, ...searchKeymap, indentWithTab]),
        readOnlyCompartment.current.of(EditorState.readOnly.of(readOnly)),
        themeCompartment.current.of(buildTheme(fontSize)),
        EditorView.updateListener.of((update) => {
          if (update.docChanged) {
            onChangeRef.current(update.state.doc.toString());
          }
        }),
      ],
    });

    const view = new EditorView({ state, parent: hostRef.current });
    viewRef.current = view;
    if (externalViewRef) externalViewRef.current = view;

    return () => {
      view.destroy();
      viewRef.current = null;
      if (externalViewRef) externalViewRef.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    const view = viewRef.current;
    if (!view) return;
    const current = view.state.doc.toString();
    if (current !== value) {
      view.dispatch({ changes: { from: 0, to: current.length, insert: value } });
    }
  }, [value]);

  useEffect(() => {
    const view = viewRef.current;
    if (!view) return;
    view.dispatch({ effects: themeCompartment.current.reconfigure(buildTheme(fontSize)) });
  }, [fontSize]);

  useEffect(() => {
    const view = viewRef.current;
    if (!view) return;
    view.dispatch({ effects: readOnlyCompartment.current.reconfigure(EditorState.readOnly.of(readOnly)) });
  }, [readOnly]);

  return <div ref={hostRef} className="md-cm-editor flex-1 min-h-0 min-w-0" style={{ overflow: "hidden" }} />;
}
