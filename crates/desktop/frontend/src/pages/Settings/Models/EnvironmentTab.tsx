import { useState, useEffect, useCallback } from "react";
import { invoke } from "@/lib/tauri";
import { Plus, Trash2, Lock } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { useT } from "@/lib/i18n";

const BUILTIN = ["huggingface", "modelscope", "github"];

export default function EnvironmentTab({ showToast }: { showToast: (msg: string) => void }) {
  const t = useT();
  const [vars, setVars] = useState<[string, string][]>([]);
  const [newKey, setNewKey] = useState("");
  const [newValue, setNewValue] = useState("");

  const load = useCallback(async () => {
    try {
      const data = await invoke<[string, string][]>("get_env_vars");
      setVars(data);
    } catch (e) { showToast(t("settings.models.env.loadFailed") + e); }
  }, [showToast]);

  useEffect(() => { load(); }, [load]);

  const handleSave = async (key: string, value: string) => {
    try {
      await invoke("set_env_var", { key, value });
      showToast(t("settings.models.env.saved"));
      load();
    } catch (e) { showToast(t("settings.models.env.saveFailed") + e); }
  };

  const handleDelete = async (key: string) => {
    try {
      const ok = await invoke<boolean>("delete_env_var_cmd", { key });
      if (ok) { showToast(t("settings.models.env.deleted")); load(); }
      else showToast(t("settings.models.env.builtinNoDelete"));
    } catch (e) { showToast(t("settings.models.env.deleteFailed") + e); }
  };

  const handleAdd = async () => {
    if (!newKey.trim()) return;
    try {
      await invoke("set_env_var", { key: newKey.trim(), value: newValue.trim() });
      setNewKey(""); setNewValue("");
      showToast(t("settings.models.env.added"));
      load();
    } catch (e) { showToast(t("settings.models.env.addFailed") + e); }
  };

  return (
    <div className="space-y-1.5">
      <p className="text-[11px] text-muted-foreground/70 mb-2">
        {t("settings.models.env.hintPrefix")} <code className="px-1 py-0.5 rounded bg-muted text-[10px] font-mono">{"{" + t("settings.models.env.hintVar") + "}"}</code> {t("settings.models.env.hintSuffix")}
      </p>
      {vars.map(([key, value]) => (
        <div
          key={key}
          className="group flex items-center gap-2 px-3 py-1.5 rounded-md border border-border/60 bg-surface hover:border-border transition-colors"
        >
          <div className="flex items-center gap-1 w-32 shrink-0">
            <span className="text-xs font-mono font-medium">{key}</span>
            {BUILTIN.includes(key) && <Lock className="w-2.5 h-2.5 text-muted-foreground/40" />}
          </div>
          <input
            autoCapitalize="off"
            autoCorrect="off"
            spellCheck={false}
            className="flex-1 px-2 py-0.5 text-xs rounded border border-transparent bg-transparent hover:border-border focus:border-voice/40 focus:bg-background transition-colors outline-none"
            defaultValue={value}
            onBlur={(e) => { if (e.target.value !== value) handleSave(key, e.target.value); }}
          />
          {!BUILTIN.includes(key) && (
            <Button
              variant="destructive-ghost"
              size="icon-sm"
              className="opacity-0 group-hover:opacity-100"
              onClick={() => handleDelete(key)}
            >
              <Trash2 />
            </Button>
          )}
        </div>
      ))}
      {/* 新增行 */}
      <div className="flex items-center gap-2 px-3 py-1.5 mt-2 pt-2.5 border-t border-border/60">
        <Input
          variant="mono"
          size="sm"
          className="w-32 text-xs"
          placeholder={t("settings.models.env.varName")}
          value={newKey}
          onChange={(e) => setNewKey(e.target.value)}
        />
        <Input
          variant="mono"
          size="full"
          className="text-xs"
          placeholder="https://..."
          value={newValue}
          onChange={(e) => setNewValue(e.target.value)}
        />
        <Button
          variant="ghost"
          size="sm"
          onClick={handleAdd}
        >
          <Plus /> {t("settings.models.env.add")}
        </Button>
      </div>
    </div>
  );
}
