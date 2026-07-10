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
    <div className="space-y-1.5 max-w-[560px]">
      <p className="text-[11px] text-muted-foreground/70 mb-2">
        模型下载地址中的 <code className="px-1 py-0.5 rounded bg-muted text-[10px] font-mono">{"{变量名}"}</code> 会自动替换为此处配置的值
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
            className="flex-1 px-2 py-0.5 text-xs rounded border border-transparent bg-transparent hover:border-border focus:border-voice/40 focus:bg-background transition-colors outline-none"
            defaultValue={value}
            onBlur={(e) => { if (e.target.value !== value) handleSave(key, e.target.value); }}
          />
          {!BUILTIN.includes(key) && (
            <button
              className="p-1 rounded opacity-0 group-hover:opacity-100 hover:bg-destructive/10 text-muted-foreground hover:text-destructive transition-all"
              onClick={() => handleDelete(key)}
            >
              <Trash2 className="w-3 h-3" />
            </button>
          )}
        </div>
      ))}
      {/* 新增行 */}
      <div className="flex items-center gap-2 px-3 py-1.5 mt-2 pt-2.5 border-t border-border/60">
        <input
          className="w-32 px-2 py-0.5 text-xs rounded border border-border bg-background outline-none focus:border-voice/40 transition-colors"
          placeholder="变量名"
          value={newKey}
          onChange={(e) => setNewKey(e.target.value)}
        />
        <input
          className="flex-1 px-2 py-0.5 text-xs rounded border border-border bg-background outline-none focus:border-voice/40 transition-colors"
          placeholder="https://..."
          value={newValue}
          onChange={(e) => setNewValue(e.target.value)}
        />
        <button
          className="flex items-center gap-1 px-2 py-0.5 text-[11px] rounded bg-foreground/5 hover:bg-foreground/10 text-foreground transition-colors"
          onClick={handleAdd}
        >
          <Plus className="w-3 h-3" /> 添加
        </button>
      </div>
    </div>
  );
}
