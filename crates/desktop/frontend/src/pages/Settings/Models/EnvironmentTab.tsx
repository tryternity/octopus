import { useState, useEffect, useCallback } from "react";
import { invoke } from "@/lib/tauri";
import { Plus, Trash2, Lock } from "lucide-react";

const BUILTIN = ["huggingface", "modelscope", "github"];

export default function EnvironmentTab({ showToast }: { showToast: (msg: string) => void }) {
  const [vars, setVars] = useState<[string, string][]>([]);
  const [newKey, setNewKey] = useState("");
  const [newValue, setNewValue] = useState("");

  const load = useCallback(async () => {
    try {
      const data = await invoke<[string, string][]>("get_env_vars");
      setVars(data);
    } catch (e) { showToast("加载环境变量失败：" + e); }
  }, [showToast]);

  useEffect(() => { load(); }, [load]);

  const handleSave = async (key: string, value: string) => {
    try {
      await invoke("set_env_var", { key, value });
      showToast("已保存");
      load();
    } catch (e) { showToast("保存失败：" + e); }
  };

  const handleDelete = async (key: string) => {
    try {
      const ok = await invoke<boolean>("delete_env_var_cmd", { key });
      if (ok) { showToast("已删除"); load(); }
      else showToast("内置变量不可删除");
    } catch (e) { showToast("删除失败：" + e); }
  };

  const handleAdd = async () => {
    if (!newKey.trim()) return;
    try {
      await invoke("set_env_var", { key: newKey.trim(), value: newValue.trim() });
      setNewKey(""); setNewValue("");
      showToast("已添加");
      load();
    } catch (e) { showToast("添加失败：" + e); }
  };

  return (
    <div className="space-y-3">
      <p className="text-xs text-muted-foreground">
        模型下载地址中的 {"{变量名}"} 会自动替换为此处配置的值。内置变量不可删除。
      </p>
      {vars.map(([key, value]) => (
        <div key={key} className="flex items-center gap-2">
          <div className="flex items-center gap-1 w-36 shrink-0">
            <span className="text-xs font-mono font-medium">{key}</span>
            {BUILTIN.includes(key) && <Lock className="w-3 h-3 text-muted-foreground/50" />}
          </div>
          <input
            className="flex-1 px-2 py-1 text-xs rounded border border-border bg-background"
            defaultValue={value}
            onBlur={(e) => { if (e.target.value !== value) handleSave(key, e.target.value); }}
          />
          {!BUILTIN.includes(key) && (
            <button
              className="p-1 rounded hover:bg-destructive/10 text-muted-foreground hover:text-destructive"
              onClick={() => handleDelete(key)}
            >
              <Trash2 className="w-3 h-3" />
            </button>
          )}
        </div>
      ))}
      <div className="flex items-center gap-2 pt-2 border-t border-border">
        <input
          className="w-36 px-2 py-1 text-xs rounded border border-border"
          placeholder="变量名"
          value={newKey}
          onChange={(e) => setNewKey(e.target.value)}
        />
        <input
          className="flex-1 px-2 py-1 text-xs rounded border border-border"
          placeholder="https://..."
          value={newValue}
          onChange={(e) => setNewValue(e.target.value)}
        />
        <button
          className="flex items-center gap-1 px-2 py-1 text-xs rounded bg-foreground/5 hover:bg-foreground/10"
          onClick={handleAdd}
        >
          <Plus className="w-3 h-3" /> 添加
        </button>
      </div>
    </div>
  );
}
