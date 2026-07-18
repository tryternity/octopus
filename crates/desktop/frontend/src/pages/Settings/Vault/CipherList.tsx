import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { confirm } from "@tauri-apps/plugin-dialog";
import { Plus, Search, Star, Trash2 } from "lucide-react";
import { cn } from "@/lib/utils";
import { useT } from "@/lib/i18n";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import CipherEditor from "./CipherEditor";
import { FolderSidebar } from "./FolderSidebar";
import {
  FolderPromptDialog,
  type PromptOptions,
} from "./FolderPromptDialog";
import type {
  FolderCounts,
  FolderDto,
  FolderSelection,
} from "./folderTypes";

/**
 * CipherList —— 密码条目列表 + 新建入口 + folder 侧栏（follow-up #6）。
 *
 * 布局：左侧 FolderSidebar（所有/收藏/folders/回收站）+ 右侧搜索+列表。
 *
 * 后端 `vault_list_ciphers` 一次拉全量（含已软删除的）。
 * 搜索过滤：name / username / uri 任一命中；同时叠加 sidebar 选择。
 * 单击行进入 CipherEditor（新建或编辑）。
 */

// 与后端 CipherDto 对齐（vault_commands.rs，snake_case，无 rename_all）
interface LoginUriDto {
  uri: string;
  match_type: number | null;
}
interface LoginDataDto {
  uris: LoginUriDto[];
  username: string | null;
  password: string | null;
  totp: string | null;
}
interface FieldDto {
  name: string;
  value: string | null;
  field_type: number;
}
export interface CipherDto {
  id: number;
  folder_id: number | null;
  favorite: boolean;
  atype: number;
  name: string;
  notes: string | null;
  login: LoginDataDto | null;
  fields: FieldDto[];
  reprompt: number;
  deleted_at: string | null;
  created_at: string;
  updated_at: string;
}

export default function CipherList({ showToast }: { showToast: (msg: string) => void }) {
  const t = useT();
  const [ciphers, setCiphers] = useState<CipherDto[]>([]);
  const [folders, setFolders] = useState<FolderDto[]>([]);
  const [selected, setSelected] = useState<FolderSelection>("all");
  const [query, setQuery] = useState("");
  const [loaded, setLoaded] = useState(false);
  const [editing, setEditing] = useState<number | "new" | null>(null);
  // folder prompt dialog 状态：null=关闭，对象=打开（标题/初值不同区分新建/重命名）
  const [prompt, setPrompt] = useState<PromptOptions | null>(null);
  // pending rename 目标（与 prompt 配对：confirm 时按是否设此区分新建 vs 重命名）
  const [renameTarget, setRenameTarget] = useState<FolderDto | null>(null);

  const refreshCiphers = useCallback(async () => {
    try {
      const list = await invoke<CipherDto[]>("vault_list_ciphers");
      setCiphers(list);
    } catch (e) {
      showToast(String(e));
    }
    setLoaded(true);
  }, [showToast]);

  const refreshFolders = useCallback(async () => {
    try {
      const list = await invoke<FolderDto[]>("vault_list_folders");
      setFolders(list);
    } catch (e) {
      // vault 未解锁或失败：静默——sidebar 留空即可
      setFolders([]);
      void e;
    }
  }, []);

  useEffect(() => {
    refreshCiphers();
    refreshFolders();
  }, [refreshCiphers, refreshFolders]);

  // 各 sidebar 项的条目计数（用于角标）。同一份 ciphers 派生，零额外请求。
  const counts: FolderCounts = useMemo(() => {
    const c: FolderCounts = { all: 0, favorites: 0, trash: 0 };
    for (const cipher of ciphers) {
      if (cipher.deleted_at) {
        c.trash += 1;
        continue;
      }
      c.all += 1;
      if (cipher.favorite) c.favorites += 1;
      if (cipher.folder_id !== null) {
        c[cipher.folder_id] = (c[cipher.folder_id] ?? 0) + 1;
      }
    }
    return c;
  }, [ciphers]);

  // sidebar selection + 搜索框 双重过滤
  const filtered = useMemo(() => {
    // 1. sidebar selection
    const sel = ciphers.filter((c) => {
      if (c.deleted_at) return selected === "trash";
      if (selected === "all") return true;
      if (selected === "favorites") return c.favorite;
      if (selected === "trash") return false;
      return c.folder_id === selected;
    });
    // 2. 搜索 query
    const q = query.trim().toLowerCase();
    if (!q) return sel;
    return sel.filter((c) => {
      const uname = c.login?.username ?? "";
      const uris = c.login?.uris ?? [];
      return (
        c.name.toLowerCase().includes(q) ||
        uname.toLowerCase().includes(q) ||
        uris.some((u) => u.uri.toLowerCase().includes(q))
      );
    });
  }, [ciphers, selected, query]);

  // === Folder CRUD ===

  const handleCreateFolderClick = useCallback(() => {
    setRenameTarget(null);
    setPrompt({
      title: t("settings.vault.folder.newFolder"),
      confirmLabel: t("settings.vault.editor.save"),
    });
  }, [t]);

  const handleRenameFolder = useCallback(
    (folder: FolderDto) => {
      setRenameTarget(folder);
      setPrompt({
        title: t("settings.vault.folder.renameFolder"),
        initial: folder.name,
        confirmLabel: t("settings.vault.editor.save"),
      });
    },
    [t],
  );

  const handleDeleteFolder = useCallback(
    async (folder: FolderDto) => {
      const ok = await confirm(
        t("settings.vault.folder.folderDeleteWarning"),
        {
          title: t("settings.vault.folder.deleteFolder") + ": " + folder.name,
          kind: "warning",
        },
      );
      if (!ok) return;
      try {
        await invoke("vault_delete_folder", { id: folder.id });
        // 当前选中的正是被删 folder → 退回 "all"，避免空视图
        if (selected === folder.id) setSelected("all");
        await refreshCiphers(); // folder 下 cipher 的 folder_id 被 FK SET NULL
        await refreshFolders();
        showToast(t("settings.vault.folder.deleteFolder"));
      } catch (e) {
        showToast(String(e));
      }
    },
    [refreshCiphers, refreshFolders, selected, showToast, t],
  );

  const handlePromptConfirm = useCallback(
    async (value: string) => {
      setPrompt(null);
      try {
        if (renameTarget) {
          await invoke("vault_rename_folder", { id: renameTarget.id, name: value });
          showToast(t("settings.vault.folder.renameFolder"));
        } else {
          await invoke("vault_create_folder", { name: value });
          showToast(t("settings.vault.folder.newFolder"));
        }
        await refreshFolders();
      } catch (e) {
        showToast(String(e));
      } finally {
        setRenameTarget(null);
      }
    },
    [renameTarget, refreshFolders, showToast, t],
  );

  const handlePromptCancel = useCallback(() => {
    setPrompt(null);
    setRenameTarget(null);
  }, []);

  if (editing !== null) {
    return (
      <CipherEditor
        cipherId={editing === "new" ? null : editing}
        folders={folders}
        onClose={async () => {
          setEditing(null);
          await refreshCiphers();
        }}
        showToast={showToast}
      />
    );
  }

  return (
    <div className="flex h-full min-h-0 gap-3">
      <FolderSidebar
        folders={folders}
        counts={counts}
        selected={selected}
        onSelect={setSelected}
        onCreateFolder={handleCreateFolderClick}
        onRenameFolder={handleRenameFolder}
        onDeleteFolder={handleDeleteFolder}
      />

      <div className="flex min-w-0 flex-1 flex-col gap-3">
        <div className="flex shrink-0 items-center gap-2">
          <div className="relative flex-1">
            <Search className="pointer-events-none absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground/50" />
            <Input
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder={t("settings.vault.list.search")}
              className="pl-8"
              size="full"
            />
          </div>
          <Button variant="voice" size="sm" onClick={() => setEditing("new")}>
            <Plus />
            {t("settings.vault.list.addNew")}
          </Button>
        </div>

        <div className="min-h-0 flex-1 overflow-y-auto">
          {!loaded ? (
            <p className="py-12 text-center text-sm text-muted-foreground">
              {t("settings.loading")}
            </p>
          ) : filtered.length === 0 ? (
            <div className="flex flex-col items-center justify-center gap-2 py-16 text-center">
              <p className="text-sm font-medium">{t("settings.vault.list.empty")}</p>
            </div>
          ) : (
            <div className="space-y-px">
              {filtered.map((c) => (
                <button
                  key={c.id}
                  onClick={() => setEditing(c.id)}
                  className={cn(
                    "group flex w-full items-center gap-3 rounded-md px-2.5 py-2 text-left transition-colors",
                    "hover:bg-muted/40",
                  )}
                >
                  <span
                    className={cn(
                      "row-span-2 h-full max-h-[28px] w-[3px] shrink-0 self-stretch rounded-full",
                      c.deleted_at
                        ? "bg-muted-foreground/40"
                        : c.favorite
                          ? "bg-amber-500"
                          : "bg-voice/50",
                    )}
                  />
                  <div className="min-w-0 flex-1">
                    <div className="flex items-center gap-1.5">
                      <span
                        className={cn(
                          "truncate text-sm font-medium",
                          c.deleted_at && "text-muted-foreground/60 line-through",
                        )}
                      >
                        {c.name}
                      </span>
                      {c.favorite && (
                        <Star className="size-3 shrink-0 fill-amber-500 text-amber-500" />
                      )}
                    </div>
                    <div className="truncate text-xs text-muted-foreground/70">
                      {c.login?.username || "—"}
                    </div>
                  </div>
                  {c.deleted_at && (
                    <span className="shrink-0 rounded bg-destructive/10 px-1.5 py-0.5 text-[10px] uppercase tracking-wider text-destructive">
                      {t("settings.vault.list.deleted")}
                    </span>
                  )}
                  <Trash2 className="size-3.5 shrink-0 text-muted-foreground/40 opacity-0 transition-opacity group-hover:opacity-100" />
                </button>
              ))}
            </div>
          )}
        </div>
      </div>

      {prompt && (
        <FolderPromptDialog
          options={prompt}
          onConfirm={handlePromptConfirm}
          onCancel={handlePromptCancel}
        />
      )}
    </div>
  );
}
