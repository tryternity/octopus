import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ArrowLeft } from "lucide-react";
import { cn } from "@/lib/utils";
import { useT } from "@/lib/i18n";
import { Button } from "@/components/ui/button";
import { Input, Textarea } from "@/components/ui/input";
import { Toggle as UIToggle } from "@/components/ui/toggle";
import type { CipherDto } from "./CipherList";

/**
 * CipherEditor —— 新建/编辑一条 Login cipher。
 *
 * 字段：name / urls(多行) / username / password / totp / notes / favorite。
 * 保存：新建 → vault_create_cipher；编辑 → vault_update_cipher。
 * 删除：软删除（permanent=false）；已软删时显示「永久删除」（permanent=true）。
 *
 * 后端 CipherInputDto.login 必填，所以即便用户没填任何 login 字段，
 * 也至少传一个空 LoginDataDto（uris/username/password/totp 全可空）。
 */

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
interface CipherInputDto {
  folder_id: number | null;
  favorite: boolean;
  name: string;
  notes: string | null;
  login: LoginDataDto;
  fields: FieldDto[];
  reprompt: number | null;
}

// Toggle 适配：把 ActionBarPanel 范式 (checked, onChange) 转到共享 UIToggle (on, onClick)。
const Toggle = ({ checked, onChange }: { checked: boolean; onChange: (v: boolean) => void }) => (
  <UIToggle on={checked} onClick={() => onChange(!checked)} />
);

const inputCls = "w-full";

export default function CipherEditor({
  cipherId,
  onClose,
  showToast,
}: {
  cipherId: number | null;
  onClose: () => Promise<void>;
  showToast: (msg: string) => void;
}) {
  const t = useT();
  const [name, setName] = useState("");
  const [urls, setUrls] = useState(""); // 多行，每行一个 uri
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [totp, setTotp] = useState("");
  const [notes, setNotes] = useState("");
  const [favorite, setFavorite] = useState(false);
  const [deletedAt, setDeletedAt] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [loaded, setLoaded] = useState(cipherId === null); // 新建默认 loaded

  const loadCipher = useCallback(async () => {
    if (cipherId === null) return;
    try {
      const c = await invoke<CipherDto>("vault_get_cipher", { id: cipherId });
      setName(c.name);
      setUrls((c.login?.uris ?? []).map((u) => u.uri).join("\n"));
      setUsername(c.login?.username ?? "");
      setPassword(c.login?.password ?? "");
      setTotp(c.login?.totp ?? "");
      setNotes(c.notes ?? "");
      setFavorite(c.favorite);
      setDeletedAt(c.deleted_at);
    } catch (e) {
      showToast(String(e));
    }
    setLoaded(true);
  }, [cipherId, showToast]);

  useEffect(() => {
    loadCipher();
  }, [loadCipher]);

  function buildInput(): CipherInputDto {
    const uris = urls
      .split("\n")
      .map((s) => s.trim())
      .filter((s) => s.length > 0)
      .map((uri) => ({ uri, match_type: null }));
    return {
      folder_id: null,
      favorite,
      name: name.trim() || "(untitled)",
      notes: notes.trim() || null,
      login: {
        uris,
        username: username.trim() || null,
        password: password || null,
        totp: totp.trim() || null,
      },
      fields: [],
      reprompt: 0,
    };
  }

  const handleSave = useCallback(async () => {
    if (!name.trim()) {
      showToast(t("settings.vault.editor.nameLabel"));
      return;
    }
    setBusy(true);
    try {
      const input = buildInput();
      if (cipherId === null) {
        await invoke("vault_create_cipher", { input });
      } else {
        await invoke("vault_update_cipher", { id: cipherId, input });
      }
      showToast(t("settings.vault.editor.save"));
      await onClose();
    } catch (e) {
      showToast(String(e));
    } finally {
      setBusy(false);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [cipherId, name, urls, username, password, totp, notes, favorite, showToast, onClose, t]);

  const handleDelete = useCallback(
    async (permanent: boolean) => {
      if (cipherId === null) return;
      setBusy(true);
      try {
        await invoke("vault_delete_cipher", { id: cipherId, permanent });
        showToast(permanent ? t("settings.vault.editor.permanentDelete") : t("settings.vault.editor.delete"));
        await onClose();
      } catch (e) {
        showToast(String(e));
      } finally {
        setBusy(false);
      }
    },
    [cipherId, onClose, showToast, t],
  );

  if (!loaded) {
    return (
      <div className="flex h-full items-center justify-center text-sm text-muted-foreground">
        {t("settings.loading")}
      </div>
    );
  }

  return (
    <div className="flex h-full flex-col gap-3">
      {/* 导航栏 */}
      <div className="flex shrink-0 items-center gap-3 border-b border-border/40 pb-2">
        <button
          onClick={() => onClose()}
          className="flex items-center gap-1.5 text-xs text-muted-foreground transition-colors hover:text-foreground"
        >
          <ArrowLeft className="size-3.5" />
          {t("settings.vault.list.title")}
        </button>
        <span className="font-mono text-[10px] uppercase tracking-widest text-muted-foreground/70">
          {cipherId === null ? "NEW" : "EDIT"}
        </span>
      </div>

      {/* 表单卡片 */}
      <div className="flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto rounded-lg border border-border/50 bg-muted/15 p-4">
        <div className="space-y-1.5">
          <label className="block text-[11px] font-medium uppercase tracking-wide text-muted-foreground/80">
            {t("settings.vault.editor.nameLabel")}
          </label>
          <Input value={name} onChange={(e) => setName(e.target.value)} className={inputCls} autoFocus />
        </div>

        <div className="space-y-1.5">
          <label className="block text-[11px] font-medium uppercase tracking-wide text-muted-foreground/80">
            {t("settings.vault.editor.urlLabel")}
          </label>
          <Textarea
            value={urls}
            onChange={(e) => setUrls(e.target.value)}
            className={cn(inputCls, "min-h-[60px] resize-y font-mono text-xs")}
            placeholder={"https://example.com/login"}
          />
        </div>

        <div className="grid grid-cols-2 gap-3">
          <div className="space-y-1.5">
            <label className="block text-[11px] font-medium uppercase tracking-wide text-muted-foreground/80">
              {t("settings.vault.editor.usernameLabel")}
            </label>
            <Input value={username} onChange={(e) => setUsername(e.target.value)} className={inputCls} />
          </div>
          <div className="space-y-1.5">
            <label className="block text-[11px] font-medium uppercase tracking-wide text-muted-foreground/80">
              {t("settings.vault.editor.passwordLabel")}
            </label>
            <Input
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              className={cn(inputCls, "font-mono")}
              type="text"
              autoComplete="off"
            />
          </div>
        </div>

        <div className="space-y-1.5">
          <label className="block text-[11px] font-medium uppercase tracking-wide text-muted-foreground/80">
            {t("settings.vault.editor.totpLabel")}
          </label>
          <Input
            value={totp}
            onChange={(e) => setTotp(e.target.value)}
            className={cn(inputCls, "font-mono")}
            placeholder="JBSWY3DPEHPK3PXP"
            autoComplete="off"
          />
        </div>

        <div className="space-y-1.5">
          <label className="block text-[11px] font-medium uppercase tracking-wide text-muted-foreground/80">
            {t("settings.vault.editor.notesLabel")}
          </label>
          <Textarea
            value={notes}
            onChange={(e) => setNotes(e.target.value)}
            className={cn(inputCls, "min-h-[60px] resize-y")}
          />
        </div>

        <div className="flex items-center gap-2.5">
          <Toggle checked={favorite} onChange={setFavorite} />
          <span className="text-xs text-muted-foreground">
            {t("settings.vault.editor.favoriteLabel")}
          </span>
        </div>
      </div>

      {/* 底部按钮 */}
      <div className="flex shrink-0 items-center justify-between gap-2.5">
        <div>
          {cipherId !== null && (
            <Button
              variant="destructive-ghost"
              size="sm"
              disabled={busy}
              onClick={() => handleDelete(false)}
            >
              {deletedAt
                ? t("settings.vault.editor.permanentDelete")
                : t("settings.vault.editor.delete")}
            </Button>
          )}
        </div>
        <div className="flex gap-2.5">
          <Button variant="outline" size="sm" onClick={() => onClose()} disabled={busy}>
            {t("settings.vault.editor.cancel")}
          </Button>
          <Button variant="voice" size="sm" onClick={handleSave} disabled={busy}>
            {busy ? "..." : t("settings.vault.editor.save")}
          </Button>
        </div>
      </div>
    </div>
  );
}
