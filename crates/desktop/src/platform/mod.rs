//! 平台/输入辅助功能域：app_context（AX/ATSPI/UIA）+ 输入源 + 键盘 + 粘贴 + 系统打开 + 激活 + 焦点追踪。

pub mod app_context;
pub mod input_source;
pub mod finder_selection;
pub mod keystroke;
pub mod paste;
pub mod sys_open;
pub mod activation;
pub mod focus_tracker;
pub mod ptt;
