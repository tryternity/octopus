import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { confirm } from "@tauri-apps/plugin-dialog";
import { Plus, Search } from "lucide-react";
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
 * 布局：左侧 FolderSidebar（所有/收藏/folders/回收站）+ 右侧搜索 + 卡片网格。
 *
 * 后端 `vault_list_ciphers` 一次拉全量（含已软删除的）。
 * 搜索过滤：name / username / uri 任一命中；同时叠加 sidebar 选择。
 * 单击卡片进入 CipherEditor（新建或编辑）。
 *
 * 视觉（UI 重设计）：表格行 → 封印条卡片。每张卡左侧 3px 竖条（封印），
 * 颜色编码状态：默认 fg/30、收藏 fg（满）、弱密码 warning、已删 虚线无填充。
 * 凭证数据（name / username / password 掩码）用 .font-mono-vault 等宽字。
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
  // 刚被点击的卡片 id——短暂高亮（active 态 bg-accent）
  const [activeId, setActiveId] = useState<number | null>(null);

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

      <div className="flex min-w-0 flex-1 flex-col gap-2">
        {/* 搜索 + 新建 —— 单行紧凑 */}
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

        {/* 卡片网格区 */}
        <div className="thin-scrollbar min-h-0 flex-1 overflow-y-auto pr-1">
          {!loaded ? (
            <p className="py-12 text-center text-sm text-muted-foreground">
              {t("settings.loading")}
            </p>
          ) : filtered.length === 0 ? (
            <div className="flex flex-col items-center justify-center gap-2 py-16 text-center">
              <p className="text-sm font-medium">{t("settings.vault.list.empty")}</p>
            </div>
          ) : (
            <div className="space-y-1.5">
              {filtered.map((c) => (
                <CipherCard
                  key={c.id}
                  cipher={c}
                  active={activeId === c.id}
                  onClick={() => {
                    setActiveId(c.id);
                    setEditing(c.id);
                  }}
                />
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

/**
 * CipherCard —— 封印条卡片。
 *
 * 左侧 3px 竖条（封印）+ 顶部小圆节点，颜色编码状态：
 *   已删 → 虚线无填充（border-dashed）
 *   收藏 → fg 满色 + 右上角 ★
 *   弱密码 → warning 色（TODO: 当前未接入 weak_cipher_ids，始终 false）
 *   默认 → fg/30
 *
 * 凭证数据（name / username / password 掩码）用 .font-mono-vault 等宽字。
 */
function CipherCard({
  cipher,
  active,
  onClick,
}: {
  cipher: CipherDto;
  active: boolean;
  onClick: () => void;
}) {
  const t = useT();

  // TODO(v2): 接入 HealthReport 的 weak_cipher_ids 做弱密码高亮。
  // 当前 isWeak 始终 false——避免在卡片层重复拉健康报告。
  // 后续可由 VaultPanel 拉一次 weak list 透传下来，或新增轻量 vault_weak_ids 命令。
  const isWeak = false;

  const sealColor = cipher.deleted_at
    ? "border-l-[3px] border-dashed border-muted-foreground/40 bg-transparent"
    : isWeak
      ? "bg-warning"
      : cipher.favorite
        ? "bg-foreground"
        : "bg-foreground/30";

  return (
    <button
      onClick={onClick}
      className={cn(
        "group relative w-full overflow-hidden rounded-md border border-border bg-background p-3 pl-5 text-left transition-colors",
        "hover:bg-accent/50",
        active && "bg-accent",
        cipher.deleted_at && "opacity-60",
      )}
    >
      {/* 封印条：左侧 3px 竖条 + 顶部圆节点 */}
      {cipher.deleted_at ? (
        <span className={cn("absolute left-0 top-0 bottom-0 w-[3px]", sealColor)} />
      ) : (
        <>
          <span className={cn("absolute left-0 top-0 bottom-0 w-[3px]", sealColor)} />
          <span
            className={cn(
              "absolute left-[-2.5px] top-[10px] size-2 rounded-full",
              sealColor,
            )}
          />
        </>
      )}

      {/* 收藏星标 */}
      {cipher.favorite && (
        <span className="absolute right-2 top-2 text-[11px] leading-none text-foreground/60">
          ★
        </span>
      )}

      {/* 名称（等宽） */}
      <div
        className={cn(
          "font-mono-vault pr-4 text-sm font-medium text-foreground",
          cipher.deleted_at && "line-through",
        )}
      >
        {cipher.name}
      </div>

      {/* 用户名（等宽） */}
      {cipher.login?.username && (
        <div className="font-mono-vault mt-0.5 truncate text-xs text-muted-foreground">
          {cipher.login.username}
        </div>
      )}

      {/* 密码掩码 + 更新时间（掩码等宽） */}
      <div className="mt-1 flex items-center gap-3 text-[11px] text-muted-foreground/80">
        {cipher.login?.password && (
          <span className="font-mono-vault tracking-widest">
            {"•".repeat(Math.min(12, cipher.login.password.length))}
          </span>
        )}
        <span>{relativeTime(cipher.updated_at, t)}</span>
        {cipher.deleted_at && (
          <span className="rounded bg-destructive/10 px-1.5 py-0.5 text-[10px] uppercase tracking-wider text-destructive">
            {t("settings.vault.list.deleted")}
          </span>
        )}
      </div>
    </button>
  );
}

/** 相对时间简表（"2d" / "1w" / "3min"）——纯展示，无 i18n 严格要 求 */
function relativeTime(iso: string, _t: (k: string) => string): string {
  const ts = Date.parse(iso);
  if (Number.isNaN(ts)) return "";
  const diffMs = Date.now() - ts;
  const sec = Math.floor(diffMs / 1000);
  if (sec < 60) return `${sec}s`;
  const min = Math.floor(sec / 60);
  if (min < 60) return `${min}min`;
  const hr = Math.floor(min / 60);
  if (hr < 24) return `${hr}h`;
  const day = Math.floor(hr / 24);
  if (day < 7) return `${day}d`;
  const wk = Math.floor(day / 7);
  if (wk < 4) return `${wk}w`;
  const mo = Math.floor(day / 30);
  if (mo < 12) return `${mo}mo`;
  const yr = Math.floor(day / 365);
  return `${yr}y`;
}
