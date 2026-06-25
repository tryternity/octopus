import { getCurrentWindow } from "@tauri-apps/api/window";
import Result from "@/pages/Result";

function App() {
  const label = getCurrentWindow().label;
  switch (label) {
    case "result_window":
      return <Result />;
    default:
      return (
        <div className="p-4 text-foreground">
          <p className="text-sm text-muted-foreground">Window: {label}</p>
        </div>
      );
  }
}

export default App;
