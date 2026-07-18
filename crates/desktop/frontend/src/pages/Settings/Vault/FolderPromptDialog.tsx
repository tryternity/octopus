import { useEffect, useRef, useState } from "react";
import { X } from "lucide-react";
import { useT } from "@/lib/i18n";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";

/**
 * FolderPromptDialog —— 内联模态：用于新建 / 重命名 folder（follow-up #6）。
 *
 * `@tauri-apps/plugin-dialog` 只提供 `confirm` / `ask` / `message`，没有 prompt，
 * 故实现一个最小的内联 modal：标题 + 单行输入 + 取消/确认。Esc 取消，Enter 确认。
 *
 * 受控组件：父层用 `null` 表示关闭，传 `{ title, initial, confirmLabel }` 表示打开。
 */
export interface PromptOptions {
  title: string;
  initial?: string;
  confirmLabel?: string;
}

export function FolderPromptDialog({
  options,
  onConfirm,
  onCancel,
}: {
  options: PromptOptions;
  onConfirm: (value: string) => void;
  onCancel: () => void;
}) {
  const t = useT();
  const [value, setValue] = useState(options.initial ?? "");
  const inputRef = useRef<HTMLInputElement | null>(null);

  // 打开时 autofocus + 选全（rename 时方便整体替换）。
  useEffect(() => {
    const id = setTimeout(() => {
      inputRef.current?.focus();
      inputRef.current?.select();
    }, 0);
    return () => clearTimeout(id);
  }, []);

  function submit() {
    const trimmed = value.trim();
    if (!trimmed) return;
    onConfirm(trimmed);
  }

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/40"
      onClick={onCancel}
    >
      <div
        className="w-80 rounded-lg border border-border/60 bg-background p-4 shadow-xl"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="mb-3 flex items-center justify-between">
          <h3 className="text-sm font-medium">{options.title}</h3>
          <button
            onClick={onCancel}
            className="text-muted-foreground/70 hover:text-foreground"
          >
            <X className="size-4" />
          </button>
        </div>
        <Input
          ref={inputRef}
          value={value}
          onChange={(e) => setValue(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              submit();
            } else if (e.key === "Escape") {
              e.preventDefault();
              onCancel();
            }
          }}
          size="full"
          autoFocus
        />
        <div className="mt-3 flex justify-end gap-2">
          <Button variant="outline" size="sm" onClick={onCancel}>
            {t("settings.vault.editor.cancel")}
          </Button>
          <Button
            variant="voice"
            size="sm"
            disabled={!value.trim()}
            onClick={submit}
          >
            {options.confirmLabel ?? t("settings.vault.editor.save")}
          </Button>
        </div>
      </div>
    </div>
  );
}
