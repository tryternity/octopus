# 结果展示区可编辑 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在录音会话进行中允许用户编辑结果展示区文本（双击/按钮进入，快捷键/按钮/失焦退出），编辑期间 ASR 硬暂停；编辑后的文本作为后续展示与润色基准，新识别文本追加其上，停止粘贴时保留编辑；编辑后触发润色时只润色新增、保留已编辑（折回 edited + 边界提示词）。

**Architecture:** `Transcript` 新增 `edited` 字段，作为 `edited ≻ polished ≻ raw` 分层优先级最高层；`display_text()` = `committed + increase`。编辑是 coordinator 主循环里的 `editing` 标志——置位时两个 tick handler 跳过喂引擎、只排空丢弃音频（硬暂停）。提交时 `commit_edit` 写回 transcript 并 `UPDATE edited_text`。编辑×润色交互（spec §12）：`take_polish_input()` 返回 `(preserved=edited, to_polish=increase)`，LLM 仅润色 `to_polish`，`on_polish_done` 在 `has_edit()` 时把结果折回 `edited`（避免 edited 遮蔽 polished 导致丢字）。

**Tech Stack:** Rust（crates: infra, asr, llm, desktop）/ Tauri webview / 原生 HTML+JS（`dist/result/index.html`）/ SQLite（rusqlite）。

参考 spec：[`docs/superpowers/specs/2026-06-18-editable-result-window-design.md`](../specs/2026-06-18-editable-result-window-design.md)（§4 三文本模型、§5 提交语义、§12 编辑×润色交互）。

---

## File Structure

| 文件 | 职责 | 改动任务 |
|---|---|---|
| `crates/desktop/src/transcript.rs` | 三文本状态机 | T1 编辑模型 + T5 润色模型 |
| `crates/llm/src/prompt.rs` + `client.rs` | 润色提示词 | T2 边界提示词 |
| `crates/infra/src/db.sql` + `db.rs` | DDL + DB 访问 | T3 edited_text 列 |
| `crates/desktop/src/coordinator.rs` | 状态机主循环 | T4 编辑机制 + T5 润色接线 + T6 停止路径 |
| `crates/desktop/src/main.rs` | Tauri 命令注册 | T4 |
| `crates/desktop/dist/result/index.html` + `icons/edit.svg` | 结果窗前端 | T7 |
| `docs/configuration.md` / `architecture.md` | 文档 | T8 |

> **编译绿原则**：每个任务结束 `cargo check -p <crate>` 通过。T1 只加「编辑模型」方法（不动 snapshot/on_polish_done，coordinator 照旧编译）；T2 改 polish 签名时同步把 coordinator 旧调用点改成 `polish(None, &text, ..)`；T5 才统一接 `(preserved, to_polish)`。

---

## Task 1: Transcript 编辑模型

**Files:**
- Modify: `crates/desktop/src/transcript.rs`

纯逻辑、单文件、完全可单测。只加「编辑相关」字段/方法（`edited`、`commit_edit`、`has_edit`、`edited_text`、`display_text` 优先级链、`edited_display`），**不动** `snapshot_for_polish` / `on_polish_done`（留给 T5），coordinator 照旧编译。`edited` 为空时 `display_text()` 与现有行为等价，现有测试不破。

- [x] **Step 1: 写失败测试**

在 `transcript.rs` 的 `#[cfg(test)] mod tests` 末尾追加：

```rust
#[test]
fn commit_edit_sets_edited_and_advances_boundary() {
    let mut t = Transcript::new(30, PolishMode::Intermediate);
    t.set_full("你好世界");
    t.snapshot_for_polish(); // T1 阶段仍用旧 snapshot；T5 替换
    t.on_polish_done("你好，世界。".into());
    assert_eq!(t.display_text(), "你好，世界。");

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
    assert_eq!(t.full(), "原文"); // raw（full）原样保留
    t.set_full("原文新增");
    assert_eq!(t.display_text(), "原文（手改）新增"); // edited + 新增
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
fn edited_display_returns_display_when_edited_else_none() {
    let mut t = Transcript::new(34, PolishMode::Intermediate);
    t.set_full("原文");
    assert_eq!(t.edited_display(), None); // 未编辑
    t.commit_edit("手改".into());
    assert_eq!(t.edited_display().as_deref(), Some("手改"));
    t.set_full("原文新增");
    assert_eq!(t.edited_display().as_deref(), Some("手改新增")); // = display
}
```

- [x] **Step 2: 运行测试确认失败**

Run: `cargo test -p octopus-desktop transcript::tests`
Expected: 编译失败（`commit_edit`/`edited_text`/`has_edit`/`edited_display` 未定义）。

- [x] **Step 3: 加 `edited` 字段**

`Transcript` struct（`polished: String,` 下一行）加：

```rust
    /// 用户编辑后的 committed 文本（空 = 未编辑；非空时覆盖 polished/raw，优先级最高）。
    edited: String,
```

`new()`（`polished: String::new(),` 下一行）加：

```rust
            edited: String::new(),
```

- [x] **Step 4: 实现 `commit_edit` + 访问器**

`impl Transcript` 中（`on_polish_done` 附近）加：

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

- [x] **Step 5: 改 `display_text()` 优先级链**

替换现有 `display_text()`（123-132 行）：

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

- [x] **Step 6: 加 `edited_display`**

`edited_text()` 旁加（停止路径无润色/兜底粘贴用，T6）：

```rust
/// 停止时喂给「无润色粘贴/兜底」的文本。
/// edited 非空 → display（用户编辑结果 + 新增，不补标点）。
/// 否则 None → 调用方走原 raw 逻辑（db_text + 按需补「。」）。
pub fn edited_display(&self) -> Option<String> {
    if self.edited.is_empty() {
        None
    } else {
        Some(self.display_text())
    }
}
```

- [x] **Step 7: 运行测试确认通过**

Run: `cargo test -p octopus-desktop transcript::tests`
Expected: 全 PASS（新增 5 个 + 现有测试；edited 空时 display 行为保持）。

- [x] **Step 8: 编译验证 desktop crate**

Run: `cargo check -p octopus-desktop --all-targets`
Expected: 通过（coordinator 未受影响——snapshot_for_polish/on_polish_done 未动）。

- [x] **Step 9: Commit**

```bash
git add crates/desktop/src/transcript.rs
git commit -m "feat(desktop): Transcript 编辑模型（edited 字段 + commit_edit + display 优先级链 + edited_display）"
```

---

## Task 2: llm 边界提示词（polish 加 preserved）

**Files:**
- Modify: `crates/llm/src/prompt.rs`
- Modify: `crates/llm/src/client.rs`（`polish` 签名）
- Modify: `crates/desktop/src/coordinator.rs`（2 处旧调用点改 `polish(None, ..)` 保持行为）
- Modify: `crates/llm/examples/test_polish.rs`（签名适配）

`polish` 签名加 `preserved: Option<&str>`；`user_prompt` 分块构造（已确认原样保留 + 新增润色）；system prompt 加增量保留规则。coordinator 旧调用点先传 `None`（保持现状），T5 再接真值。

- [x] **Step 1: 写失败测试 —— user_prompt 分块**

`crates/llm/src/prompt.rs` 末尾加测试模块（当前无 tests）：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_prompt_without_preserved_is_plain() {
        let p = user_prompt(None, "你好");
        assert!(p.contains("请润色以下语音识别文本"));
        assert!(p.contains("你好"));
        assert!(!p.contains("已确认部分"));
    }

    #[test]
    fn user_prompt_with_preserved_marks_boundary() {
        let p = user_prompt(Some("已确认文本"), "新增文本");
        assert!(p.contains("已确认部分"));
        assert!(p.contains("原样保留"));
        assert!(p.contains("已确认文本"));
        assert!(p.contains("新增部分"));
        assert!(p.contains("新增文本"));
    }
}
```

- [x] **Step 2: 运行测试确认失败**

Run: `cargo test -p octopus-llm prompt::tests`
Expected: 编译失败（`user_prompt` 当前只接 `&str`）。

- [x] **Step 3: system prompt 加增量保留规则**

`DEFAULT_SYSTEM_PROMPT` 的 `# Rules` 列表（规则 6 后）加：

```markdown
7. [增量保留]：若用户提供【已确认部分】，该部分必须逐字原样保留、严禁修改，仅润色【新增部分】，最终输出两者拼接。
```

- [x] **Step 4: `user_prompt` 加 preserved**

替换 `user_prompt`：

```rust
/// 构建 user prompt。
/// - preserved=None：全量润色（to_polish = 完整文本）。
/// - preserved=Some：编辑后增量润色，告知 LLM 已确认部分原样保留、仅润色 to_polish。
pub fn user_prompt(preserved: Option<&str>, to_polish: &str) -> String {
    match preserved {
        None => format!("请润色以下语音识别文本：\n{}", to_polish),
        Some(confirmed) => format!(
            "以下文本中，【已确认部分】已经用户人工校对，必须原样保留、严禁修改；仅对【新增部分】进行润色。\n\n\
             【已确认部分（原样保留）】\n{}\n\n【新增部分（请润色）】\n{}\n\n\
             请输出：已确认部分 + 润色后的新增部分，拼接为完整文本，仅输出纯文本。",
            confirmed, to_polish
        ),
    }
}
```

- [x] **Step 5: `polish` 签名加 preserved**

`crates/llm/src/client.rs` 的 `polish`（55 行）签名 + 空检查 + user_prompt 调用改：

```rust
/// 对 ASR 识别文本进行润色。
/// - preserved=Some：增量润色，保留 preserved 原样、仅润色 to_polish（编辑后用）。
/// - preserved=None：全量润色 to_polish。
/// 返回润色后的完整文本。
pub fn polish(preserved: Option<&str>, to_polish: &str, config: &CompatibleLlmConfig) -> Result<String> {
    if to_polish.trim().is_empty() {
        return Ok(to_polish.to_string());
    }
    // ...max_tokens 仍按 to_polish 长度（新增部分）估；若 preserved 存在，输出更长，×1.2 余量已覆盖
    let max_tokens = ((to_polish.chars().count() as f64) * 1.2).ceil() as u64;
```

`messages` 里 user content 改：

```rust
            Message {
                role: "user".to_string(),
                content: prompt::user_prompt(preserved, to_polish),
            },
```

> 其余（thinking/enable_thinking 分派、请求发送、响应解析）不变。

- [x] **Step 6: coordinator 旧调用点改 `polish(None, ..)`**

`coordinator.rs:672`（最终润色）：

```rust
                let result = match octopus_llm::polish(None, &text_to_polish, &llm_config) {
```

`coordinator.rs:1044`（spawn_polish_thread）：

```rust
        let result = match octopus_llm::polish(None, &text, &llm_config) {
```

> 仅签名适配，行为不变（preserved=None）。T5 改为真值。

- [x] **Step 7: test_polish.rs example 适配**

`crates/llm/examples/test_polish.rs` 的 `octopus_llm::polish(...)` 调用加首参 `None`（具体行由实现者 grep 定位，仅改调用签名）。

- [x] **Step 8: 运行测试 + 编译**

Run: `cargo test -p octopus-llm` && `cargo check --workspace --all-targets`
Expected: llm 测试 PASS；workspace 编译通过。

- [x] **Step 9: Commit**

```bash
git add crates/llm/src/prompt.rs crates/llm/src/client.rs crates/desktop/src/coordinator.rs crates/llm/examples/test_polish.rs
git commit -m "feat(llm): polish 加 preserved 边界提示词（增量润色保留已确认部分）"
```

---

## Task 3: DB `edited_text` 列

**Files:**
- Modify: `crates/infra/src/db.sql`
- Modify: `crates/infra/src/db.rs`（`update_polished` 旁加 `update_edited_text`；`TranscriptionRecord` + `list_transcriptions_at`）

开发阶段删库重建（`~/.octopus/octopus.db`），与 db.sql 头注释约定一致，不写 ALTER 迁移。`finalize_transcription` **不改**——`edited_text` 由 commit_edit / 折回时单独 UPDATE。

- [x] **Step 1: 写失败测试**

`crates/infra/src/db.rs` 的 `#[cfg(test)] mod tests` 末尾加（复用内存 DB 辅助 `open_init`，约 538 行 `Connection::open_in_memory() + INIT_SQL`）：

```rust
#[test]
fn update_edited_text_persists_and_lists() {
    let conn = open_init();
    conn.execute(
        "INSERT INTO transcriptions (id, created_at, engine, raw_text, polish_status)
         VALUES (1, '2026-06-18', 'test', 'raw原文', 'off')",
        [],
    ).unwrap();

    let n = conn.execute(
        "UPDATE transcriptions SET edited_text=?1 WHERE id=?2",
        rusqlite::params!["手改文本", 1],
    ).unwrap();
    assert_eq!(n, 1);

    let edited: Option<String> = conn.query_row(
        "SELECT edited_text FROM transcriptions WHERE id=1", [], |r| r.get(0),
    ).unwrap();
    assert_eq!(edited.as_deref(), Some("手改文本"));
}
```

> 若 `open_init` 名称/签名不同，先 grep `fn open_init` 或 `open_in_memory` 确认实际辅助名再复用。

- [x] **Step 2: 运行测试确认失败**

Run: `cargo test -p octopus-infra update_edited_text_persists_and_lists`
Expected: 失败（`edited_text` 列不存在）。

- [x] **Step 3: DDL 加列**

`crates/infra/src/db.sql` 的 `transcriptions` 表（`polished_text TEXT,` 下一行）加：

```sql
    edited_text   TEXT,                     -- 用户编辑后的最终文本（未编辑为 NULL）
```

- [x] **Step 4: 加 `update_edited_text`**

`crates/infra/src/db.rs`，`update_polished` 函数后加（参照其 `with_db` 模式；`params` 已在 use 域）：

```rust
/// 用户提交编辑 / 中间润色折回后更新 edited_text。
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

- [x] **Step 5: `TranscriptionRecord` 加字段**

`TranscriptionRecord` struct（`polished_text` 字段下一行）加：

```rust
    pub edited_text: Option<String>,
```

- [x] **Step 6: `list_transcriptions_at` SELECT + 映射加列**

SELECT（`polished_text` 后加 `edited_text`）；`query_map` 映射按新列序（edited_text 在 polished_text 后，其余顺延）。实现者读现有 SELECT/映射块（约 453-471 行），在 `polished_text` 后插入 `edited_text` 列与 `edited_text: row.get(n)?`，后续列号 +1。

- [x] **Step 7: 运行测试确认通过**

Run: `cargo test -p octopus-infra`
Expected: PASS（含新测试 + 现有 db 测试）。

- [x] **Step 8: 编译验证**

Run: `cargo check -p octopus-infra --all-targets`
Expected: 通过。

- [x] **Step 9: Commit**

```bash
git add crates/infra/src/db.sql crates/infra/src/db.rs
git commit -m "feat(infra): transcriptions 加 edited_text 列 + update_edited_text + 历史查询"
```

---

## Task 4: coordinator 编辑命令 + tick 硬暂停闸门 + commit→DB

**Files:**
- Modify: `crates/desktop/src/coordinator.rs`
- Modify: `crates/desktop/src/main.rs`（invoke_handler）

编辑态：`editing: bool` + `edit_buffer: Option<String>` 为循环局部变量；tick 在 editing 时排空丢弃音频；commit 时写 transcript（T1 `commit_edit`）+ `UPDATE edited_text`（T3）。Toggle-期间-编辑 用 `edit_buffer`（前端 input 防抖推送）恢复。**本任务不碰润色路径**（T5）。

- [x] **Step 1: `Command` enum 加 3 变体**

`coordinator.rs:18` 的 `enum Command`（`PolishNow` 后）加：

```rust
    /// 进入编辑态（前端双击/编辑按钮触发；ASR 硬暂停）
    EnterEditMode,
    /// 更新编辑缓冲（前端 input 防抖推送；供 Toggle-期间-编辑 恢复）
    UpdateEditBuffer { text: String },
    /// 提交编辑（快捷键/完成按钮/失焦触发）
    CommitEdit { text: String },
```

- [x] **Step 2: `DbCommand` enum 加 `UpdateEdited`**

`enum DbCommand`（`Finalize` 后）加：

```rust
    UpdateEdited {
        id: i64,
        edited_text: String,
    },
```

- [x] **Step 3: `process_db_command` 加 arm**

`process_db_command` 的 `match cmd`（`Finalize` arm 后）加：

```rust
        DbCommand::UpdateEdited { id, edited_text } => {
            if let Err(e) = octopus_asr::db::update_edited_text(id, &edited_text) {
                warn!("Background DB update_edited_text failed: {}", e);
            }
        }
```

- [x] **Step 4: 主循环加 `editing` + `edit_buffer` 局部变量**

`let mut stage = Stage::Idle;` 旁加：

```rust
            let mut stage = Stage::Idle;
            // 编辑态：置位时 tick 跳过喂引擎、只排空丢弃音频（硬暂停）。
            let mut editing = false;
            // 编辑缓冲：前端 input 防抖推送的最新文本；Toggle-期间-编辑 时用作提交文本。
            let mut edit_buffer: Option<String> = None;
```

- [x] **Step 5: tick 分发加 editing 闸门**

`Command::StreamingTick` arm 改为（`set_mode` 后、`handle_streaming_tick` 前加闸门）：

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
                            let _ = audio.drain_samples(); // 编辑期丢弃音频，不喂引擎
                        } else {
                            handle_streaming_tick(&mut stage, &audio, &config, &app_handle, &tx);
                        }
                    }
```

`Command::VadSegmentedTick` arm 同样在 `set_mode` 后、`handle_vad_segmented_tick` 前加：

```rust
                        if editing {
                            let _ = audio.drain_samples();
                        } else {
                            handle_vad_segmented_tick(
                                &mut stage, &audio, &engine, &config, &app_handle, &tx,
                            );
                        }
```

> 保留原 arm 其余结构；仅把 `handle_xxx_tick(...)` 调用包进 else。

- [x] **Step 6: 加 3 个编辑 Command 分发 arm + TranscriptionDone 守卫**

`Command::PolishNow` arm 后加：

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

`Command::TranscriptionDone` arm 改（编辑期忽略在途结果，spec §7）：

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

- [x] **Step 7: `Command::Toggle` 加编辑态先提交**

`Command::Toggle =>` 在 `handle_toggle(...)` 调用前插入（保持原 `handle_toggle` 所有参数不变）：

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
                            /* …原参数不变… */
```

> **前置导入**：coordinator.rs 顶部 use 区（`use std::time::Instant;` 下一行）加 `use tauri::Emitter;`（`app_handle.emit` 需 Emitter trait）。

- [x] **Step 8: 实现 `handle_enter_edit_mode` + `commit_edit_apply`**

`handle_polish_now` / `start_final_polish_or_paste` 附近加：

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

- [x] **Step 9: 加 Coordinator 公开方法 + Tauri 命令**

`impl Coordinator` 内（`polish_now` 方法后）加 3 方法；其 Tauri 命令（`pub fn polish_now(...)` 后）加 3 命令：

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

- [x] **Step 10: main.rs 注册 3 命令**

`invoke_handler` 的 `generate_handler!`（`coordinator::polish_now,` 后）加：

```rust
            coordinator::enter_edit_mode,
            coordinator::update_edit_buffer,
            coordinator::commit_edit,
```

- [x] **Step 11: 编译 + 测试**

Run: `cargo check -p octopus-desktop --all-targets && cargo test -p octopus-desktop`
Expected: 编译通过；现有测试 PASS（transcript + coordinator 测试）。

- [x] **Step 12: Commit**

```bash
git add crates/desktop/src/coordinator.rs crates/desktop/src/main.rs
git commit -m "feat(desktop): 编辑态命令 + tick 硬暂停闸门 + commit→DB edited_text"
```

---

## Task 5: 编辑×润色接线（take_polish_input + preserved + 折回）

**Files:**
- Modify: `crates/desktop/src/transcript.rs`（`take_polish_input` 替代 `snapshot_for_polish`；`on_polish_done` 折回）
- Modify: `crates/desktop/src/coordinator.rs`（`spawn_polish_thread` + 两条润色路径接 `(preserved, to_polish)`；`handle_polish_done` 折回 DB 分支）

T1/T2/T3/T4 已就绪。本任务把「润色输入 = (edited, 新增)」与「结果折回 edited」贯通（spec §12）。`on_polish_done` 在 `has_edit()` 时折回，避免 edited 遮蔽 polished 丢字。

- [x] **Step 1: 写失败测试 —— take_polish_input + 折回**

`transcript.rs` tests 末尾加：

```rust
#[test]
fn take_polish_input_no_edit_returns_full() {
    let mut t = Transcript::new(40, PolishMode::Intermediate);
    t.set_full("第一段第二段");
    let (preserved, to_polish) = t.take_polish_input();
    assert_eq!(preserved, None);
    assert_eq!(to_polish, "第一段第二段");
}

#[test]
fn take_polish_input_after_edit_returns_preserved_and_increase() {
    let mut t = Transcript::new(41, PolishMode::Intermediate);
    t.set_full("原文");
    t.commit_edit("原文（手改）"); // edited="原文（手改）", raw_len=2
    t.set_full("原文新增"); // increase="新增"
    let (preserved, to_polish) = t.take_polish_input();
    assert_eq!(preserved.as_deref(), Some("原文（手改）"));
    assert_eq!(to_polish, "新增");
}

#[test]
fn on_polish_done_folds_into_edited_when_has_edit() {
    let mut t = Transcript::new(42, PolishMode::Intermediate);
    t.set_full("原文");
    t.commit_edit("原文（手改）");
    t.set_full("原文新增");
    let _ = t.take_polish_input(); // 推进 raw_len
    // LLM 返回 edited + 润色后新增
    t.on_polish_done("原文（手改）新增（润色）".into());
    assert_eq!(t.edited_text(), Some("原文（手改）新增（润色）"));
    assert_eq!(t.display_text(), "原文（手改）新增（润色）"); // 折回 edited，无丢字
}

#[test]
fn on_polish_done_no_edit_writes_polished() {
    let mut t = Transcript::new(43, PolishMode::Intermediate);
    t.set_full("原文");
    let _ = t.take_polish_input();
    t.on_polish_done("润色".into());
    assert_eq!(t.polished(), "润色"); // 无编辑 → polished（现状）
    assert_eq!(t.display_text(), "润色");
}
```

- [x] **Step 2: 运行测试确认失败**

Run: `cargo test -p octopus-desktop transcript::tests`
Expected: 编译失败（`take_polish_input` 未定义）。

- [x] **Step 3: Transcript 加 `take_polish_input`，删 `snapshot_for_polish`**

替换 `snapshot_for_polish`（82-85 行）为：

```rust
/// 取润色输入并推进 raw_len 边界（increase 清空）。
/// - has_edit：(Some(edited), increase) —— 已确认=edited（LLM 须原样保留），待润色=increase（新增）
/// - 否则：(None, full) —— 全量原始 ASR（保持现状）
pub fn take_polish_input(&mut self) -> (Option<String>, String) {
    let preserved = if self.has_edit() {
        Some(self.edited.clone())
    } else {
        None
    };
    let to_polish = if self.has_edit() {
        self.full.chars().skip(self.raw_len).collect()
    } else {
        self.full.clone()
    };
    self.raw_len = self.full.chars().count();
    (preserved, to_polish)
}
```

> 同步更新其上方 doc 注释里「raw_len 已在 snapshot_for_polish 推进」之类措辞为 take_polish_input。

- [x] **Step 4: `on_polish_done` 折回**

替换 `on_polish_done`（88-92 行）为：

```rust
/// 润色完成：
/// - has_edit：结果折回 edited（= edited + 润色后新增），避免 edited 遮蔽 polished 丢字（spec §12）。
/// - 否则：写 polished（raw_len 已在 take_polish_input 推进）。
pub fn on_polish_done(&mut self, result: String) {
    if self.has_edit() {
        self.edited = result;
    } else {
        self.polished = result;
    }
    self.polish_pending = false;
    self.last_polish_time = Instant::now();
}
```

- [x] **Step 5: 迁移 transcript 测试中的 snapshot_for_polish 调用**

grep `snapshot_for_polish` in transcript.rs tests，逐处改 take_polish_input：
- `let snap = t.snapshot_for_polish();`（断言 snap）→ `let (preserved, snap) = t.take_polish_input();`（断言 `preserved, None` + `snap`）
- `t.snapshot_for_polish();`（不断言）→ `let _ = t.take_polish_input();`

确保无 `snapshot_for_polish` 残留（含 doc）。

- [x] **Step 6: `spawn_polish_thread` 签名加 preserved**

`spawn_polish_thread`（1027 行）签名 + body 改：

```rust
fn spawn_polish_thread(
    preserved: Option<String>,
    to_polish: String,
    config: &AppConfig,
    tx: &Sender<Command>,
    ignore_mode: bool,
) {
    let llm_config = if ignore_mode {
        crate::config::llm_config_ignore_mode(&config)
    } else {
        crate::config::llm_config(&config)
    };
    let llm_config = match llm_config {
        Some(c) => c,
        None => return,
    };
    let tx = tx.clone();
    std::thread::spawn(move || {
        let result = match octopus_llm::polish(preserved.as_deref(), &to_polish, &llm_config) {
            Ok(polished) => Ok(polished),
            Err(e) => {
                log::warn!("Polish thread error: {}", e);
                Err(e.to_string())
            }
        };
        let _ = tx.send(Command::PolishDone { result });
    });
}
```

- [x] **Step 7: 中间润色 + PolishNow 接 take_polish_input**

`check_and_trigger_polish`（1086-1089 行）：

```rust
    // 取润色输入（编辑态: preserved+increase；否则 full）+ 标记 pending + 送 LLM
    let (preserved, to_polish) = transcript.take_polish_input();
    transcript.mark_polish_pending();
    spawn_polish_thread(preserved, to_polish, config, tx, false);
```

`handle_polish_now`（1476-1479 行）：

```rust
    let (preserved, to_polish) = transcript.take_polish_input();
    transcript.mark_polish_pending();
    info!("PolishNow triggered, polishing {} chars", to_polish.chars().count());
    spawn_polish_thread(preserved, to_polish, config, tx, true);
```

- [x] **Step 8: 最终润色入口接 take_polish_input**

`start_final_polish_or_paste` 的 polish 分支（670-683 行）。当前 `let text_to_polish = text.to_string();` → 改为从 owned transcript 取边界（transcript 此时还在，未移入 Polishing）：

```rust
            let id = transcript.id;
            let raw_text = transcript.db_text();
            let (preserved, to_polish) = transcript.take_polish_input();

            *stage = Stage::Polishing {
                id,
                raw_text: raw_text.clone(),
            };

            let tx = tx.clone();
            std::thread::spawn(move || {
                let result = match octopus_llm::polish(preserved.as_deref(), &to_polish, &llm_config) {
                    Ok(polished) => {
                        if polished.is_empty() {
                            Err("Final polish returned empty".to_string())
                        } else {
                            Ok(polished)
                        }
                    }
                    Err(e) => Err(e.to_string()),
                };
                let _ = tx.send(Command::FinalPolishDone { result });
            });
```

> `take_polish_input` 推进 raw_len，但 `db_text()` 返回 full（不受 raw_len 影响），顺序 OK。无润色分支（`None => do_paste(text, ..)`）仍用调用方传入的 `text`（T6 改为 edited_display）。

- [x] **Step 9: `handle_polish_done` 折回 DB 分支**

`handle_polish_done`（1413-1425+ 行）的 `Ok(polished) => { ... }` 块：`on_polish_done` 后按 `has_edit()` 决定 DB 命令。读现有 `DbCommand::UpdatePolished { ... }` 块，改为：

```rust
        Ok(polished) => {
            if polished.is_empty() {
                warn!("Polish returned empty, keeping previous");
                transcript.on_polish_failed();
            } else {
                transcript.on_polish_done(polished.clone());
                // 折回→UpdateEdited（保持 edited_text 与 display 一致）；否则 UpdatePolished（现状）
                let cmd = if transcript.has_edit() {
                    DbCommand::UpdateEdited {
                        id: transcript.id,
                        edited_text: polished,
                    }
                } else {
                    DbCommand::UpdatePolished {
                        /* …原字段不变（id/text/status/model）… */
                    }
                };
                // …原 send cmd 逻辑…
            }
        }
```

> 实现者读现有 UpdatePolished 块的字段，搬进 else 分支；`polished` 变量在 if 分支 move 进 UpdateEdited，故先 `on_polish_done(polished.clone())`。

- [x] **Step 10: 编译 + 测试**

Run: `cargo check -p octopus-desktop --all-targets && cargo test -p octopus-desktop`
Expected: 编译通过；transcript 新测试 + 现有测试 PASS。

- [x] **Step 11: Commit**

```bash
git add crates/desktop/src/transcript.rs crates/desktop/src/coordinator.rs
git commit -m "feat(desktop): 编辑×润色接线（take_polish_input 边界 + on_polish_done 折回 edited）"
```

---

## Task 6: 停止路径无润色/兜底用 edited_display

**Files:**
- Modify: `crates/desktop/src/coordinator.rs`

两部分：**Part A** 三处「无润色粘贴 / finish 兜底」文本从 `db_text()` 改为 edited 优先（T1 `edited_display`）：edited 非空用 display（不补句末标点）；否则走原 raw 逻辑（含补「。」）。DB raw 仍用 `db_text()`。**Part B** 最终润色失败兜底保留编辑——`Stage::Polishing` 加 `fallback_text`（= 停止时 final_text），LLM 失败时 `do_paste(&fallback_text)` 而非 raw ASR。最终润色输入已由 T5 的 `take_polish_input` 处理。

### Part A：三处无润色/兜底站点改 edited_display

- [x] **Step 1: VadSegmented 停止路径（handle_toggle VadSegmented 分支）**

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

- [x] **Step 2: handle_transcription_done 停止路径**

同样替换（结构与 Step 1 相同的 `final_text` 块）。

- [x] **Step 3: Streaming 停止路径（combined + finish 失败兜底）**

`let combined = transcript.db_text();` 改：

```rust
            let combined = transcript
                .edited_display()
                .unwrap_or_else(|| transcript.db_text());
```

`finish()` 失败兜底 `transcript.db_text()` 改：

```rust
                    Err(e) => {
                        error!("Streaming finish failed: {}", e);
                        transcript
                            .edited_display()
                            .unwrap_or_else(|| transcript.db_text())
                    }
```

### Part B：最终润色失败兜底保留编辑

> Part A 后调用方传入 `start_final_polish_or_paste` 的 `text` = edited_display（含编辑）或 raw(+「。」）。复用它作最终润色失败的兜底粘贴文本，避免失败时丢编辑。

- [x] **Step 4: Stage::Polishing 加 fallback_text 字段**

`enum Stage` 的 `Polishing { id, raw_text }` 加字段：

```rust
    Polishing {
        id: i64,
        raw_text: String,
        /// 最终润色失败时的兜底粘贴文本（= 停止时 display，含编辑；成功时不用）
        fallback_text: String,
    },
```

- [x] **Step 5: 构造 Polishing 时设 fallback_text**

`start_final_polish_or_paste` 的 `*stage = Stage::Polishing { id, raw_text: raw_text.clone() }` 改：

```rust
            *stage = Stage::Polishing {
                id,
                raw_text: raw_text.clone(),
                fallback_text: text.to_string(),
            };
```

- [x] **Step 6: handle_final_polish_done 解构 + Err 分支用 fallback_text**

解构 `Stage::Polishing { id, raw_text }` → 加 `fallback_text`：

```rust
    let (id, raw_text, fallback_text) = match stage {
        Stage::Polishing { id, raw_text, fallback_text } => {
            (*id, raw_text.clone(), fallback_text.clone())
        }
        _ => { ... }
    };
```

Err 分支（原 `do_paste(stage, &raw_text, id, &raw_text, "failed", ...)`）改为第一参用 fallback_text、第四参（DB raw）仍 raw_text：

```rust
        Err(e) => {
            warn!("Final polish failed: {}, using fallback (display)", e);
            do_paste(stage, &fallback_text, id, &raw_text, "failed", config, app_handle, tx);
        }
```

> Ok 分支 `do_paste(&polished, id, &raw_text, "done", ...)` 不变。其他 `Stage::Polishing { .. }` 解构点（用 `{ .. }` 忽略）无需改。

- [x] **Step 7: 编译 + 测试**

Run: `cargo check -p octopus-desktop --all-targets && cargo test -p octopus-desktop`
Expected: 通过；`edited_display` dead_code 警告消失（已被多处消费）。

- [x] **Step 8: Commit**

```bash
git add crates/desktop/src/coordinator.rs
git commit -m "feat(desktop): 停止路径用 edited_display（无润色/兜底/最终润色失败均保留编辑）"
```

> **T8 e2e 重点**：流式中途编辑→停止→最终润色失败 的路径（Streaming `set_full` 后 edited_display 切片 `full[raw_len..]` 在越界时 Rust 返回空、不 panic，但需 e2e 确认拼接符合预期）。

---

## Task 7: 前端编辑交互

**Files:**
- Create: `crates/desktop/dist/result/icons/edit.svg`
- Modify: `crates/desktop/dist/result/index.html`

`#result-text` 默认不可编辑；双击或点编辑按钮 → `contenteditable=true` + 聚焦 + `enter_edit_mode`；`Cmd/Ctrl+Enter` / 完成按钮 / blur → `commit_edit`；input 防抖推 `update_edit_buffer`；编辑态加边框、禁 mouseleave 收起、冻结 update-result。

- [x] **Step 1: 新建 edit.svg**

`crates/desktop/dist/result/icons/edit.svg`：

```svg
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 20h9"/><path d="M16.5 3.5a2.121 2.121 0 0 1 3 3L7 19l-4 1 1-4Z"/></svg>
```

- [x] **Step 2: 加编辑按钮 + 完成按钮 HTML**

工具栏（`#tool-polish-now` 按钮 `</button>` 后）加编辑按钮：

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

- [x] **Step 3: 加 CSS**

`<style>`（`#tool-polish-now .icon` 行后）加编辑按钮图标：

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

- [x] **Step 4: 加编辑态 JS**

`<script>`（润色逻辑后）加：

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
      currentWindow.setFocus();
      resultText.focus();
      invoke('enter_edit_mode');
      updateEditBuffer();
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

    resultText.addEventListener('dblclick', enterEdit);
    btnEdit.addEventListener('click', (e) => { e.preventDefault(); enterEdit(); });

    document.addEventListener('keydown', (e) => {
      if (editing && (e.metaKey || e.ctrlKey) && e.key === 'Enter') {
        e.preventDefault();
        commitEdit();
      }
    });

    btnEditDone.addEventListener('mousedown', (e) => e.preventDefault());
    btnEditDone.addEventListener('click', (e) => { e.preventDefault(); commitEdit(); });

    resultText.addEventListener('blur', () => { if (editing) commitEdit(); });

    let editBufTimer = null;
    resultText.addEventListener('input', () => {
      clearTimeout(editBufTimer);
      editBufTimer = setTimeout(updateEditBuffer, 150);
    });

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

- [x] **Step 5: 编辑态冻结 update-result + 禁 mouseleave 收起**

`listen('update-result', ...)` 加 editing 守卫：

```js
    listen('update-result', (event) => {
      if (editing) return;               // 编辑态冻结，不覆盖用户输入
      resultText.textContent = event.payload;
      resultText.scrollTop = resultText.scrollHeight;
    });
```

`hideToolbar` 函数开头加守卫：

```js
    function hideToolbar() {
      if (!toolbarVisible || editing) return;   // 编辑态不收起
      /* …原逻辑… */
```

- [x] **Step 6: 手动构建验证** (待用户手动验证：T8 环境无 GUI；前端 dist 已构建并通过 snapshot/编译检查，行为验证需在本地 GUI 跑)

Run: `cargo run -p octopus-desktop`
手动验证（debug 构建自动开 devtools）：
1. 录音 → 说一句 → 结果窗出文本。
2. 双击文本 → 蓝边框 + 「完成编辑」→ 继续说话，窗口不刷新（硬暂停）。
3. 改字 → 点「完成编辑」→ 边框消失 → 继续说 → 新文本追加在编辑结果后。
4. 双击 → 改 → Cmd+Enter → 同样生效。
5. 编辑态点工具栏 ASR 按钮 → 先退出编辑（blur 提交）再弹浮层。

Expected: 行为符合预期，devtools 无 JS 报错。

- [x] **Step 7: Commit**

```bash
git add crates/desktop/dist/result/index.html crates/desktop/dist/result/icons/edit.svg
git commit -m "feat(desktop): 结果窗可编辑（双击/按钮进入，快捷键/按钮/失焦退出，硬暂停）"
```

---

## Task 8: 文档同步 + 端到端验证

**Files:**
- Modify: `docs/configuration.md`
- Modify: `docs/architecture.md`
- Modify: `docs/superpowers/specs/2026-06-18-editable-result-window-design.md`（状态行）
- Modify: `docs/superpowers/plans/2026-06-18-editable-result-window.md`（checkbox 勾选）

- [x] **Step 1: configuration.md 加编辑能力说明**

结果窗/工具栏相关段加：

```markdown
### 结果展示区编辑

录音过程中可随时修正识别/润色文本：
- **进入编辑**：双击结果区文本，或点工具栏 ✏️ 编辑按钮。单击不触发（防误触）。
- **编辑期间 ASR 硬暂停**（音频丢弃），改完恢复。
- **退出编辑**（择一）：`Cmd/Ctrl+Enter`、点「完成编辑」按钮、或失焦（点别处/工具栏）。
- 编辑后的文本作为后续展示与润色基准；新识别文本追加其上；停止粘贴时保留编辑。
- 编辑后再触发润色时，仅润色新增部分、保留已编辑（润色结果折回）。
- 未编辑时行为与旧版完全一致。
```

- [x] **Step 2: architecture.md 同步**

Transcript 相关段加：

```markdown
- `Transcript` 三文本分层：`edited ≻ polished ≻ raw`。`display_text()` = committed + increase；
  `full`（原始 ASR）独立保留为 DB `raw_text`。
- 编辑态：coordinator 主循环 `editing` 标志置位时，Streaming/VadSegmented tick 跳过喂引擎、
  只排空丢弃音频（硬暂停）。`commit_edit` 写回 transcript 并 `UPDATE edited_text`。
- 编辑×润色（spec §12）：`take_polish_input()` 返回 `(preserved=edited, to_polish=increase)`，
  LLM 仅润色新增；`on_polish_done` 在 `has_edit()` 时折回 `edited`（避免遮蔽丢字）。
- `transcriptions` 表加 `edited_text` 列（commit + 中间润色折回时写）。
- 停止路径：润色输入 = `take_polish_input`；无润色/兜底粘贴 = `edited_display()`；DB raw 仍 = `db_text()`。
```

- [x] **Step 3: spec 状态行置已实现**

`docs/superpowers/specs/2026-06-18-editable-result-window-design.md` 顶部 `> Status:` 行改为：

```
> Status: ✅ 已实现（2026-06-18，plan 2026-06-18-editable-result-window.md v2）。会话中编辑（双击/按钮进入，快捷键/按钮/失焦退出，硬暂停）+ 三文本分层 + 编辑×润色折回 + DB edited_text 均已落地。
```

- [x] **Step 4: plan checkbox 勾选**

本文件所有 `- [ ]` → `- [x]`（实现者确认每步已做）。

- [x] **Step 5: 删库重建 + 全流程 e2e** (待用户手动验证：T8 环境无 GUI，e2e 检查清单已附在 T8 任务报告中)

```bash
cp ~/.octopus/octopus.db ~/.octopus/octopus.db.bak.$(date +%s)
rm ~/.octopus/octopus.db
cargo run -p octopus-desktop
```

手动 e2e（三种 PolishMode 各验一次）：
1. `polish_mode=2`：录音 → 说一段（出中间润色）→ 双击改错 → 完成 → 继续说 → 停顿触发润色（仅润色新增，edited 保留）→ 停止 → 粘贴 = edited + 润色后新增；DB `edited_text` 非空、`raw_text` 为原始 ASR。
2. `polish_mode=0`：录音 → 双击改 → 完成 → 停止 → 粘贴 = edited。
3. 编辑态按停止热键 → edit_buffer 提交编辑后停止 → 粘贴含编辑。
4. `sqlite3 ~/.octopus/octopus.db "SELECT raw_text, polished_text, edited_text FROM transcriptions ORDER BY id DESC LIMIT 3;"` 验证三列互不干扰。

- [x] **Step 6: workspace 全量编译 + 测试**

```bash
cargo check --workspace --all-targets
cargo test --workspace
```
Expected: 全绿。

- [x] **Step 7: Commit**

```bash
git add docs/configuration.md docs/architecture.md docs/superpowers/specs/2026-06-18-editable-result-window-design.md docs/superpowers/plans/2026-06-18-editable-result-window.md
git commit -m "docs: 同步结果窗可编辑（configuration/architecture/spec 状态/plan v2）"
```

---

## 验证总结

| 场景 | 预期 |
|---|---|
| 未编辑（任意 mode） | 行为与旧版完全一致（display 公式等价；polish(None, full)） |
| 双击编辑 → 完成 → 继续说 | 新文本追加在 edited 后；display = edited + increase |
| 编辑后停顿润色（mode=2） | take_polish_input=(edited, 新增)；LLM 仅润色新增；结果折回 edited，无丢字 |
| 停止粘贴（有润色） | 粘贴 = polish(edited, 新增) 的结果；DB raw 仍原始 ASR |
| 停止粘贴（无润色/兜底） | 粘贴 = edited_display（含 edited） |
| 编辑态点 toolbar | 先退出编辑（blur 提交）再执行按钮动作 |
| 编辑态按停止热键 | edit_buffer 提交编辑后停止 |
| DB | raw/polished/edited 三列独立、互不干扰 |

## 关键文件

- `crates/desktop/src/transcript.rs`（edited + commit_edit + display 优先级链 + edited_display + take_polish_input + on_polish_done 折回）
- `crates/llm/src/prompt.rs` + `client.rs`（user_prompt/preserved + polish 签名）
- `crates/infra/src/db.sql` + `db.rs`（edited_text 列 + update_edited_text + 历史查询）
- `crates/desktop/src/coordinator.rs`（Command/DbCommand 变体 + editing 闸门 + commit→DB + 润色接线 take_polish_input/preserved + 折回 DB 分支 + 停止路径 edited_display）
- `crates/desktop/src/main.rs`（invoke_handler 注册 3 命令）
- `crates/desktop/dist/result/index.html` + `icons/edit.svg`（编辑交互）
