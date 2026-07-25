// 录屏区域选区 picker 入口——多屏全屏透明覆盖，用户拖框选区域。
//
// 与 screenshot 选区浮窗同模式（独立 entry 让依赖图隔离），但：
// - 不加载截图 RGBA（半透明黑遮罩）
// - 拖完即确认（mouseup 立即调 confirm_record_area_picker，不显示标注工具）
// - Esc/右键取消
import "@/index.css";
import { mountApp } from "@/lib/mountApp";
import AreaPicker from "@/pages/AreaPicker";

mountApp(<AreaPicker />);
