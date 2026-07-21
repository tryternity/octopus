// CompactEditor 窗口（统一查看器）入口。依赖闭包：CodeMirror 全套 + lezer + markdown-it + ImagePreview。
// URL query 可含 itemId/source/itemType（首 tab 经 URL 注入，零 IPC 打开）。
import "@/index.css";
import { mountApp } from "@/lib/mountApp";
import CompactEditor from "@/pages/CompactEditor";

mountApp(<CompactEditor />);
