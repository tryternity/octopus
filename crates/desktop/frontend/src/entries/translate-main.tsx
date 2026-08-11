// 截图翻译译文浮窗独立入口。
// 与其他窗口 entry 共用：lib/mountApp（启动逻辑）+ lib/theme + lib/i18n + index.css。
import "@/index.css";
import { mountApp } from "@/lib/mountApp";
import Translate from "@/pages/Translate";

mountApp(<Translate />);
