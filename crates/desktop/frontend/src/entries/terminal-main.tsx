// 终端窗口入口。依赖闭包：xterm.js 全套（Task 7 接入）。
// URL query 可含 cwd（shell 启动目录，Rust open_terminal_window 注入）。
//
// Task 6 只搭脚手架（窗口 + entry + vite config），Terminal 组件在 Task 7 实现
// （多 tab + xterm.js + agent 状态徽章）。当前为占位，验证窗口链路打通。
import "@/index.css";
import { mountApp } from "@/lib/mountApp";
import Terminal from "@/pages/Terminal";

mountApp(<Terminal />);
