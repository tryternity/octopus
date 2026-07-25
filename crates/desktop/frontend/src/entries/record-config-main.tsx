// 录屏配置浮窗入口——按 Cmd+Shift+R 弹出，选源（display/window/area）+ 音频开关。
//
// 设计与 password-generator/overlay 同模式：独立 entry 让依赖图隔离，
// 浮窗启动延迟最小化（用户按快捷键期望立即响应）。
// 共用 lib/mountApp（启动逻辑）+ lib/theme + lib/i18n + index.css。
import "@/index.css";
import { mountApp } from "@/lib/mountApp";
import RecordConfig from "@/pages/RecordConfig";

mountApp(<RecordConfig />);
