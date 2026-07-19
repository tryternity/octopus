/// <reference types="vitest" />
import { defineConfig } from "vite";
import type { UserConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import yaml from "@modyfi/vite-plugin-yaml";
import path from "path";

// 单一 config：vitest 自动读 vite.config.ts 的 test 字段。
// 此前 vitest.config.ts 与本文件并存时，vitest 4 加载了本文件（无 test 段 → 默认 node
// 环境）导致 DOM 测试（i18n/markdown/caret）document is not defined。合并到一处消除冲突。
//
// 注意：defineConfig 从 "vite" 导入，不从 "vitest/config"。
// vitest 4 重导出的 ViteUserConfig 与 vite 8 的 ServerOptions 类型不兼容——
// 会把 server.clearScreen 标为未知属性（No overload matches this call）。
// vitest 4 通过 /// <reference types="vitest" /> 注入 test 字段类型 +
// 运行时自动识别 vite.config.ts 中的 test 字段。
//
// vite 8 把 clearScreen 从 server 子字段提升到顶层 UserConfig.clearScreen
// （vite 7→8 breaking change）。
//
// vitest 的 test 字段在 tsconfig.node.json types:["node"] 下不会被自动识别，
// 这里通过 union 类型断言补上（vitest 运行时会识别此字段）。
export default defineConfig({
  plugins: [react(), tailwindcss(), yaml()],
  base: "./",
  // 不清屏，保留 cargo run 的 stdout 日志（vite 8：顶层字段，不再属于 server）。
  clearScreen: false,
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
  },
  test: {
    environment: "jsdom",
    include: ["src/**/*.test.ts", "src/**/*.test.tsx"],
  },
} as UserConfig & { test: { environment: string; include: string[] } });
