import { getCurrentWindow } from "@tauri-apps/api/window";
import Result from "@/pages/Result";
import Settings from "@/pages/Settings";
import Clipboard from "@/pages/Clipboard";

function App() {
  const label = getCurrentWindow().label;
  switch (label) {
    case "result_window":
      return <Result />;
    case "settings_window":
      return <Settings />;
    case "clipboard_window":
      return <Clipboard />;
    default:
      return (
        <div className="p-4 text-foreground">
          <p className="text-sm text-muted-foreground">Window: {label}</p>
        </div>
      );
  }
}

export default App;
