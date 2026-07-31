// InstantOverlay 窗口入口（talk / PTT 模式指示浮窗）。
// 依赖闭包：仅 React + @tauri-apps/api/event（极简，无 CodeMirror / lucide）。
import "@/index.css";
import { mountApp } from "@/lib/mountApp";
import InstantOverlay from "@/pages/InstantOverlay";

mountApp(<InstantOverlay />);
