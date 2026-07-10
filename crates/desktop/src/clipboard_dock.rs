//! 剪贴板浮窗 dock（吸附收缩）NSWindow 操作。
//!
//! 收缩/展开不使用 setIgnoresMouseEvents——它会阻挡整个窗口的鼠标事件，
//! 导致 8px 细条无法接收 hover。改为纯 CSS pointer-events 控制：
//! 收缩态容器 pointer-events: none + 细条 pointer-events: auto。
//! macOS WKWebView 在 transparent 窗口上，pointer-events: none 的区域
//! 会将鼠标事件透传给下层窗口。

// 预留：未来如需 NSWindow 级操作（如 NSTrackingArea），在此扩展。
