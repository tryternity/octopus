/**
 * 平台检测工具（前端条件渲染用）。
 *
 * navigator.platform 在 Tauri WKWebView 可靠返回标准值：
 *   macOS → "MacIntel"
 *   Windows → "Win32"
 *   Linux → "Linux x86_64" / "Linux armv7l" 等
 *
 * Tauri 不覆盖 navigator API，这是 Web 标准。零依赖，同步。
 * 如需更权威的检测可用 @tauri-apps/plugin-os（后端 main.rs:270 已注册），
 * 但需加前端依赖 + async，对「隐藏 tab」场景过度。
 */
export const isMac = navigator.platform.startsWith("Mac");
export const isWindows = navigator.platform.startsWith("Win");
export const isLinux = navigator.platform.startsWith("Linux");
