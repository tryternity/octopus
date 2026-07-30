// 菜单行组件。从 ActionBarPanel.tsx 拆出（2026-07-30）。

import { ArrowUp, ArrowDown, Trash2, Pencil, X } from "lucide-react";
import { cn } from "@/lib/utils";
import { useT } from "@/lib/i18n";
import ShortcutButton from "@/components/ShortcutButton";
import type { ActionBarItem } from "./types";
import { TYPE_META, pad2, TypeTag } from "./constants";

export interface MenuRowProps {
  item: ActionBarItem;
  index: number;
  selected: boolean;
  isFirst: boolean;
  isLast: boolean;
  deleteConfirmId: number | null;
  isMain?: boolean;
  onSelect?: () => void;
  onMove: (dir: number) => void;
  onEdit: () => void;
  onDelete: () => void;
  showShortcuts?: boolean;
  onCaptureShortcut?: () => void;
  capturing?: boolean;
  onClearShortcut?: () => void;
}

export default function MenuRow(props: MenuRowProps) {
  const t = useT();
  const { item, index, selected, isFirst, isLast, deleteConfirmId } = props;
  const meta = TYPE_META[item.actionType] ?? { bar: "bg-stone-400", dot: "bg-stone-400", label: (item.actionType || "unknown").toUpperCase().slice(0, 8), descKey: "", placeholderKey: "" };
  const isDeleting = deleteConfirmId === item.id;
  const showShortcuts = !!props.showShortcuts && item.actionType !== "submenu";

  return (
    <div
      onClick={props.onSelect}
      className={cn(
        "group relative grid items-center gap-x-2 gap-y-0.5 rounded-md py-1.5 pl-1 pr-1.5 transition-colors",
        showShortcuts
          ? "[grid-template-columns:auto_auto_minmax(40px,1fr)_5rem_auto_auto]"
          : "[grid-template-columns:auto_auto_1fr_auto]",
        selected ? "bg-voice/12" : "hover:bg-muted/40",
        props.onSelect && "cursor-pointer",
      )}
    >
      <div className={cn("row-span-2 h-full w-[3px] shrink-0 self-stretch rounded-full transition-all", meta.bar)} />
      <span className="row-span-2 self-start pt-0.5 text-right font-mono text-[11px] tabular-nums text-muted-foreground/50">
        {pad2(index)}
      </span>
      <span className={cn(
        "min-w-0 truncate",
        props.isMain ? "text-sm font-semibold" : "text-sm",
        item.isEnabled ? "text-foreground" : "text-muted-foreground/50",
      )}>
        {item.title}
      </span>

      {showShortcuts && (
        /* 斜杠命令名（显式 grid-column:4 独立列，各行 / 左对齐） */
        <span className={cn(
          "col-start-4 row-start-1 min-w-0 truncate rounded px-1.5 py-0.5 text-left font-mono text-[11px]",
          item.triggerKeyword ? "bg-muted/60 text-muted-foreground" : "",
        )}>
          {item.triggerKeyword ? `/${item.triggerKeyword}` : ""}
        </span>
      )}

      {showShortcuts && (
        <div className="flex shrink-0 items-center justify-end gap-0.5">
          {/* 全局快捷键 */}
          <ShortcutButton
            shortcut={item.globalShortcut ?? ""}
            capturing={props.capturing ?? false}
            onClick={() => props.onCaptureShortcut?.()}
            title={t("settings.actionBar.globalShortcutHint")}
          />
          <button
            onClick={(e) => { e.stopPropagation(); props.onClearShortcut?.(); }}
            className={cn(
              "rounded p-0.5 text-muted-foreground/50 transition-opacity hover:bg-destructive/10 hover:text-destructive",
              item.globalShortcut && !props.capturing
                ? "opacity-0 group-hover:opacity-100"
                : "invisible",
            )}
            aria-label={t("settings.actionBar.clearShortcut")}
          >
            <X className="h-3 w-3" />
          </button>
        </div>
      )}

      <div className={cn(
        "flex shrink-0 items-center gap-0.5 transition-opacity focus-within:opacity-100 group-hover:opacity-100",
        // 左侧主菜单：平时隐藏（hover 显示，避免拥挤）；右侧子菜单：常驻（避免空白）
        props.isMain ? "opacity-0" : "opacity-60",
      )}>
        <button
          onClick={(e) => { e.stopPropagation(); props.onMove(-1); }}
          disabled={isFirst}
          className="rounded p-0.5 text-muted-foreground transition-colors hover:text-foreground disabled:opacity-25"
          aria-label={t("settings.actionBar.moveUp")}
        >
          <ArrowUp className="h-3.5 w-3.5" />
        </button>
        <button
          onClick={(e) => { e.stopPropagation(); props.onMove(1); }}
          disabled={isLast}
          className="rounded p-0.5 text-muted-foreground transition-colors hover:text-foreground disabled:opacity-25"
          aria-label={t("settings.actionBar.moveDown")}
        >
          <ArrowDown className="h-3.5 w-3.5" />
        </button>
        <button
          onClick={(e) => { e.stopPropagation(); props.onEdit(); }}
          className="rounded p-0.5 text-muted-foreground transition-colors hover:text-foreground"
          aria-label={t("settings.actionBar.edit")}
        >
          <Pencil className="h-3.5 w-3.5" />
        </button>
        <button
          onClick={(e) => { e.stopPropagation(); props.onDelete(); }}
          disabled={item.isSystem}
          className={cn(
            "rounded p-0.5 transition-colors disabled:opacity-25",
            isDeleting
              ? "bg-destructive text-destructive-foreground hover:opacity-90"
              : "text-muted-foreground hover:text-destructive",
          )}
          aria-label={t("settings.actionBar.delete")}
          title={isDeleting ? t("settings.actionBar.deleteConfirm") : t("settings.actionBar.delete")}
        >
          {isDeleting ? (
            <span className="px-1 text-[10px] font-medium">{t("settings.actionBar.confirm")}</span>
          ) : (
            <Trash2 className="h-3.5 w-3.5" />
          )}
        </button>
      </div>

      <div className={cn("flex items-center gap-1.5 text-[10px] text-muted-foreground/60", showShortcuts ? "col-span-2" : "col-span-2")}>
        <TypeTag type={item.actionType} />
        {item.isSystem && (
          <span className="text-muted-foreground/40">· {t("settings.actionBar.builtin")}</span>
        )}
        {!item.isEnabled && (
          <span className="text-muted-foreground/40">· {t("settings.actionBar.hidden")}</span>
        )}
      </div>
    </div>
  );
}
