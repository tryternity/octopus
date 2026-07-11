import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import path from "path";

// 单一 config：vitest 自动读 vite.config.ts 的 test 字段。
// 此前 vitest.config.ts 与本文件并存时，vitest 4 加载了本文件（无 test 段 → 默认 node
// 环境）导致 DOM 测试（i18n/markdown/caret）document is not defined。合并到一处消除冲突。
export default defineConfig({
  plugins: [react(), tailwindcss()],
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
    port: 1420,
  },
  test: {
    environment: "jsdom",
    include: ["src/**/*.test.ts", "src/**/*.test.tsx"],
  },
});
