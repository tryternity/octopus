import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import yaml from "@modyfi/vite-plugin-yaml";
import path from "path";

// 单一 config：vitest 自动读 vite.config.ts 的 test 字段。
// 此前 vitest.config.ts 与本文件并存时，vitest 4 加载了本文件（无 test 段 → 默认 node
// 环境）导致 DOM 测试（i18n/markdown/caret）document is not defined。合并到一处消除冲突。
//
// vitest 4 的 defineConfig 类型在 server.clearScreen 上与 vite 8 的 ServerOptions 冲突
//（vitest 重导出了一份 ServerOptions$1 不含 clearScreen），运行时 vite 仍正确处理该字段。
// 用对象字面量 + 显式类型注释绕开重载推断。
const config = {
  plugins: [react(), tailwindcss(), yaml()],
  base: "./",
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
  },
  build: {
    outDir: "../dist",
    emptyOutDir: true,
    chunkSizeWarningLimit: 2000,
  },
  server: {
    // Tauri dev 模式期望固定端口——devUrl 指向这里，strictPort 避免被占时
    // vite 静默改用 1421 导致 Tauri 连不上。
    port: 1420,
    strictPort: true,
    // 不清屏，保留 cargo run 的 stdout 日志。
    clearScreen: false,
  },
  test: {
    environment: "jsdom",
    include: ["src/**/*.test.ts", "src/**/*.test.tsx"],
  },
};

export default defineConfig(config as never);
