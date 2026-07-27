# casing 统一 Task 2: record 命令 DTO — 设计规格（spec）

> **Status: 📝 设计阶段**（2026-07-27，分支 `feat/record-followup`）。
>
> **本 spec 范围**：给 `RecordStatus` + `StartedInfo` 加 `#[serde(rename_all = "camelCase")]`，并同步前端消费点。全工程 casing 统一 roadmap 第 2 个 task（Task 1 已完成 RecordTaskEvent enum）。
>
> **关联文档**：
> - 全工程 casing roadmap：AGENTS.md「序列化 casing 规范」
> - Task 1：`docs/superpowers/specs/2026-07-27-record-task-event-enum.md`

---

## 实现注记（Implementation Notes）

实施过程中与原 spec 的偏差回写至此处。

<!-- 待填 -->

---

## 0. 改动清单

### 后端

| struct | 文件 | 当前字段 | 改动 |
|---|---|---|---|
| `RecordStatus` | `crates/desktop/src/record_commands.rs:839` | `state` / `elapsed_secs` | 加 `#[serde(rename_all = "camelCase")]`（`elapsed_secs` → `elapsedSecs`） |
| `StartedInfo` | `crates/record/src/session.rs:28` | `width` / `height`（单词，无需转） | 加 `#[serde(rename_all = "camelCase")]`（当前字段不受影响，但为未来加字段安全） |

`StartedInfo` 当前字段都是单词（width/height），加 rename_all 实际不改变现有序列化输出。但按 AGENTS.md 规范「命令返回值 DTO 必须显式加 rename_all」，统一加上。

### 前端（3 处 inline type + 2 处字段访问）

| 文件 | 行 | 改动 |
|---|---|---|
| `crates/desktop/frontend/src/pages/RecordControl/index.tsx` | 51 | inline type `elapsed_secs: number` → `elapsedSecs: number` |
| `crates/desktop/frontend/src/pages/RecordControl/index.tsx` | 56 | `status.elapsed_secs` → `status.elapsedSecs` |
| `crates/desktop/frontend/src/pages/RecordAnnotation/index.tsx` | 88 | inline type `elapsed_secs: number` → `elapsedSecs: number` |
| `crates/desktop/frontend/src/pages/RecordAnnotation/index.tsx` | 92 | `status.elapsed_secs` → `status.elapsedSecs` |

`StartedInfo` 前端无消费（grep 确认 record_start 返回值未被读字段），无需前端改动。

## 1. 验收

| # | 检查项 | 通过标准 |
|---|---|---|
| **A1** | `cargo build -p octopus-desktop -p octopus-record` | 0 error 0 warning |
| **A2** | `cargo test -p octopus-desktop -p octopus-record` | 全套无回归 |
| **A3** | `pnpm tsc --noEmit` | 0 error |
| **A4** | grep 确认前端无 `elapsed_secs` 残留 | RecordControl + RecordAnnotation 内 0 个 |
| **A5** | grep 确认后端 struct 有 rename_all | RecordStatus + StartedInfo 各 1 个 `rename_all = "camelCase"` |

## 2. 测试策略

纯字段重命名 + 加 serde 属性，无新逻辑，不写新单测。验收靠 build + tsc + 现有测试套件无回归。

## 3. 不动

- `StoppedInfo`（session.rs:33）——内部 struct，不 Serialize，不需改
- `RecordStatus.state`（String，单词不受影响）
- `StartedInfo.width` / `height`（单词不受影响）
