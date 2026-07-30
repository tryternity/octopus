/**
 * 终端窗口主组件（Task 6 占位，Task 7 完整实现）。
 *
 * Task 6 只验证窗口链路（HTML entry → vite → Rust window），
 * 显示 URL query 里的 cwd 确认 Rust → 前端的参数传递打通。
 *
 * Task 7 将替换为：多 tab + xterm.js + pty-bridge + agent 状态徽章。
 */
import { useEffect, useState } from "react";
import { useT } from "@/lib/i18n";

export default function Terminal() {
  const t = useT();
  const [cwd, setCwd] = useState<string | null>(null);

  useEffect(() => {
    const params = new URLSearchParams(window.location.search);
    setCwd(params.get("cwd"));
  }, []);

  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        height: "100vh",
        fontFamily: "var(--font-mono, monospace)",
      }}
    >
      <div
        style={{
          padding: "16px 20px",
          opacity: 0.6,
          fontSize: 13,
          borderBottom: "1px solid var(--border, rgba(255,255,255,0.08))",
        }}
      >
        {t("terminal.loading")}
        {cwd && (
          <span style={{ marginLeft: 12, opacity: 0.7 }}>cwd: {cwd}</span>
        )}
      </div>
      <div style={{ flex: 1, minHeight: 0 }} />
    </div>
  );
}
