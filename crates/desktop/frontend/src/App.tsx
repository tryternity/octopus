import { Component, type ReactNode, useEffect } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { listen } from "@tauri-apps/api/event";
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

  // 每个窗口 mount 时应用主题 + 监听 config-changed 同步主题切换。
  // Tauri app_handle.emit 广播到所有窗口，但需每窗口自行 listen。
  useEffect(() => {
    // 异步校正 localStorage 与 DB 的差异（首次运行/清缓存/多窗口不同步时生效）。
    // 不阻塞首屏渲染——restoreCachedTheme 已在 main.tsx 同步完成。
    applyThemeFromConfig();
    const unlisten = listen("config-changed", () => applyThemeFromConfig());
    return () => { unlisten.then((fn) => fn()); };
  }, []);

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
            return <PasswordGenerator />;
          case "vault_picker_window":
            return <VaultPicker />;
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
