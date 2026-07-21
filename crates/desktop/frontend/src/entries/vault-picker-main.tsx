// VaultPicker 窗口入口。依赖闭包：lucide-react + password-input + input + button。
//
// vault feature 探针：feature off 时窗口根本不会被后端创建（热键不注册、命令不存在），
// 但 mount 阶段同步消费 is_vault_enabled 是稳定契约——为防御性，仍拉一次确认。
// 逻辑从原 App.tsx 迁移，保持完全一致。
import "@/index.css";
import { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { mountApp } from "@/lib/mountApp";
import VaultPicker from "@/pages/VaultPicker";

function VaultPickerGate() {
  const [enabled, setEnabled] = useState<boolean | null>(null);
  useEffect(() => {
    let cancelled = false;
    invoke<boolean>("is_vault_enabled")
      .then((v) => { if (!cancelled) setEnabled(v); })
      .catch(() => { if (!cancelled) setEnabled(false); });
    return () => { cancelled = true; };
  }, []);
  // enabled=null 加载中（渲染 null 避免 flash）；enabled=false 不该发生（窗口不会被创建）
  return enabled ? <VaultPicker /> : null;
}

mountApp(<VaultPickerGate />);
