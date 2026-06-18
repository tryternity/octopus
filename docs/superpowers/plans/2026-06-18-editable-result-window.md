# 结果展示区可编辑 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在录音会话进行中允许用户编辑结果展示区文本（双击/按钮进入，快捷键/按钮/失焦退出），编辑期间 ASR 硬暂停；编辑后的文本作为后续展示与润色的基准，新识别文本追加其上，停止粘贴时保留编辑。

**Architecture:** `Transcript` 新增 `edited` 字段，作为 `edited ≻ polished ≻ raw` 分层优先级的最高层；`display_text()` 统一为 `committed + increase`。编辑是 coordinator 主循环里的一个 `editing` 标志——置位时两个 tick handler 跳过喂引擎、只排空丢弃音频（硬暂停）。提交时 `commit_edit` 写回 transcript 并直接 `UPDATE edited_text`（行已存在，无需贯通 do_paste）。停止路径的粘贴输入从 `db_text()` 改为 `display_text()`，使编辑保留到粘贴。

**Tech Stack:** Rust（crates: infra, asr, desktop）/ Tauri webview / 原生 HTML+JS（`dist/result/index.html`）/ SQLite（rusqlite）。

参考 spec：[`docs/superpowers/specs/2026-06-18-editable-result-window-design.md`](../specs/2026-06-18-editable-result-window-design.md)。

---

## File Structure

| 文件 | 职责 | 改动 |
|---|---|---|
| `crates/desktop/src/transcript.rs` | 三文本状态机 | 加 `edited` 字段 + `commit_edit` + `display_text` 优先级链 + `edited_text`/`has_edit` |
| `crates/infra/src/db.sql` | DDL | `transcriptions` 加 `edited_text TEXT` 列 |
| `crates/infra/src/db.rs` | DB 访问 | 加 `update_edited_text`；`TranscriptionRecord` + `list_transcriptions_at` 加 `edited_text` |
| `crates/desktop/src/coordinator.rs` | 状态机主循环 | `Command`/`DbCommand` 加变体；`editing`+`edit_buffer` 闸门；commit→DB；停止路径用 display |
| `crates/desktop/src/main.rs` | Tauri 命令注册 | `invoke_handler` 注册 3 命令 |
| `crates/desktop/dist/result/index.html` | 结果窗前端 | contenteditable + 双击/按钮/Cmd+Enter/blur + 编辑指示 |
| `crates/desktop/dist/result/icons/edit.svg` | 编辑按钮图标 | 新建 |
| `docs/configuration.md` / `docs/architecture.md` | 文档 | 同步 |

---

## Task 1: Transcript 三文本分层模型

**Files:**
- Modify: `crates/desktop/src/transcript.rs`

纯逻辑、单文件、完全可单测。是整个特性的地基。`edited` 为空时 `display_text()` 与现有行为等价（`full[..raw_len] + full[raw_len..] = full`；polished 非空时 = polished + increase），现有测试基本不破。

- [ ] **Step 1: 写失败测试 —— commit_edit 与优先级**

在 `transcript.rs` 的 `#[cfg(test)] mod tests` 末尾追加：

```rust
#[test]
fn commit_edit_sets_edited_and_advances_boundary() {
    let mut t = Transcript::new(30, PolishMode::Intermediate);
    t.set_full("你好世界");
    t.snapshot_for_polish();
    t.on_polish_done("你好，世界。".into());
    assert_eq!(t.display_text(), "你好，世界。");

    // 用户把润色结果改掉
    t.commit_edit("你好世界（手改）");
    assert_eq!(t.edited_text(), Some("你好世界（手改）"));
    assert!(t.has_edit());
    // raw_len 推进到 full 末尾 → increase 清空
    assert_eq!(t.display_text(), "你好世界（手改）");
}

#[test]
fn commit_edit_preserves_raw_and_appends_new() {
    let mut t = Transcript::new(31, PolishMode::Intermediate);
    t.set_full("原文");
    t.commit_edit("原文（手改）");
    // raw（full）原样保留
    assert_eq!(t.full(), "原文");
    // 继续说 → 新增追加在 edited 之后
    t.set_full("原文新增");
    assert_eq!(t.display_text(), "原文（手改）新增");
}

#[test]
fn edited_takes_priority_over_polished_and_raw() {
    let mut t = Transcript::new(32, PolishMode::Intermediate);
    t.set_full("raw文本");
    t.snapshot_for_polish();
    t.on_polish_done("polished文本".into());
    t.commit_edit("edited文本".into());
    assert_eq!(t.display_text(), "edited文本"); // edited ≻ polished ≻ raw
}

#[test]
fn empty_commit_clears_edit_falls_back() {
    let mut t = Transcript::new(33, PolishMode::Intermediate);
    t.set_full("原文");
    t.commit_edit("手改".into());
    assert!(t.has_edit());
    t.commit_edit("");
    assert!(!t.has_edit());
    assert_eq!(t.edited_text(), None);
    assert_eq!(t.display_text(), "原文"); // 回退 raw
}

#[test]
fn polish_input_after_edit_is_edited_plus_increase() {
    // 验证 spec §5/§Q3：编辑后续润色输入 = edited + increase
    let mut t = Transcript::new(34, PolishMode::Intermediate);
    t.set_full("原文");
    t.commit_edit("原文（手改）".into());
    t.set_full("原文新增");
    // 停顿快照：润色输入应 = display = edited + increase
    let snap = t.snapshot_for_polish();
    assert_eq!(snap, "原文（手改）新增");
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p octopus-desktop transcript::tests`
Expected: 编译失败（`commit_edit`/`edited_text`/`has_edit` 未定义）。

- [ ] **Step 3: 实现 —— 加 `edited` 字段**

在 `Transcript` struct（`full`/`raw_len`/`polished` 旁）加字段：

```rust
/// 用户编辑后的 committed 文本（空 = 未编辑；非空时覆盖 polished/raw，优先级最高）。
edited: String,
```

`new()` 初始化（与 `polished: String::new()` 并列）：

```rust
edited: String::new(),
```

- [ ] **Step 4: 实现 —— `commit_edit` + 访问器**

在 `impl Transcript` 中（`on_polish_done` 附近）加：

```rust
/// 用户提交编辑：edited = 文本，raw_len 推进到 full 末尾（increase 清空），full（raw）不变。
/// 空串 → 清空 edited（回退到 polished/raw）。
pub fn commit_edit(&mut self, text: &str) {
    if text.is_empty() {
        self.edited.clear();
    } else {
        self.edited = text.to_string();
        self.raw_len = self.full.chars().count();
    }
}

/// 是否已编辑（edited 非空）。
pub fn has_edit(&self) -> bool {
    !self.edited.is_empty()
}

/// edited 文本（未编辑返回 None）。
pub fn edited_text(&self) -> Option<&str> {
    if self.edited.is_empty() {
        None
    } else {
        Some(&self.edited)
    }
}
```

- [ ] **Step 5: 实现 —— 改 `display_text()` 优先级链**

替换现有 `display_text()`（行 123-132）：

```rust
/// 展示文本：committed 前缀 + increase。
/// committed 优先级：edited ≻ polished ≻ full[..raw_len]。
/// edited 为空时与旧行为等价（full[..raw_len] + full[raw_len..] = full）。
pub fn display_text(&self) -> String {
    let committed = if !self.edited.is_empty() {
        self.edited.clone()
    } else if !self.polished.is_empty() {
        self.polished.clone()
    } else {
        self.full.chars().take(self.raw_len).collect()
    };
    let inc: String = self.full.chars().skip(self.raw_len).collect();
    let mut s = committed;
    s.push_str(&inc);
    s
}
```

- [ ] **Step 6: 运行测试确认通过**

Run: `cargo test -p octopus-desktop transcript::tests`
Expected: 全部 PASS（含新增 5 个 + 现有测试，display 在 edited 空时行为保持）。

- [ ] **Step 7: Commit**

```bash
git add crates/desktop/src/transcript.rs
git commit -m "feat(desktop): Transcript 三文本分层（edited ≻ polished ≻ raw）+ commit_edit"
```

---

## Task 2: DB `edited_text` 列

**Files:**
- Modify: `crates/infra/src/db.sql:7-18`
- Modify: `crates/infra/src/db.rs`（`update_polished` 旁加 `update_edited_text`；`TranscriptionRecord` + `list_transcriptions_at`）

开发阶段删库重建（`~/.octopus/octopus.db`），与 db.sql 头注释约定一致，不写 ALTER 迁移。`finalize_transcription` **不改**——`edited_text` 由 `commit_edit` 时单独 UPDATE，finalize 不触碰该列（保留）。

- [ ] **Step 1: 写失败测试 —— update_edited_text 往返**

在 `crates/infra/src/db.rs` 的 `#[cfg(test)] mod tests` 末尾加（参照现有 finalize/round-trip 测试模式；若已有内存 DB 测试辅助则复用）：

```rust
#[test]
fn update_edited_text_persists_and_lists() {
    let conn = open_init(); // 复用现有内存 DB 辅助（db.rs:538，open_in_memory + INIT_SQL）
    // 先插一条记录（仿 insert_transcription_at_id）
    conn.execute(
        "INSERT INTO transcriptions (id, created_at, engine, raw_text, polish_status)
         VALUES (1, '2026-06-18', 'test', 'raw原文', 'off')",
        [],
    ).unwrap();

    // UPDATE edited_text
    let n = conn.execute(
        "UPDATE transcriptions SET edited_text=?1 WHERE id=?2",
        rusqlite::params!["手改文本", 1],
    ).unwrap();
    assert_eq!(n, 1);

    // 读回
    let edited: Option<String> = conn.query_row(
        "SELECT edited_text FROM transcriptions WHERE id=1", [], |r| r.get(0),
    ).unwrap();
    assert_eq!(edited.as_deref(), Some("手改文本"));
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p octopus-infra update_edited_text_persists_and_lists`
Expected: 失败（`edited_text` 列不存在 / 辅助缺失）。

- [ ] **Step 3: DDL 加列**

`crates/infra/src/db.sql` 的 `transcriptions` 表（`polished_text TEXT,` 下一行）加：

```sql
    edited_text   TEXT,                     -- 用户编辑后的最终文本（未编辑为 NULL）
```

- [ ] **Step 4: 加 `update_edited_text` 函数**

`crates/infra/src/db.rs`，`update_polished` 函数（行 378）之后加：

```rust
/// 用户提交编辑后更新 edited_text（commit_edit 时调用）。
pub fn update_edited_text(id: i64, edited_text: &str) -> Result<()> {
    with_db(|conn| {
        conn.execute(
            "UPDATE transcriptions SET edited_text=?1 WHERE id=?2",
            params![edited_text, id],
        )?;
        Ok(())
    })
}
```

- [ ] **Step 5: `TranscriptionRecord` 加字段**

`crates/infra/src/db.rs:416` 的 struct 加字段（`polished_text` 下一行）：

```rust
    pub edited_text: Option<String>,
```

- [ ] **Step 6: `list_transcriptions_at` SELECT + 映射加列**

行 459 SELECT 改为（在 `polished_text` 后加 `edited_text`）：

```rust
        "SELECT id, created_at, engine, raw_text, polished_text, edited_text, polish_status, duration_ms
         FROM transcriptions ORDER BY id DESC LIMIT ?1 OFFSET ?2"
```

行 462-471 的 `query_map` 改为（注意列序变化：edited_text 在 5，polish_status 移到 6，duration_ms 移到 7）：

```rust
        let rows = stmt.query_map(params![limit, offset], |row| {
            Ok(TranscriptionRecord {
                id: row.get(0)?,
                created_at: row.get(1)?,
                engine: row.get(2)?,
                raw_text: row.get(3)?,
                polished_text: row.get(4)?,
                edited_text: row.get(5)?,
                polish_status: row.get(6)?,
                duration_ms: row.get(7)?,
            })
        })?;
```

- [ ] **Step 7: 运行测试确认通过**

Run: `cargo test -p octopus-infra`
Expected: PASS（含新测试 + 现有 db 测试）。

- [ ] **Step 8: Commit**

```bash
git add crates/infra/src/db.sql crates/infra/src/db.rs
git commit -m "feat(infra): transcriptions 加 edited_text 列 + update_edited_text + 历史查询"
```

---

## Task 3: coordinator 编辑命令 + tick 闸门 + commit→DB

**Files:**
- Modify: `crates/desktop/src/coordinator.rs`
- Modify: `crates/desktop/src/main.rs`（invoke_handler）

编辑态：`editing: bool` + `edit_buffer: Option<String>` 为循环局部变量；tick 在 editing 时排空丢弃音频；commit 时写 transcript + `UPDATE edited_text`。Toggle-期间-编辑 用 `edit_buffer`（前端 input 防抖推送）恢复。

- [ ] **Step 1: `Command` enum 加 3 变体**

`coordinator.rs:18` 的 `enum Command`（`PolishNow` 后）加：

```rust
    /// 进入编辑态（前端双击/编辑按钮触发；ASR 硬暂停）
    EnterEditMode,
    /// 更新编辑缓冲（前端 input 防抖推送；供 Toggle-期间-编辑 恢复）
    UpdateEditBuffer { text: String },
    /// 提交编辑（快捷键/完成按钮/失焦触发）
    CommitEdit { text: String },
```

- [ ] **Step 2: `DbCommand` enum 加 `UpdateEdited`**

`coordinator.rs:1493` 的 `enum DbCommand`（`Finalize` 后）加：

```rust
    UpdateEdited {
        id: i64,
        edited_text: String,
    },
```

- [ ] **Step 3: `process_db_command` 加 arm**

`coordinator.rs:1531` 的 `match cmd`（`Finalize` arm 后）加：

```rust
        DbCommand::UpdateEdited { id, edited_text } => {
            if let Err(e) = octopus_asr::db::update_edited_text(id, &edited_text) {
                warn!("Background DB update_edited_text failed: {}", e);
            }
        }
```

- [ ] **Step 4: 主循环加 `editing` + `edit_buffer` 局部变量**

`coordinator.rs:163`（`let mut stage = Stage::Idle;` 旁）加：

```rust
            let mut stage = Stage::Idle;
            // 编辑态：置位时 tick 跳过喂引擎、只排空丢弃音频（硬暂停）。
            let mut editing = false;
            // 编辑缓冲：前端 input 防抖推送的最新文本；Toggle-期间-编辑 时用作提交文本。
            let mut edit_buffer: Option<String> = None;
```

- [ ] **Step 5: tick 分发加 editing 闸门**

`coordinator.rs:199`（`Command::StreamingTick`）改为：

```rust
                    Command::StreamingTick => {
                        {
                            let rc = runtime_config.read().unwrap();
                            config.polish_mode = rc.polish_mode;
                        }
                        if let Stage::Streaming { transcript, .. } = &mut stage {
                            transcript.set_mode(config.polish_mode);
                        }
                        if editing {
                            // 编辑期：排空音频缓冲丢弃，不喂引擎（硬暂停）
                            let _ = audio.drain_samples();
                        } else {
                            handle_streaming_tick(&mut stage, &audio, &config, &app_handle, &tx);
                        }
                    }
```

`coordinator.rs:209`（`Command::VadSegmentedTick`）同样在 `set_mode` 后、`handle_vad_segmented_tick` 前加闸门：

```rust
                    Command::VadSegmentedTick => {
                        {
                            let rc = runtime_config.read().unwrap();
                            config.polish_mode = rc.polish_mode;
                        }
                        if let Stage::VadSegmented { transcript, .. } = &mut stage {
                            transcript.set_mode(config.polish_mode);
                        }
                        if editing {
                            let _ = audio.drain_samples();
                        } else {
                            handle_vad_segmented_tick(
                                &mut stage, &audio, &engine, &config, &app_handle, &tx,
                            );
                        }
                    }
```

- [ ] **Step 6: 加 3 个编辑 Command 分发 arm + TranscriptionDone 编辑期守卫**

在 `Command::PolishNow` arm（行 288）后加：

```rust
                    Command::EnterEditMode => {
                        handle_enter_edit_mode(&mut stage, &mut editing, &mut edit_buffer);
                    }
                    Command::UpdateEditBuffer { text } => {
                        if editing {
                            edit_buffer = Some(text);
                        }
                    }
                    Command::CommitEdit { text } => {
                        if editing {
                            commit_edit_apply(&mut stage, &text, &app_handle);
                            editing = false;
                        }
                    }
```

接着改 `Command::TranscriptionDone` arm（`coordinator.rs:229`），编辑期忽略在途结果（硬暂停下不应有新段，防御性丢弃——spec §7）：

```rust
                    Command::TranscriptionDone { text, seq, session_id } => {
                        if editing {
                            debug!("TranscriptionDone ignored during edit");
                        } else {
                            handle_transcription_done(
                                &mut stage, text, seq, session_id, &config, &app_handle, &tx,
                            );
                        }
                    }
```

- [ ] **Step 7: `Command::Toggle` 加编辑态先提交**

`coordinator.rs:175`（`Command::Toggle =>`）在 `handle_toggle(...)` 调用前插入：

```rust
                    Command::Toggle => {
                        // 编辑态下停止：先用 edit_buffer 提交编辑，再走停止流程（spec §7）
                        if editing {
                            if let Some(text) = edit_buffer.take() {
                                commit_edit_apply(&mut stage, &text, &app_handle);
                            }
                            editing = false;
                            let _ = app_handle.emit("edit-force-exit", ());
                        }
                        handle_toggle(
                            // ...原参数不变
```

> 保持原 `handle_toggle(...)` 调用的所有参数不变，只在前面插入 if 块。
>
> **前置导入**：coordinator.rs 顶部 use 区（`use std::time::Instant;` 下一行）加 `use tauri::Emitter;`——`app_handle.emit(...)` 需 `Emitter` trait（现有 emit 在 `handle_polish_done` 等处用函数内局部 `use tauri::Emitter;`，文件顶统一导入更简洁，局部导入保留无害）。

- [ ] **Step 8: 实现 `handle_enter_edit_mode` + `commit_edit_apply`**

在 `handle_polish_now` 等函数附近（如 `start_final_polish_or_paste` 之前）加：

```rust
/// 进入编辑态：仅活跃会话（Streaming/VadSegmented）有效；初始化 edit_buffer = 当前 display。
fn handle_enter_edit_mode(stage: &mut Stage, editing: &mut bool, edit_buffer: &mut Option<String>) {
    let transcript = match stage {
        Stage::Streaming { transcript, .. } | Stage::VadSegmented { transcript, .. } => transcript,
        _ => {
            debug!("enter_edit_mode ignored in non-active stage");
            return;
        }
    };
    *editing = true;
    *edit_buffer = Some(transcript.display_text());
    info!("Entered edit mode (transcript id={})", transcript.id);
}

/// 提交编辑：写回 transcript（commit_edit）+ UPDATE edited_text（行已存在）+ 刷新展示。
fn commit_edit_apply(stage: &mut Stage, text: &str, app_handle: &tauri::AppHandle) {
    let transcript = match stage {
        Stage::Streaming { transcript, .. } | Stage::VadSegmented { transcript, .. } => transcript,
        _ => {
            debug!("commit_edit ignored in non-active stage");
            return;
        }
    };
    transcript.commit_edit(text);
    if transcript.db_inserted() {
        let id = transcript.id;
        if let Err(e) = get_db_sender().send(DbCommand::UpdateEdited {
            id,
            edited_text: text.to_string(),
        }) {
            warn!("Queue DB UpdateEdited failed: {}", e);
        }
    }
    crate::result_window::update_result(app_handle, &transcript.display_text());
    info!("Edit committed ({} chars)", text.chars().count());
}
```

> or-pattern `Stage::Streaming { transcript, .. } | Stage::VadSegmented { transcript, .. }` 合法：两变体都有 `transcript: Transcript` 字段，绑定同类型。

- [ ] **Step 9: 加 Coordinator 公开方法 + Tauri 命令**

`coordinator.rs:320`（`polish_now` 方法后，`impl Coordinator` 内）加：

```rust
    /// 进入编辑态
    pub fn enter_edit_mode(&self) {
        if let Ok(tx) = self.tx.lock() {
            if tx.send(Command::EnterEditMode).is_err() {
                error!("Coordinator channel closed");
            }
        }
    }

    /// 更新编辑缓冲（前端 input 防抖推送）
    pub fn update_edit_buffer(&self, text: String) {
        if let Ok(tx) = self.tx.lock() {
            if tx.send(Command::UpdateEditBuffer { text }).is_err() {
                error!("Coordinator channel closed");
            }
        }
    }

    /// 提交编辑
    pub fn commit_edit(&self, text: String) {
        if let Ok(tx) = self.tx.lock() {
            if tx.send(Command::CommitEdit { text }).is_err() {
                error!("Coordinator channel closed");
            }
        }
    }
```

`coordinator.rs:338`（`pub fn polish_now(...)` Tauri 命令后）加：

```rust
/// 前端命令：进入编辑态（双击/编辑按钮触发）。
#[tauri::command]
pub fn enter_edit_mode(coordinator: tauri::State<'_, Coordinator>) {
    coordinator.enter_edit_mode();
}

/// 前端命令：更新编辑缓冲（input 防抖推送）。
#[tauri::command]
pub fn update_edit_buffer(coordinator: tauri::State<'_, Coordinator>, text: String) {
    coordinator.update_edit_buffer(text);
}

/// 前端命令：提交编辑（快捷键/完成按钮/失焦触发）。
#[tauri::command]
pub fn commit_edit(coordinator: tauri::State<'_, Coordinator>, text: String) {
    coordinator.commit_edit(text);
}
```

- [ ] **Step 10: main.rs 注册 3 命令**

`crates/desktop/src/main.rs` `invoke_handler`（行 165-181 的 `generate_handler!`）加 3 行（`coordinator::polish_now,` 后）：

```rust
            coordinator::enter_edit_mode,
            coordinator::update_edit_buffer,
            coordinator::commit_edit,
```

- [ ] **Step 11: 编译验证**

Run: `cargo check -p octopus-desktop --all-targets`
Expected: 编译通过（或提示 `tauri::Emitter` 未导入 → 在 coordinator.rs 顶部 `use tauri::{Emitter, Manager};` 修正）。

- [ ] **Step 12: 运行现有测试确认无回归**

Run: `cargo test -p octopus-desktop`
Expected: PASS（transcript 测试 + 现有 coordinator 测试）。

- [ ] **Step 13: Commit**

```bash
git add crates/desktop/src/coordinator.rs crates/desktop/src/main.rs
git commit -m "feat(desktop): 编辑态命令 + tick 硬暂停闸门 + commit→DB edited_text"
```

---

## Task 4: 停止路径用 display_text 保留编辑

**Files:**
- Modify: `crates/desktop/src/coordinator.rs`

3 处 paste 输入（Streaming 584 / VadSegmented 522-524 / handle_transcription_done 1349-1351）从 `db_text()` 改为「edited 优先」：edited 非空用 `display_text()`（不补句末标点）；否则走原 raw 逻辑（含补「。」）。DB raw 仍用 `db_text()`（649/662/1647/1656 不动）。

- [ ] **Step 1: 加 `Transcript::edited_display` 辅助**

`crates/desktop/src/transcript.rs`（`edited_text` 旁）加：

```rust
/// 停止时喂给「最终润色/粘贴」的文本。
/// edited 非空 → edited + increase（= display，用户编辑结果，不补标点）。
/// 否则 None → 调用方走原 raw 逻辑（db_text + 按需补「。」）。
pub fn edited_display(&self) -> Option<String> {
    if self.edited.is_empty() {
        None
    } else {
        Some(self.display_text())
    }
}
```

- [ ] **Step 2: VadSegmented 停止路径（行 516-525）**

替换 `let final_text = if ... else ...` 块为 edited 优先：

```rust
                let final_text = if let Some(edited) = transcript.edited_display() {
                    edited
                } else if transcript.full().is_empty() {
                    String::new()
                } else if transcript
                    .full()
                    .ends_with(|c: char| ",.，。！？!?\n".contains(c))
                {
                    transcript.db_text()
                } else {
                    format!("{}。", transcript.db_text())
                };
```

- [ ] **Step 3: handle_transcription_done 停止路径（行 1349-1351）**

同样替换（结构与 Step 2 相同的 `final_text` 块）：

```rust
                    let final_text = if let Some(edited) = transcript.edited_display() {
                        edited
                    } else if transcript.full().is_empty() {
                        String::new()
                    } else if transcript
                        .full()
                        .ends_with(|c: char| ",.，。！？!?\n".contains(c))
                    {
                        transcript.db_text()
                    } else {
                        format!("{}。", transcript.db_text())
                    };
```

- [ ] **Step 4: Streaming 停止路径（行 584 + 571）**

行 584 `let combined = transcript.db_text();` 改为优先 edited：

```rust
            let combined = transcript
                .edited_display()
                .unwrap_or_else(|| transcript.db_text());
```

行 571（`finish()` 失败回退）`transcript.db_text()` 改为同样优先 edited：

```rust
                    Err(e) => {
                        error!("Streaming finish failed: {}", e);
                        transcript
                            .edited_display()
                            .unwrap_or_else(|| transcript.db_text())
                    }
```

- [ ] **Step 5: 编译验证**

Run: `cargo check -p octopus-desktop --all-targets`
Expected: 通过。

- [ ] **Step 6: 运行测试**

Run: `cargo test -p octopus-desktop`
Expected: PASS。

- [ ] **Step 7: Commit**

```bash
git add crates/desktop/src/coordinator.rs crates/desktop/src/transcript.rs
git commit -m "feat(desktop): 停止粘贴/润色输入改用 display_text（edited 优先，保留编辑）"
```

---

## Task 5: 前端编辑交互

**Files:**
- Create: `crates/desktop/dist/result/icons/edit.svg`
- Modify: `crates/desktop/dist/result/index.html`

`#result-text` 默认不可编辑；双击或点编辑按钮 → `contenteditable=true` + 聚焦 + `enter_edit_mode`；`Cmd/Ctrl+Enter` / 完成按钮 / blur → `commit_edit`；input 防抖推 `update_edit_buffer`；编辑态加边框、禁 mouseleave 收起、冻结 update-result。

- [ ] **Step 1: 新建 edit.svg 图标**

`crates/desktop/dist/result/icons/edit.svg`（铅笔图标，currentcolor 友好）：

```svg
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 20h9"/><path d="M16.5 3.5a2.121 2.121 0 0 1 3 3L7 19l-4 1 1-4Z"/></svg>
```

- [ ] **Step 2: 加编辑按钮 + 完成按钮 HTML**

`index.html` 工具栏（`#tool-polish-now` 按钮 `</button>` 后）加编辑按钮：

```html
        <button class="tool" id="tool-edit" title="编辑" aria-label="编辑">
          <span class="icon"></span>
        </button>
```

`#text-wrapper`（`<div id="text-wrapper">` 内、`#result-text` 前）加完成按钮：

```html
    <div id="text-wrapper">
      <button id="edit-done" hidden>完成编辑</button>
      <div id="result-text"></div>
    </div>
```

- [ ] **Step 3: 加 CSS —— 编辑按钮图标 + 完成按钮 + 编辑态指示**

`index.html` `<style>`（`#tool-polish-now .icon` 行后）加编辑按钮图标：

```css
    #tool-edit .icon { -webkit-mask-image: url(icons/edit.svg?v=1); mask-image: url(icons/edit.svg?v=1); }
```

`#result-text` 规则后加完成按钮 + 编辑态：

```css
    /* 完成编辑按钮：编辑态显示，浮于文本区右上 */
    #edit-done {
      position: absolute;
      top: 4px;
      right: 8px;
      z-index: 15;
      font-size: 12px;
      padding: 2px 10px;
      border: 0.5px solid rgba(0,122,255,0.4);
      border-radius: 6px;
      background: rgba(0,122,255,0.08);
      color: #007aff;
      cursor: pointer;
    }
    #edit-done:hover { background: rgba(0,122,255,0.16); }

    /* 编辑态：蓝边框 + 可编辑光标 */
    #container.editing #result-text {
      border: 1px solid #007aff;
      border-radius: 4px;
      padding: 1px 13px 7px 13px;
    }
    #container.editing #result-text:focus { background: rgba(0, 122, 255, 0.06); }
```

- [ ] **Step 4: 加编辑态 JS**

`index.html` `<script>`（`const btnPolishNow = ...` 附近，润色逻辑后）加：

```js
    // ── 编辑态 ──
    let editing = false;
    const btnEdit = document.getElementById('tool-edit');
    const btnEditDone = document.getElementById('edit-done');

    function enterEdit() {
      if (editing) return;
      editing = true;
      resultText.setAttribute('contenteditable', 'true');
      container.classList.add('editing');
      btnEditDone.hidden = false;
      btnEdit.classList.add('active');
      currentWindow.setFocus();          // 保证键盘焦点进入 webview
      resultText.focus();
      invoke('enter_edit_mode');
      updateEditBuffer();                 // 初始 buffer
    }

    function commitEdit() {
      if (!editing) return;
      const text = resultText.innerText;
      editing = false;
      resultText.setAttribute('contenteditable', 'false');
      container.classList.remove('editing');
      btnEditDone.hidden = true;
      btnEdit.classList.remove('active');
      invoke('commit_edit', { text });
    }

    function updateEditBuffer() {
      if (!editing) return;
      invoke('update_edit_buffer', { text: resultText.innerText });
    }

    // 进入：双击文本 / 点编辑按钮
    resultText.addEventListener('dblclick', enterEdit);
    btnEdit.addEventListener('click', (e) => { e.preventDefault(); enterEdit(); });

    // 退出：Cmd/Ctrl+Enter
    document.addEventListener('keydown', (e) => {
      if (editing && (e.metaKey || e.ctrlKey) && e.key === 'Enter') {
        e.preventDefault();
        commitEdit();
      }
    });

    // 退出：完成按钮（mousedown preventDefault 防过早 blur 抢焦导致 click 丢失）
    btnEditDone.addEventListener('mousedown', (e) => e.preventDefault());
    btnEditDone.addEventListener('click', (e) => { e.preventDefault(); commitEdit(); });

    // 退出：失焦（点别处 / 点 toolbar / 点文本区外）
    resultText.addEventListener('blur', () => { if (editing) commitEdit(); });

    // input 防抖推 edit_buffer（Toggle-期间-编辑 恢复用）
    let editBufTimer = null;
    resultText.addEventListener('input', () => {
      clearTimeout(editBufTimer);
      editBufTimer = setTimeout(updateEditBuffer, 150);
    });

    // 后端强制退出（Toggle-期间-编辑 提交后）
    listen('edit-force-exit', () => {
      if (editing) {
        editing = false;
        resultText.setAttribute('contenteditable', 'false');
        container.classList.remove('editing');
        btnEditDone.hidden = true;
        btnEdit.classList.remove('active');
      }
    });
```

- [ ] **Step 5: 编辑态冻结 update-result + 禁 mouseleave 收起**

`listen('update-result', ...)`（行 447）加 editing 守卫：

```js
    listen('update-result', (event) => {
      if (editing) return;               // 编辑态冻结，不覆盖用户输入
      resultText.textContent = event.payload;
      resultText.scrollTop = resultText.scrollHeight;
    });
```

`hideToolbar` 函数（行 236）开头加守卫：

```js
    function hideToolbar() {
      if (!toolbarVisible || editing) return;   // 编辑态不收起
      ...
```

- [ ] **Step 6: 手动构建验证（devtools 开启）**

Run: `cargo run -p octopus-desktop`
手动验证（debug 构建自动开 devtools）：
1. 按快捷键录音 → 说一句 → 结果窗出文本。
2. 双击文本 → 出现蓝边框 + 「完成编辑」按钮 → 继续说话，窗口不再刷新（硬暂停）。
3. 改一两个字 → 点「完成编辑」→ 边框消失 → 继续说话 → 新文本追加在编辑结果后。
4. 再双击 → 改 → 按 Cmd+Enter → 同样生效。
5. 编辑态点工具栏 ASR 按钮 → 先退出编辑（边框消失）再弹 ASR 浮层。

Expected: 上述行为符合预期，devtools 无 JS 报错。

- [ ] **Step 7: Commit**

```bash
git add crates/desktop/dist/result/index.html crates/desktop/dist/result/icons/edit.svg
git commit -m "feat(desktop): 结果窗可编辑（双击/按钮进入，快捷键/按钮/失焦退出，硬暂停）"
```

---

## Task 6: 文档同步 + 端到端验证

**Files:**
- Modify: `docs/configuration.md`
- Modify: `docs/architecture.md`
- Modify: `docs/superpowers/specs/2026-06-18-editable-result-window-design.md`（状态行）

- [ ] **Step 1: configuration.md 加编辑能力说明**

在结果窗/工具栏相关段落（或「使用」段）加小节：

```markdown
### 结果展示区编辑

录音过程中可随时修正识别/润色文本：
- **进入编辑**：双击结果区文本，或点工具栏 ✏️ 编辑按钮。单击不触发（防误触）。
- **编辑期间 ASR 硬暂停**（音频丢弃），改完恢复。
- **退出编辑**（择一）：`Cmd/Ctrl+Enter`、点「完成编辑」按钮、或失焦（点别处/工具栏）。
- 编辑后的文本作为后续展示与润色基准；新识别文本追加其上；停止粘贴时保留编辑。
- 未编辑时行为与旧版完全一致。
```

- [ ] **Step 2: architecture.md 同步 Transcript 模型 + 编辑态 + DB**

在「模型管理」/ Transcript 相关段加：

```markdown
- `Transcript` 三文本分层：`edited ≻ polished ≻ raw`。`display_text()` = committed + increase；
  `full`（原始 ASR）独立保留为 DB `raw_text`。
- 编辑态：coordinator 主循环 `editing` 标志置位时，Streaming/VadSegmented tick 跳过喂引擎、
  只排空丢弃音频（硬暂停）。`commit_edit` 写回 transcript 并 `UPDATE edited_text`。
- `transcriptions` 表加 `edited_text` 列（commit 时写，finalize 不触碰）。
- 停止路径粘贴/润色输入 = `display_text()`（edited 优先），DB raw 仍 = `db_text()`。
```

- [ ] **Step 3: spec 状态行置已实现**

`docs/superpowers/specs/2026-06-18-editable-result-window-design.md` 顶部 `> Status:` 行改为：

```
> Status: ✅ 已实现（2026-06-18，plan 2026-06-18-editable-result-window.md）。会话中编辑（双击/按钮进入，快捷键/按钮/失焦退出，硬暂停）+ 三文本分层 + DB edited_text 均已落地。
```

- [ ] **Step 4: 删库重建 + 全流程 e2e**

```bash
# 备份后删库重建（db.sql 改了）
cp ~/.octopus/octopus.db ~/.octopus/octopus.db.bak.$(date +%s)
rm ~/.octopus/octopus.db
cargo run -p octopus-desktop
```

手动 e2e（三种 PolishMode 各验一次）：
1. `polish_mode=2`（中间润色）：录音 → 说一段（出中间润色）→ 双击改错 → 完成 → 继续说 → 停止 → 粘贴文本 = edited + 后续，且 DB `edited_text` 非空、`raw_text` 为原始 ASR。
2. `polish_mode=0`（关闭）：录音 → 双击改 → 完成 → 停止 → 粘贴 = edited。
3. 编辑态按停止热键 → 编辑被提交（edit_buffer）→ 粘贴含编辑。
4. `sqlite3 ~/.octopus/octopus.db "SELECT raw_text, polished_text, edited_text FROM transcriptions ORDER BY id DESC LIMIT 3;"` 验证三列互不干扰。

- [ ] **Step 5: workspace 全量编译 + 测试**

```bash
cargo check --workspace --all-targets
cargo test --workspace
```
Expected: 全绿。

- [ ] **Step 6: Commit**

```bash
git add docs/configuration.md docs/architecture.md docs/superpowers/specs/2026-06-18-editable-result-window-design.md
git commit -m "docs: 同步结果窗可编辑（configuration/architecture/spec 状态）"
```

---

## 验证总结

| 场景 | 预期 |
|---|---|
| 未编辑（任意 mode） | 行为与旧版完全一致（display 公式等价） |
| 双击编辑 → 完成 → 继续说 | 新文本追加在 edited 后；display = edited + increase |
| 编辑后停顿润色（mode=2） | 润色输入 = edited + increase（spec Q3） |
| 停止粘贴 | 粘贴 = display（含 edited）；DB raw 仍原始 ASR |
| 编辑态点 toolbar | 先退出编辑（blur 提交）再执行按钮动作 |
| 编辑态按停止热键 | edit_buffer 提交编辑后停止 |
| DB | raw/polished/edited 三列独立、互不干扰 |

## 关键文件

- `crates/desktop/src/transcript.rs`（edited 字段 + commit_edit + display 优先级链 + edited_display）
- `crates/infra/src/db.sql` + `crates/infra/src/db.rs`（edited_text 列 + update_edited_text + 历史查询）
- `crates/desktop/src/coordinator.rs`（Command/DbCommand 变体 + editing 闸门 + commit→DB + 停止路径 display）
- `crates/desktop/src/main.rs`（invoke_handler 注册 3 命令）
- `crates/desktop/dist/result/index.html` + `icons/edit.svg`（编辑交互）
