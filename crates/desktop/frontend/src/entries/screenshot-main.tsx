// 截图窗口独立入口——只加载截图所需依赖（React + tauri api + annotation + Screenshot）。
//
// 设计原因：截图窗口对启动延迟极敏感（用户触发截图后期望立即看到画面）。
// 独立 entry 让依赖边界与产物边界对齐——screenshot chunk 仅含 ~291KB
// （vs 原 1.27MB 主 bundle），截图窗口 ready 时间从 ~3s 降到 <1s。
//
// 与其他窗口 entry 共用：lib/mountApp（启动逻辑）+ lib/theme + lib/i18n + index.css。
import "@/index.css";
import { mountApp } from "@/lib/mountApp";
import Screenshot from "@/pages/Screenshot";

mountApp(<Screenshot />);
