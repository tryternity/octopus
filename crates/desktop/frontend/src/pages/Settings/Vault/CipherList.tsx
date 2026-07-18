import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Plus, Search, Star, Trash2 } from "lucide-react";
import { cn } from "@/lib/utils";
import { useT } from "@/lib/i18n";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import CipherEditor from "./CipherEditor";

/**
 * CipherList —— 密码条目列表 + 新建入口。
 *
 * 后端 `vault_list_ciphers` 一次拉全量（含已软删除的）。
 * 搜索过滤：name / username / uri 任一命中。
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
  const [query, setQuery] = useState("");
  const [loaded, setLoaded] = useState(false);
  const [editing, setEditing] = useState<number | "new" | null>(null);

  const refresh = useCallback(async () => {
    try {
      const list = await invoke<CipherDto[]>("vault_list_ciphers");
      setCiphers(list);
    } catch (e) {
      showToast(String(e));
    }
    setLoaded(true);
  }, [showToast]);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return ciphers;
    return ciphers.filter((c) => {
      const uname = c.login?.username ?? "";
      const uris = c.login?.uris ?? [];
      return (
        c.name.toLowerCase().includes(q) ||
        uname.toLowerCase().includes(q) ||
        uris.some((u) => u.uri.toLowerCase().includes(q))
      );
    });
  }, [ciphers, query]);

  if (editing !== null) {
    return (
      <CipherEditor
        cipherId={editing === "new" ? null : editing}
        onClose={async () => {
          setEditing(null);
          await refresh();
        }}
        showToast={showToast}
      />
    );
  }

  return (
    <div className="flex h-full flex-col gap-3">
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
          <p className="py-12 text-center text-sm text-muted-foreground">{t("settings.loading")}</p>
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
                    c.deleted_at ? "bg-muted-foreground/40" : c.favorite ? "bg-amber-500" : "bg-voice/50",
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
                    {c.favorite && <Star className="size-3 shrink-0 fill-amber-500 text-amber-500" />}
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
  );
}
