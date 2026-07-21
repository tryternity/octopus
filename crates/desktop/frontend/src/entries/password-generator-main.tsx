// PasswordGenerator 独立浮窗入口。依赖闭包：复用 components/PasswordGenerator
// （抽自 Settings/Vault），含 lucide + password-input。
//
// vault feature 探针（与 vault-picker-main 同范式）：防御性拉 is_vault_enabled。
import "@/index.css";
import { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { mountApp } from "@/lib/mountApp";
import PasswordGeneratorWindow from "@/pages/PasswordGenerator";

function PasswordGeneratorGate() {
  const [enabled, setEnabled] = useState<boolean | null>(null);
  useEffect(() => {
    let cancelled = false;
    invoke<boolean>("is_vault_enabled")
      .then((v) => { if (!cancelled) setEnabled(v); })
      .catch(() => { if (!cancelled) setEnabled(false); });
    return () => { cancelled = true; };
  }, []);
  return enabled ? <PasswordGeneratorWindow /> : null;
}

mountApp(<PasswordGeneratorGate />);
