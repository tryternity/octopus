# RecordTaskEvent enum — 设计规格（spec）

> **Status: 📝 设计阶段**（2026-07-27，分支 `feat/record-followup`）。
>
> **本 spec 范围**：把 gif + merge 6 个独立 Tauri 事件合并为 1 个 `record://task` 事件 + `RecordTaskEvent` enum payload。这是「全工程 casing 统一」roadmap 的第一个 task（详见 AGENTS.md「序列化 casing 规范」）。
>
> **不在本 spec 范围**：命令返回值 DTO 统一（Task 2-10）、`MergeResult` 改动（已是 camelCase）、前端 listener（当前无，未来如需监听用单 `listen("record://task")`）。
>
> **关联文档**：
> - 全工程 casing roadmap：AGENTS.md「序列化 casing 规范」
> - 上游 spec：`docs/superpowers/specs/2026-07-27-screen-record-audio-post-merge.md`（P2 项）

---

## 实现注记（Implementation Notes）

实施过程中与原 spec 的偏差回写至此处（实施时填充）。

<!-- 待填 -->

---

## 0. 决策回顾

### 0.1 问题陈述

当前录屏的 GIF 导出和音轨合并各自 emit 3 个事件（started/done/failed），共 6 个事件名：
- `record://gif-started` / `record://gif-done` / `record://gif-failed`
- `record://merge-started` / `record://merge-done` / `record://merge-failed`

payload 都是 `serde_json::json!({...})` 手写字面量，字段名不统一（gif-done 用 `path`，merge-done 用 `new_id` + `path`），且与 `MergeResult` struct 解耦。

### 0.2 brainstorming 决策清单

| 维度 | 决策 | 理由 |
|---|---|---|
| **事件名** | 合并为 1 个 `record://task` | 减少事件名数量；前端未来单 `listen` + switch 即可 |
| **payload 形式** | `RecordTaskEvent` enum（内部 tagged） | 与 `HelperEvent`（`record://event`）同模式，项目已有先例 |
| **变体名 casing** | kebab-case（`gif-started` / `merge-done`） | 外层 `#[serde(rename_all = "kebab-case")]`，与 HelperEvent 一致 |
| **字段 casing** | camelCase（`newId` / `filePath`） | 每个变体 `#[serde(rename_all = "camelCase")]`，遵循 AGENTS.md「序列化 casing 规范」 |
| **enum 位置** | `crates/desktop/src/record_commands.rs`（与 `MergeResult` 同文件） | 命令层事件，与 emit 调用点同文件 |
| **前端改动** | 无（前端不监听这些事件，零破坏） | grep 确认前端 0 个 `record://gif-*` / `record://merge-*` listener |

### 0.3 排除的方案

| 方案 | 排除理由 |
|---|---|
| 3 个独立 event struct（MergeStartedEvent 等） | struct 多，与 GIF 事件模式不一致（GIF 也是手写 json） |
| 1 个 enum + 1 个 payload struct（非 tagged） | 前端解构多一层（`payload.event` + `payload.data.xxx`），不如 tagged 扁平 |
| 字段 snake_case（跟 HelperEvent） | 与 AGENTS.md「事件 payload camelCase」规范矛盾，未来还是要改 |
| Rust 字段直接写 camelCase | 违反 Rust 命名惯例，clippy warn（non_snake_case） |

---

## 1. 设计

### 1.1 RecordTaskEvent enum 定义

```rust
/// 录屏异步任务（GIF 导出 / 音轨合并）的进度事件。
///
/// 统一替代原 `record://gif-{started,done,failed}` + `record://merge-{started,done,failed}`
/// 6 个事件名。前端未来如需监听，单个 `listen("record://task", ...)` + switch(payload.event) 即可。
///
/// 与 HelperEvent（record://event）同模式：内部 tagged enum。
/// 变体名 kebab-case（外层 rename_all），字段 camelCase（变体级 rename_all）——
/// 遵循 AGENTS.md「序列化 casing 规范」。
#[derive(serde::Serialize, Clone, Debug)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub enum RecordTaskEvent {
    #[serde(rename_all = "camelCase")]
    GifStarted { id: i64 },
    #[serde(rename_all = "camelCase")]
    GifDone { id: i64, path: String },
    #[serde(rename_all = "camelCase")]
    GifFailed { id: i64, error: String },
    #[serde(rename_all = "camelCase")]
    MergeStarted { id: i64 },
    #[serde(rename_all = "camelCase")]
    MergeDone { id: i64, new_id: i64, path: String },
    #[serde(rename_all = "camelCase")]
    MergeFailed { id: i64, error: String },
}
```

序列化示例：
```json
{"event":"gif-started","id":1}
{"event":"gif-done","id":1,"path":"/x.gif"}
{"event":"merge-done","id":1,"newId":2,"path":"/x_merged.mp4"}
{"event":"merge-failed","id":1,"error":"ffmpeg amix 失败"}
```

### 1.2 emit 改动（6 处）

**export_gif（`record_commands.rs:863-914`）3 处**：

| 行 | 原 | 新 |
|---|---|---|
| 885 | `app.emit("record://gif-started", serde_json::json!({ "id": id }))` | `app.emit("record://task", &RecordTaskEvent::GifStarted { id })` |
| 901-904 | `app.emit("record://gif-failed", serde_json::json!({ "id": id, "error": "ffmpeg 转 GIF 失败" }))` | `app.emit("record://task", RecordTaskEvent::GifFailed { id, error: "ffmpeg 转 GIF 失败".into() })` |
| 909-912 | `app.emit("record://gif-done", serde_json::json!({ "id": id, "path": path_str }))` | `app.emit("record://task", RecordTaskEvent::GifDone { id, path: path_str.clone() })`（path_str 还要 return，clone） |

**merge_audio_tracks（`record_commands.rs:926-1100`）3 处**：

| 行 | 原 | 新 |
|---|---|---|
| 978 | `app.emit("record://merge-started", serde_json::json!({ "id": id }))` | `app.emit("record://task", RecordTaskEvent::MergeStarted { id })` |
| 1007-1010 | `app.emit("record://merge-failed", serde_json::json!({ "id": id, "error": "ffmpeg amix 失败" }))` | `app.emit("record://task", RecordTaskEvent::MergeFailed { id, error: "ffmpeg amix 失败".into() })` |
| 1093-1096 | `app.emit("record://merge-done", serde_json::json!({ "id": id, "new_id": new_id, "path": file_path_str }))` | `app.emit("record://task", RecordTaskEvent::MergeDone { id, new_id, path: file_path_str })` |

### 1.3 改动文件清单

| 文件 | 改动 |
|---|---|
| `crates/desktop/src/record_commands.rs` | 加 `RecordTaskEvent` enum + 6 处 emit 替换 |

**不动**：前端、`record://event`（HelperEvent）、`record://stopped` / `record://stop-failed`、`MergeResult` struct。

---

## 2. 验收标准

| # | 检查项 | 通过标准 |
|---|---|---|
| **A1** | `cargo build -p octopus-desktop` | 0 error 0 warning |
| **A2** | `cargo test -p octopus-desktop` | 全套无回归 |
| **A3** | grep 确认无残留 | `record_commands.rs` 内无 `record://gif-started` / `record://gif-done` / `record://gif-failed` / `record://merge-started` / `record://merge-done` / `record://merge-failed` 字符串 |
| **A4** | grep 确认 enum 用了 | `record_commands.rs` 内 `RecordTaskEvent::` 出现 6 次（每处 emit 一次） |

---

## 3. 测试策略

本 task 是纯重构（手写 json → enum），**无新逻辑**，不写新单测。验收靠 build + 现有测试套件无回归。

序列化正确性由 serde 保证（变体级 rename_all 实测有效，见 brainstorming 阶段验证）。
