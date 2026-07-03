import { Component, type ReactNode } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import Result from "@/pages/Result";
import Settings from "@/pages/Settings";
import Clipboard from "@/pages/Clipboard";
import Screenshot from "@/pages/Screenshot";
import CompactEditor from "@/pages/CompactEditor";
import ImagePreview from "@/pages/ImagePreview";

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
          case "image_preview_window":
            return <ImagePreview />;
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
