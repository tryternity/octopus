// Result 窗口（ASR 结果编辑器）入口。依赖闭包：CodeMirror（commands/view/state）+ SvgIcon。
import "@/index.css";
import { mountApp } from "@/lib/mountApp";
import Result from "@/pages/Result";

mountApp(<Result />);
