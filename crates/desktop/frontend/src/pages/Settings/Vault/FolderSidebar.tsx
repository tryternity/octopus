import { useState } from "react";
import { Folder, Pencil, Plus, Star, Trash2 } from "lucide-react";
import { cn } from "@/lib/utils";
import { useT } from "@/lib/i18n";
import type { FolderCounts, FolderDto, FolderSelection } from "./folderTypes";

/**
 * FolderSidebar —— VaultPanel 左侧导航（follow-up #6）。
 *
 * 三组：
 *   1. 顶层过滤：所有条目 / 收藏
 *   2. 文件夹列表（带创建按钮 + 单项 rename / delete）
 *   3. 回收站
 *
 * 不做 DnD / 嵌套（out of scope，单层 folder）。
 */
export function FolderSidebar({
  folders,
  counts,
  selected,
  onSelect,
  onCreateFolder,
  onRenameFolder,
  onDeleteFolder,
}: {
  folders: FolderDto[];
  counts: FolderCounts;
  selected: FolderSelection;
  onSelect: (sel: FolderSelection) => void;
  onCreateFolder: () => void;
  onRenameFolder: (folder: FolderDto) => void;
  onDeleteFolder: (folder: FolderDto) => void;
}) {
  const t = useT();
  return (
    <nav className="flex w-48 shrink-0 flex-col gap-0.5 overflow-y-auto border-r border-border/40 p-2">
      <SidebarItem
        label={t("settings.vault.folder.allItems")}
        count={counts.all}
        active={selected === "all"}
        onClick={() => onSelect("all")}
      />
      <SidebarItem
        icon={<Star className="size-3.5 text-amber-500" />}
        label={t("settings.vault.folder.favorites")}
        count={counts.favorites}
        active={selected === "favorites"}
        onClick={() => onSelect("favorites")}
      />

      <div className="flex items-center justify-between px-2 pb-1 pt-3 text-[10px] font-medium uppercase tracking-wider text-muted-foreground/60">
        <span>{t("settings.vault.folder.section")}</span>
        <button
          onClick={onCreateFolder}
          title={t("settings.vault.folder.newFolder")}
          className="text-muted-foreground/70 transition-colors hover:text-foreground"
        >
          <Plus className="size-3" />
        </button>
      </div>

      {folders.length === 0 ? (
        <p className="px-2 py-1 text-[11px] text-muted-foreground/50">
          {t("settings.vault.folder.empty")}
        </p>
      ) : (
        folders.map((f) => (
          <FolderRow
            key={f.id}
            folder={f}
            count={counts[f.id] ?? 0}
            active={selected === f.id}
            onClick={() => onSelect(f.id)}
            onRename={() => onRenameFolder(f)}
            onDelete={() => onDeleteFolder(f)}
          />
        ))
      )}

      <div className="pt-2">
        <SidebarItem
          icon={<Trash2 className="size-3.5 text-muted-foreground/70" />}
          label={t("settings.vault.folder.trash")}
          count={counts.trash}
          active={selected === "trash"}
          onClick={() => onSelect("trash")}
        />
      </div>
    </nav>
  );
}

function SidebarItem({
  icon,
  label,
  count,
  active,
  onClick,
}: {
  icon?: React.ReactNode;
  label: string;
  count?: number;
  active: boolean;
  onClick: () => void;
}) {
  return (
    <button
      onClick={onClick}
      className={cn(
        "flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-xs transition-colors",
        active
          ? "bg-voice/10 font-medium text-voice"
          : "text-muted-foreground hover:bg-muted/40 hover:text-foreground",
      )}
    >
      {icon ?? <span className="size-3.5" />}
      <span className="flex-1 truncate">{label}</span>
      {count !== undefined && count > 0 && (
        <span className="shrink-0 text-[10px] tabular-nums text-muted-foreground/70">
          {count}
        </span>
      )}
    </button>
  );
}

function FolderRow({
  folder,
  count,
  active,
  onClick,
  onRename,
  onDelete,
}: {
  folder: FolderDto;
  count: number;
  active: boolean;
  onClick: () => void;
  onRename: () => void;
  onDelete: () => void;
}) {
  const [menuOpen, setMenuOpen] = useState(false);
  return (
    <div
      className="group relative flex items-center"
      onMouseEnter={() => setMenuOpen(true)}
      onMouseLeave={() => setMenuOpen(false)}
    >
      <button
        onClick={onClick}
        className={cn(
          "flex flex-1 items-center gap-2 rounded-md px-2 py-1.5 text-left text-xs transition-colors",
          active
            ? "bg-voice/10 font-medium text-voice"
            : "text-muted-foreground hover:bg-muted/40 hover:text-foreground",
        )}
      >
        <Folder className="size-3.5 shrink-0 opacity-70" />
        <span className="flex-1 truncate">{folder.name}</span>
        {count > 0 && (
          <span className="shrink-0 text-[10px] tabular-nums text-muted-foreground/70">
            {count}
          </span>
        )}
      </button>
      {menuOpen && (
        <div className="absolute right-1 flex shrink-0 items-center gap-0.5">
          <button
            onClick={(e) => {
              e.stopPropagation();
              onRename();
            }}
            title="Rename"
            className="rounded p-0.5 text-muted-foreground/70 hover:bg-accent hover:text-foreground"
          >
            <Pencil className="size-3" />
          </button>
          <button
            onClick={(e) => {
              e.stopPropagation();
              onDelete();
            }}
            title="Delete"
            className="rounded p-0.5 text-muted-foreground/70 hover:bg-destructive/10 hover:text-destructive"
          >
            <Trash2 className="size-3" />
          </button>
        </div>
      )}
    </div>
  );
}
