# casing 统一 Task 5: system_status DTO — 设计规格（spec）

> **Status: ✅ 已实现**（2026-07-27，分支 `feat/record-followup`）。
>
> **本 spec 范围**：system_status 域 5 个 Serialize struct 加 `#[serde(rename_all = "camelCase")]`，前端 SystemPanel.tsx + systemStatusMath.ts + test 同步改 camelCase。
>
> **关联文档**：AGENTS.md「序列化 casing 规范」+ Task 1-3 已完成。

---

## 实现注记（Implementation Notes）

### 2026-07-27 实施完成

| 检查项 | 结果 |
|---|---|
| A1 cargo build | 0 error 0 warning |
| A2 cargo test | desktop 422 passed 0 failed |
| A3 **pnpm build**（含 tsc -b，非 tsc --noEmit） | 0 error（Task 3 教训：用 pnpm build 更严格） |
| A4 前端 SystemPanel + math + test 残留 | 0 |
| A5 后端 5 struct rename_all | 5 |

**偏差**：无。5 struct + 3 前端文件全部精确执行。`ModelMemoryRegistry`（内部 struct 无 Serialize）正确未动。

---

## 0. 改动清单

### 后端（5 struct，单文件 `crates/desktop/src/system_status_commands.rs`）

| struct | 行 | snake_case 字段 |
|---|---|---|
| `ModelMemory` | 15 | `display_name` |
| `ProcessStats` | 23 | `rss_bytes` / `real_bytes` / `cpu_percent` |
| `SystemStats` | 31 | `total_memory_bytes` / `used_memory_bytes` / `cpu_percent` |
| `TimeSeries` | 38 | （无 snake_case 字段，加 rename_all 为未来安全） |
| `SystemStatusSnapshot` | 47 | `sampled_at` |

不动：`ModelMemoryRegistry`（行 60，内部 struct 无 Serialize）

### 前端（3 文件）

| 文件 | 改动 |
|---|---|
| `pages/Settings/SystemPanel.tsx` | 5 个 interface 字段 + ~18 处字段访问 |
| `pages/Settings/systemStatusMath.ts` | 泛型约束 `{ sampled_at: number }` → `{ sampledAt: number }` |
| `pages/Settings/systemStatusMath.test.ts` | 2 处 `sampled_at` |

### 字段映射

| snake | camel |
|---|---|
| `rss_bytes` | `rssBytes` |
| `real_bytes` | `realBytes` |
| `cpu_percent` | `cpuPercent` |
| `total_memory_bytes` | `totalMemoryBytes` |
| `used_memory_bytes` | `usedMemoryBytes` |
| `display_name` | `displayName` |
| `sampled_at` | `sampledAt` |

## 1. 验收

| # | 检查项 | 通过标准 |
|---|---|---|
| A1 | cargo build -p octopus-desktop | 0 error 0 warning |
| A2 | cargo test -p octopus-desktop | 全套无回归 |
| A3 | pnpm build（含 tsc -b） | 0 error（**用 pnpm build 非 tsc --noEmit**，Task 3 教训） |
| A4 | grep 前端 SystemPanel + math + test 内 snake_case 残留 | 0 |
| A5 | 后端 5 struct 都有 rename_all | 5 |
