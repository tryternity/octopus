import { Component, type ReactNode, useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import Result from "@/pages/Result";
import Settings from "@/pages/Settings";
import Clipboard from "@/pages/Clipboard";
import Screenshot from "@/pages/Screenshot";
import CompactEditor from "@/pages/CompactEditor";
import ActionBar from "@/pages/ActionBar";
import Overlay from "@/pages/Overlay";
import PasswordGenerator from "@/pages/PasswordGenerator";
import VaultPicker from "@/pages/VaultPicker";
import { applyThemeFromConfig } from "@/lib/theme";

class ErrorBoundary extends Component<
  { children: ReactNode },
  { error: Error | null }
> {
  state: { error: Error | null } = { error: null };

  static getDerivedStateFromError(error: Error) {
    return { error };
  }

  render() {
    if (this.state.error) {
      return (
        <div style={{ padding: 20, color: "red", fontFamily: "monospace", fontSize: 12 }}>
          <h3>React Error:</h3>
          <pre>{this.state.error.message}</pre>
          <pre>{this.state.error.stack}</pre>
        </div>
      );
    }
    return this.props.children;
  }
}

function getWindowLabel(): string {
  try {
    return getCurrentWindow().label;
  } catch {
    return "";
  }
}

function App() {
  const label = getWindowLabel();
  // follow-up #10: vault feature 探针——决定是否渲染 password_generator_window /
  // vault_picker_window 路由。feature off 时这两个窗口根本不会被后端创建
  // （热键不注册、命令不存在），但 mount 阶段同步消费 is_vault_enabled 是稳定契约。
  const [vaultEnabled, setVaultEnabled] = useState<boolean | null>(null);

  // 每个窗口 mount 时应用主题 + 监听 config-changed 同步主题切换。
  // Tauri app_handle.emit 广播到所有窗口，但需每窗口自行 listen。
  useEffect(() => {
    // 异步校正 localStorage 与 DB 的差异（首次运行/清缓存/多窗口不同步时生效）。
    // 不阻塞首屏渲染——restoreCachedTheme 已在 main.tsx 同步完成。
    applyThemeFromConfig();
    const unlisten = listen("config-changed", () => applyThemeFromConfig());
    return () => { unlisten.then((fn) => fn()); };
  }, []);

  // follow-up #10: 拉取 vault feature 状态（永远注册的命令，后端 cfg 反射）。
  // 仅在 password_generator_window / vault_picker_window 标签下需要——其他窗口
  // 不渲染 vault UI，跳过 invoke 以省一次 IPC。
  useEffect(() => {
    if (label !== "password_generator_window" && label !== "vault_picker_window") {
      setVaultEnabled(false);
      return;
    }
    let cancelled = false;
    invoke<boolean>("is_vault_enabled")
      .then((v) => { if (!cancelled) setVaultEnabled(v); })
      .catch(() => { if (!cancelled) setVaultEnabled(false); });
    return () => { cancelled = true; };
  }, [label]);

  return (
    <ErrorBoundary>
      {(() => {
        switch (label) {
          case "result_window":
            return <Result />;
          case "settings_window":
            return <Settings />;
          case "clipboard_window":
            return <Clipboard />;
          case "compact_editor_window":
            return <CompactEditor />;
          case "action_bar_window":
            return <ActionBar />;
          case "overlay_window":
            return <Overlay />;
          case "password_generator_window":
            // follow-up #10: feature off 时后端不会创建此窗口；但 mount 阶段
            // probe 未完成 → 渲染占位（loaded 后通常直接退出此分支）。
            return vaultEnabled ? <PasswordGenerator /> : null;
          case "vault_picker_window":
            return vaultEnabled ? <VaultPicker /> : null;
          default:
            if (label.startsWith("screenshot_")) {
              return <Screenshot />;
            }
            return (
              <div className="p-4 text-foreground">
                <p className="text-sm text-muted-foreground">Window: {label}</p>
              </div>
            );
        }
      })()}
    </ErrorBoundary>
  );
}

export default App;
