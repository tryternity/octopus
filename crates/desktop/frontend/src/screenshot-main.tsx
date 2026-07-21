// 截图窗口独立入口——只加载截图所需依赖（React + tauri api + annotation + Screenshot）。
//
// 设计原因：截图窗口对启动延迟极敏感（用户触发截图后期望立即看到画面）。
// 主入口 index.html 的 bundle 包含 CodeMirror/markdown-it/lucide-react 等
// 编辑器/列表域的依赖（~1.27MB），但截图窗口根本不使用。独立 entry 让依赖
// 边界与产物边界对齐——screenshot chunk 仅含 ~200KB，截图窗口 ready 时间
// 从 ~3s（含 force show 兜底）降到 <1s。
//
// 与主入口共用：lib/theme（主题恢复）、lib/i18n（locale 缓存 + 后台校正）、
// index.css（Tailwind + 主题变量）、lib/tauri（invoke wrapper）。
import { createRoot } from "react-dom/client";
import "./index.css";
import Screenshot from "@/pages/Screenshot";
import { restoreCachedTheme } from "@/lib/theme";
import { restoreCachedLocale, initI18n } from "@/lib/i18n";

// 启动时同步恢复本地状态（零 IPC，与主入口 main.tsx 同范式）
restoreCachedTheme();
restoreCachedLocale();

const root = createRoot(document.getElementById("root")!);
root.render(<Screenshot />);

// 后台 IPC 校正 locale（DB 改了语言时同步），不阻塞渲染
initI18n().catch(() => {});
