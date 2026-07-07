import { Component, type ReactNode, useEffect } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { listen } from "@tauri-apps/api/event";
import Result from "@/pages/Result";
import Settings from "@/pages/Settings";
import Clipboard from "@/pages/Clipboard";
import Screenshot from "@/pages/Screenshot";
import CompactEditor from "@/pages/CompactEditor";
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
    applyThemeFromConfig();
    const unlisten = listen("config-changed", () => applyThemeFromConfig());
    // compact_editor + settings 创建时 visible(false)，前端渲染完毕后 show。
    // 消除 WebView 加载期间的空白窗口/PPT slide 效果。
    if (label === "compact_editor_window" || label === "settings_window") {
      // requestAnimationFrame 等 DOM 首帧绘制完再 show
      requestAnimationFrame(() => {
        getCurrentWindow().show().catch(() => {});
      });
    }
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
