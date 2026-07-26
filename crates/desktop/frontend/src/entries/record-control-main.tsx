// 录制控制浮窗入口——display/window 录制时桌面右下角显示的 pill。
// 与 RecordAnnotation 互斥（Area 录制用 RecordAnnotation，display/window 用本浮窗）。
import "@/index.css";
import { mountApp } from "@/lib/mountApp";
import RecordControl from "@/pages/RecordControl";

mountApp(<RecordControl />);
