// Clipboard 窗口入口。依赖闭包：lucide-react（5 icons）+ useClipboardHistory。
import "@/index.css";
import { mountApp } from "@/lib/mountApp";
import Clipboard from "@/pages/Clipboard";

mountApp(<Clipboard />);
