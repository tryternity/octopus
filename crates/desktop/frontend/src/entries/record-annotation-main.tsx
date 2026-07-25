// 录屏标注 overlay 入口——录屏开始后显示，用户画标注被 SCK 录进视频。
// 普通 level（非 always_on_top），SCK 录窗口 buffer。
import "@/index.css";
import { mountApp } from "@/lib/mountApp";
import RecordAnnotation from "@/pages/RecordAnnotation";

mountApp(<RecordAnnotation />);
