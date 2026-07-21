// 所有窗口 entry 的统一挂载入口。
//
// 设计目的：每窗口独立 HTML + main.tsx 后，把跨窗口共用的启动逻辑抽到一处：
//   - 同步恢复 theme + locale（零 IPC，零延迟首屏）
//   - 后台 initI18n / applyThemeFromConfig 做内存→DB 校正
//   - listen config-changed / locale-changed 跨窗口事件
//   - ErrorBoundary 兜底
//
// 每个 `xxx-main.tsx` 只需：mountApp(<Page/>)
//
// 与原 main.tsx + App.tsx 的差异：label switch 路由不再需要（每窗口直接渲染
// 自己的 page）。vault feature probe 留给 vault 相关窗口自己的 main.tsx
// 在组件内 useEffect 处理（避免所有窗口都拉 is_vault_enabled）。
import { Component, type ReactNode } from "react";
import { createRoot } from "react-dom/client";
import { listen } from "@tauri-apps/api/event";
import { restoreCachedTheme, applyThemeFromConfig } from "@/lib/theme";
import { restoreCachedLocale, initI18n } from "@/lib/i18n";

class ErrorBoundary extends Component<
  { children: ReactNode },
  { error: Error | null }
> {
  state: { error: Error | null } = { error: null };

  static getDerivedStateFromError(error: Error) {
    return { error };
  }

  render() {
    if (this.state.error) {
      return (
        <div style={{ padding: 20, color: "red", fontFamily: "monospace", fontSize: 12 }}>
          <h3>React Error:</h3>
          <pre>{this.state.error.message}</pre>
          <pre>{this.state.error.stack}</pre>
        </div>
      );
    }
    return this.props.children;
  }
}

/**
 * 挂载一个窗口的根节点。所有窗口共用此入口。
 *
 * 同步部分（render 前）：
 *   - restoreCachedTheme / restoreCachedLocale：从 localStorage 恢复，零 IPC
 *
 * 异步部分（render 后，不阻塞首屏）：
 *   - initI18n：从 get_config IPC 校正 locale，监听 locale-changed
 *   - applyThemeFromConfig：从 get_theme_id IPC 校正主题
 *   - listen config-changed：跨窗口主题切换同步
 */
export function mountApp(node: ReactNode) {
  // 同步恢复（与 main.tsx 原顺序一致）
  restoreCachedTheme();
  restoreCachedLocale();

  // 先渲染——首屏立即可见，locale 已从 localStorage 恢复为正确值。
  const root = createRoot(document.getElementById("root")!);
  root.render(<ErrorBoundary>{node}</ErrorBoundary>);

  // 后台 IPC 校正（不阻塞渲染），与缓存不一致时 setLocale/applyThemeById
  // 会触发订阅的组件重渲染。
  initI18n().catch(() => {});
  applyThemeFromConfig().catch(() => {});

  // 跨窗口主题切换：Settings 改主题后 emit("config-changed")，每窗口校正。
  // unlisten 不显式清理——窗口关闭时随 WebView 销毁（与原 App.tsx 行为一致）。
  listen("config-changed", () => applyThemeFromConfig()).catch(() => {});
}
