# Toggle 停止时立即润色结果丢失修复 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 修复用户点击「立即润色」后按 Toggle 结束录音时，润色结果丢失、只粘贴原文的 bug。

**Architecture:** 新增 `Stage::StoppingPolish` 过渡阶段。Toggle 停止时若仍有进行中的立即润色（`polish_pending == true`），不再清除 pending，而是进入 `StoppingPolish` 持有 transcript 等待 `Command::PolishDone`；PolishDone 到达后按 `polish_mode` 走 final 路径。抽取公共收尾函数 `finalize_after_stop` 统一三个 Toggle 停止分支。

**Tech Stack:** Rust、Tauri 2、mpsc channel 状态机

**Spec:** [`docs/superpowers/specs/2026-06-21-toggle-stop-polish-race-design.md`](../specs/2026-06-21-toggle-stop-polish-race-design.md)

---

## 文件结构

| 文件 | 职责 | 改动类型 |
|------|------|----------|
| `crates/desktop/src/coordinator.rs` | 状态机主逻辑 | 修改（新增 stage + helper + 改造 Toggle/PolishDone/Cancel/Discard） |
| `docs/architecture.md` | 架构文档 | 修改（状态机章节同步） |

**注意**：`crates/desktop/src/transcript.rs` **无需修改**——`polish_pending()` / `on_polish_done()` / `on_polish_failed()` / `display_text()` / `db_text()` 等方法已存在且语义正确。

---

## Task 1: 新增 `Stage::StoppingPolish` 变体 + `stage_name` 扩展

**Files:**
- Modify: `crates/desktop/src/coordinator.rs:148-155`（`Stage` enum 定义）
- Modify: `crates/desktop/src/coordinator.rs:2556-2570`（`stage_name` 函数）

- [ ] **Step 1: 在 `Stage` enum 中新增 `StoppingPolish` 变体**

在 `crates/desktop/src/coordinator.rs` 的 `Stage` enum 中，`WaitingCompletion` 变体之后、`Polishing` 变体之前插入：

```rust
    /// Toggle 停止录音后，仍有进行中的立即润色（PolishNow 未返回）。
    /// 持有 transcript 等待 `Command::PolishDone` 到达，再按 polish_mode 决定后续路径。
    /// 修复 bug：原实现直接 `clear_polish_pending` + 走 final 路径，
    /// 导致立即润色结果被 stage 切换丢弃 + 最终润色因 polish_mode=0 跳过 → 只粘贴原文。
    StoppingPolish {
        transcript: Transcript,
    },
```

- [ ] **Step 2: 在 `stage_name` 函数中新增 `StoppingPolish` arm**

在 `stage_name` 函数的 match 中添加（位置在 `WaitingCompletion` 之后）：

```rust
        Stage::StoppingPolish { .. } => "StoppingPolish",
```

- [ ] **Step 3: 构建验证**

Run: `cargo build -p octopus-desktop --features embedded,cloud 2>&1 | tail -5`
Expected: PASS（新变体未被使用会有 dead_code 警告，但不应报错；后续 Task 会用到）

---

## Task 2: 抽取 `finalize_after_stop` 公共收尾函数

**Files:**
- Modify: `crates/desktop/src/coordinator.rs`（在 `start_final_polish_or_paste` 函数之前插入新函数）

- [ ] **Step 1: 在 `start_final_polish_or_paste` 之前插入 `finalize_after_stop` 函数**

在 `crates/desktop/src/coordinator.rs` 中找到 `/// 开始最终润色或粘贴阶段（异步最终润色，防止阻塞协调器线程）。` 这行注释（`start_final_polish_or_paste` 的文档注释），在其**之前**插入：

```rust
/// Toggle 停止录音后的统一收尾：决定走 final 路径还是等待 pending 立即润色。
///
/// **修复 bug**：原实现直接 `transcript.clear_polish_pending()` 后走 final 路径，
/// 导致：(1) 立即润色的 `PolishDone` 回来时 stage 已切换 → 结果被丢弃；
/// (2) 若 `polish_mode=0`，最终润色被跳过 → 只粘贴原文，DB 也只存原文。
///
/// 现在的语义：若仍有 pending 的立即润色，进入 `StoppingPolish` 持有 transcript，
/// `PolishDone` 到达后在 `handle_polish_done` 中走 final 路径，把立即润色结果纳入最终文本。
///
/// **优化**：若 polished 非空且无新增 ASR（has_increase=false），立即润色已覆盖全部文本，
/// 跳过最终润色（mode=1/2 也跳过），直接 paste，避免平白多一次 LLM 调用。
fn finalize_after_stop(
    stage: &mut Stage,
    transcript: Transcript,
    config: &AppConfig,
    app_handle: &tauri::AppHandle,
    tx: &Sender<Command>,
) {
    // 1. 立即润色仍在途：等其完成再走 final 路径（避免丢弃润色结果）
    if transcript.polish_pending() {
        info!("Toggle stop: polish_pending=true, entering StoppingPolish");
        crate::tray::update_tray_label(app_handle, crate::tray::TrayState::Processing);
        crate::result_window::show_result(app_handle, "⏳ 等待润色完成...");
        *stage = Stage::StoppingPolish { transcript };
        return;
    }
    // 2. 无 pending：检查是否可以跳过最终润色
    //    若 polished 非空且无新增 ASR（has_increase=false），立即润色已覆盖全部文本
    let skip_final_polish = !transcript.polished().is_empty() && !transcript.has_increase();
    // 3. 句末标点补全 + display_text 计算（与原 final 路径一致）
    let combined = if let Some(edited) = transcript.edited_display() {
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
    if combined.is_empty() {
        *stage = Stage::Idle;
        crate::result_window::hide_result(app_handle);
        crate::tray::update_tray_label(app_handle, crate::tray::TrayState::Idle);
        return;
    }
    crate::result_window::show_result(app_handle, &transcript.display_text());
    if skip_final_polish {
        // 立即润色已覆盖全部文本，直接 paste（polish_status="done"）
        info!("Toggle stop: skip final polish (polished covers all, no increase)");
        let display = transcript.display_text();
        let raw = transcript.db_text();
        do_paste(stage, &display, transcript.id, &raw, "done", config, app_handle, tx);
    } else {
        // 走原 final 路径（按 polish_mode 决定是否润色）
        start_final_polish_or_paste(stage, &combined, transcript, config, app_handle, tx);
    }
}
```

- [ ] **Step 2: 构建验证**

Run: `cargo build -p octopus-desktop --features embedded,cloud 2>&1 | tail -5`
Expected: PASS（函数未被调用会有 dead_code 警告，下一个 Task 消除）

---

## Task 3: 改造 `handle_toggle` 的 `Streaming` 分支

**Files:**
- Modify: `crates/desktop/src/coordinator.rs:873-941`（`handle_toggle` 的 `Stage::Streaming` arm）

- [ ] **Step 1: 用 `finalize_after_stop` 替换 Streaming 停止分支的收尾逻辑**

找到 `handle_toggle` 中的 `Stage::Streaming { ... } => { ... }` 分支（约 873 行起），将其中的：
- 删除 `transcript.clear_polish_pending();` 这一行
- 删除停止路径中计算 `combined` + 判空 + `show_result` + `start_final_polish_or_paste` 的整段代码

替换为：

```rust
        Stage::Streaming {
            engine: streaming_engine,
            transcript,
            streaming_active,
            ..
        } => {
            // 流式模式：停止流式，获取最终文本，粘贴
            info!("Toggle: stopping streaming, finalizing");

            // 停止 tick
            streaming_active.store(false, Ordering::Relaxed);

            // 获取最终音频和识别结果
            let final_samples = audio.drain_samples();
            if !final_samples.is_empty() {
                if let Err(e) = streaming_engine.accept_samples(&final_samples, false) {
                    warn!("Error processing final samples: {}", e);
                }
            }

            let final_text = match streaming_engine.finish() {
                Ok(text) => text,
                Err(e) => {
                    error!("Streaming finish failed: {}", e);
                    // 引擎兜底：edited 非空优先（保留编辑），否则 raw
                    transcript
                        .edited_display()
                        .unwrap_or_else(|| transcript.db_text())
                }
            };

            // 重置引擎
            streaming_engine.reset();

            // 停止录音
            let _ = audio.stop();

            if !final_text.is_empty() {
                transcript.set_full(&final_text);
            }

            info!("Final streaming text: '{}'", transcript.db_text());

            let tr = std::mem::replace(transcript, Transcript::new(0, PolishMode::Disabled));
            finalize_after_stop(stage, tr, config, app_handle, tx);
        }
```

- [ ] **Step 2: 构建验证**

Run: `cargo build -p octopus-desktop --features embedded,cloud 2>&1 | tail -5`
Expected: PASS

---

## Task 4: 改造 `handle_toggle` 的 `VadSegmented` 分支

**Files:**
- Modify: `crates/desktop/src/coordinator.rs:785-878`（`handle_toggle` 的 `Stage::VadSegmented` arm）

- [ ] **Step 1: 用 `finalize_after_stop` 替换 VadSegmented 停止分支的收尾逻辑**

找到 `handle_toggle` 中的 `Stage::VadSegmented { ... } => { ... }` 分支（约 785 行起），将其中的：
- 删除 `transcript.clear_polish_pending();` 这一行（约 827 行，注释 `// 忽略中间润色的 pending 结果（最终润色会重新处理）` 也一并删除）
- 删除 `else { ... }` 分支中计算 `final_text` + 判空 + `start_final_polish_or_paste` 的整段代码

替换 `else { ... }` 分支为：

```rust
            } else {
                // 所有识别已完成：直接收尾（按 polish_pending 决定是否等润色）
                let tr = std::mem::replace(transcript, Transcript::new(0, PolishMode::Disabled));
                finalize_after_stop(stage, tr, config, app_handle, tx);
            }
```

完整的 VadSegmented 分支应该形如：

```rust
        Stage::VadSegmented {
            ref mut filter_vad,
            audio_buffer,
            overlap_tail,
            transcript,
            has_speech,
            active_count,
            next_seq,
            completed_seq,
            completed_results,
            tick_active,
            ..
        } => {
            // VAD 伪流式：停止 tick，发送剩余缓冲区，决定等待完成或直接粘贴
            info!("Toggle: stopping VadSegmented (active_count={})", active_count);

            // 停止 tick 线程
            tick_active.store(false, Ordering::Relaxed);

            // 停止录音并排空剩余音频
            let remaining = audio.stop().unwrap_or_default();
            if !remaining.is_empty() {
                audio_buffer.extend_from_slice(&remaining);
            }

            // 如果缓冲区有语音，发送最后一次识别
            if *has_speech && !audio_buffer.is_empty() {
                let mut send_buffer = overlap_tail.clone();
                send_buffer.extend_from_slice(audio_buffer);
                let speech_samples = filter_speech_from_buffer(filter_vad, &send_buffer);
                if !speech_samples.is_empty() {
                    let seq = *next_seq;
                    *next_seq += 1;
                    *active_count += 1;
                    spawn_offline_transcription_with_seq(
                        engine, config, tx, speech_samples, seq, transcript.id,
                    );
                }
            }

            let active = *active_count;
            let cseq = *completed_seq;
            let cresults = std::mem::take(completed_results);

            if active > 0 {
                // 还有识别任务在跑：进 WaitingCompletion 等所有 seq 完成
                let tr = std::mem::replace(transcript, Transcript::new(0, PolishMode::Disabled));
                *stage = Stage::WaitingCompletion {
                    transcript: tr,
                    active_count: active,
                    completed_seq: cseq,
                    completed_results: cresults,
                };
            } else {
                // 所有识别已完成：直接收尾（按 polish_pending 决定是否等润色）
                let tr = std::mem::replace(transcript, Transcript::new(0, PolishMode::Disabled));
                finalize_after_stop(stage, tr, config, app_handle, tx);
            }
        }
```

- [ ] **Step 2: 构建验证**

Run: `cargo build -p octopus-desktop --features embedded,cloud 2>&1 | tail -5`
Expected: PASS

---

## Task 5: 改造 `WaitingCompletion` 收齐后的收尾路径

**Files:**
- Modify: `crates/desktop/src/coordinator.rs`（`handle_transcription_done` 函数，查找所有 seq 完成后的收尾代码）

**背景**：VadSegmented 的 `active_count > 0` 分支会进 `WaitingCompletion`，等所有 `TranscriptionDone` 收齐后需要收尾。原代码可能也有 `clear_polish_pending`，需要改为调 `finalize_after_stop`。

- [ ] **Step 1: 定位 WaitingCompletion 收齐后的收尾代码**

Run: `grep -n 'clear_polish_pending\|WaitingCompletion' crates/desktop/src/coordinator.rs`

查找 `WaitingCompletion` 中 `active_count` 减到 0 时的收尾代码。

- [ ] **Step 2: 移除 `clear_polish_pending` 调用，改用 `finalize_after_stop`**

在 `WaitingCompletion` 收齐所有 seq 后的收尾路径中：
- 删除 `transcript.clear_polish_pending();` 调用（如果存在）
- 将计算 `final_text` + `start_final_polish_or_paste` 的逻辑替换为 `finalize_after_stop(stage, transcript, config, app_handle, tx)`

**注意**：如果原代码在此处有句末标点补全逻辑（`format!("{}。", ...)`），`finalize_after_stop` 已内置此逻辑，无需重复。

- [ ] **Step 3: 构建验证**

Run: `cargo build -p octopus-desktop --features embedded,cloud 2>&1 | tail -5`
Expected: PASS

---

## Task 6: 改造 `finalize_cloud` 函数（CloudStreaming 无 session 路径）

**Files:**
- Modify: `crates/desktop/src/coordinator.rs:1143-1176`（`finalize_cloud` 函数）

**背景**：CloudStreaming Toggle 停止时，无活跃 session 的分支调 `finalize_cloud`。此函数原代码直接调 `start_final_polish_or_paste`，需要改为先判断 `polish_pending`。但 CloudStreaming 有特殊逻辑（append partial + ensure INSERT），不能直接用 `finalize_after_stop`。

- [ ] **Step 1: 在 `finalize_cloud` 中加入 polish_pending 判断**

找到 `finalize_cloud` 函数（约 1143 行），在 `start_final_polish_or_paste` 调用之前插入 polish_pending 判断。修改后的 `finalize_cloud` 应形如：

```rust
fn finalize_cloud(
    stage: &mut Stage,
    mut transcript: Transcript,
    current_partial: String,
    config: &AppConfig,
    app_handle: &tauri::AppHandle,
    tx: &Sender<Command>,
) {
    // 即使无 session 或 close 无返回，也提交未 commit 的 partial
    if !current_partial.is_empty() {
        if !transcript.full().is_empty() && !transcript.full().ends_with('，') {
            transcript.append_segment("，");
        }
        transcript.append_segment(&current_partial);
    }

    let combined = transcript.db_text();
    if combined.is_empty() {
        *stage = Stage::Idle;
        crate::result_window::hide_result(app_handle);
        crate::tray::update_tray_label(app_handle, crate::tray::TrayState::Idle);
        return;
    }

    // 确保 DB 记录已 INSERT
    if let Err(e) = update_transcription_raw(&mut transcript, &config.asr_engine, "streaming") {
        warn!("CloudStreaming finalize INSERT failed: {}", e);
    }

    // 立即润色仍在途：进 StoppingPolish 等 PolishDone
    // （CloudStreaming 的 partial 已 append 到 transcript.full，不会再增长）
    if transcript.polish_pending() {
        info!("CloudStreaming finalize: polish_pending=true, entering StoppingPolish");
        crate::tray::update_tray_label(app_handle, crate::tray::TrayState::Processing);
        crate::result_window::show_result(app_handle, "⏳ 等待润色完成...");
        *stage = Stage::StoppingPolish { transcript };
        return;
    }

    crate::result_window::show_result(app_handle, &transcript.display_text());
    start_final_polish_or_paste(stage, &combined, transcript, config, app_handle, tx);
}
```

- [ ] **Step 2: 构建验证**

Run: `cargo build -p octopus-desktop --features embedded,cloud 2>&1 | tail -5`
Expected: PASS

---

## Task 7: 改造 `handle_polish_done` 新增 `StoppingPolish` arm

**Files:**
- Modify: `crates/desktop/src/coordinator.rs:2377-2450`（`handle_polish_done` 函数）

- [ ] **Step 1: 在 `handle_polish_done` 的 stage match 中新增 `StoppingPolish` arm**

找到 `handle_polish_done` 函数（约 2377 行），在现有的 stage match 中，`_ => { ... 丢弃 ... }` 之前插入 `StoppingPolish` arm：

```rust
        Stage::StoppingPolish { transcript } => {
            // 跨会话护栏
            if transcript.id != session_id {
                warn!(
                    "PolishDone discarded: session_id mismatch (polish={}, transcript={}) — 跨会话护栏",
                    session_id, transcript.id
                );
                use tauri::Emitter;
                let _ = app_handle.emit("polish-done", ());
                return;
            }
            // 写入润色结果
            match result {
                Ok(polished) => {
                    if polished.is_empty() {
                        warn!("Polish returned empty, keeping previous");
                        transcript.on_polish_failed();
                    } else {
                        transcript.on_polish_done(polished.clone());
                        let cmd = if transcript.has_edit() {
                            DbCommand::UpdateEdited {
                                id: transcript.id,
                                edited_text: polished,
                            }
                        } else {
                            DbCommand::UpdatePolished {
                                id: transcript.id,
                                text: transcript.polished().to_string(),
                                status: "done".to_string(),
                                model: Some(config.polish_llm.clone()),
                            }
                        };
                        if let Err(e) = get_db_sender().send(cmd) {
                            warn!("Queue DB update_polish_result failed: {}", e);
                        }
                    }
                }
                Err(e) => {
                    warn!("Polish failed: {}, keeping previous", e);
                    transcript.on_polish_failed();
                }
            }
            // 通知前端：润色完成
            use tauri::Emitter;
            let _ = app_handle.emit("polish-done", ());
            // PolishDone 处理完成（pending 已清），走 final 路径
            let tr = std::mem::replace(transcript, Transcript::new(0, PolishMode::Disabled));
            finalize_after_stop(stage, tr, config, app_handle, tx);
            return;
        }
```

**关键**：此 arm 末尾的 `return` 确保不落入后续的 `_ =>` 丢弃分支。

- [ ] **Step 2: 构建验证**

Run: `cargo build -p octopus-desktop --features embedded,cloud 2>&1 | tail -5`
Expected: PASS

---

## Task 8: 改造 `handle_cancel` 新增 `StoppingPolish` arm

**Files:**
- Modify: `crates/desktop/src/coordinator.rs:2072-2140`（`handle_cancel` 函数）

- [ ] **Step 1: 在 `handle_cancel` 的 stage match 中新增 `StoppingPolish` arm**

找到 `handle_cancel` 函数（约 2072 行）。在现有的 stage match 中（`Polishing` / `WaitingCompletion` 等 arm 附近）添加：

```rust
        Stage::StoppingPolish { transcript, .. } => {
            info!("Cancel: stopping StoppingPolish");
            // 立即润色结果将被丢弃，回到 Idle
        }
```

**注意**：`handle_cancel` 末尾已有统一的 DB 清理逻辑（检查 `db_inserted` → `DbCommand::Delete`），`StoppingPolish` 的 transcript 会被该逻辑覆盖（`StoppingPolish { transcript, .. }` 匹配后，末尾的 `db_id_to_delete` 提取逻辑需要新增 `StoppingPolish` arm，见 Step 2）。

- [ ] **Step 2: 在 `handle_cancel` 末尾的 `db_id_to_delete` 提取逻辑中新增 `StoppingPolish` arm**

找到 `handle_cancel` 中提取 `db_id_to_delete` 的 match 表达式（约 2118-2127 行），添加 `StoppingPolish` arm：

```rust
        Stage::StoppingPolish { transcript, .. } => {
            if transcript.db_inserted() { Some(transcript.id) } else { None }
        }
```

- [ ] **Step 3: 构建验证**

Run: `cargo build -p octopus-desktop --features embedded,cloud 2>&1 | tail -5`
Expected: PASS

---

## Task 9: 改造 `handle_discard` 新增 `StoppingPolish` arm

**Files:**
- Modify: `crates/desktop/src/coordinator.rs`（`handle_discard` 函数）

- [ ] **Step 1: 定位 `handle_discard` 函数**

Run: `grep -n 'fn handle_discard' crates/desktop/src/coordinator.rs`

- [ ] **Step 2: 在 `handle_discard` 中新增 `StoppingPolish` arm**

`handle_discard` 与 `handle_cancel` 共享停止逻辑，但额外 finalize DB 记录。找到其 stage match，添加 `StoppingPolish` arm（与 `Polishing` arm 类似，finalize DB 记录）：

```rust
        Stage::StoppingPolish { transcript, .. } => {
            info!("Discard: finalizing StoppingPolish");
            // finalize DB 记录（保留识别历史）
            let raw_text = transcript.db_text();
            let edited = transcript.edited_display();
            let polished = if let Some(ref e) = edited { e.clone() } else { transcript.polished().to_string() };
            let polish_status = if polished.is_empty() { "off" } else { "done" };
            if let Err(e) = get_db_sender().send(DbCommand::Finalize {
                id: transcript.id,
                raw_text,
                polished_text: if polished.is_empty() { None } else { Some(polished) },
                polish_status: polish_status.to_string(),
                polish_model: Some(config.polish_llm.clone()),
                duration_ms: None,
            }) {
                warn!("Discard: queue DB Finalize failed: {}", e);
            }
        }
```

**注意**：需检查 `handle_discard` 是否已有 `Polishing` arm 的 finalize 逻辑模板，参照其写法。如果 `handle_discard` 的 finalize 逻辑与上述不同，以现有 `Polishing` arm 的写法为准。

- [ ] **Step 3: 构建验证**

Run: `cargo build -p octopus-desktop --features embedded,cloud 2>&1 | tail -5`
Expected: PASS

---

## Task 10: 改造 `handle_toggle` 新增 `StoppingPolish` arm（忽略）

**Files:**
- Modify: `crates/desktop/src/coordinator.rs:997-1015`（`handle_toggle` 的 busy stage 忽略分支）

- [ ] **Step 1: 在 `handle_toggle` 中新增 `StoppingPolish` 忽略 arm**

找到 `handle_toggle` 中忽略 busy stage 的 match 分支（`WaitingCompletion` / `Polishing` / `Pasting` 等返回 `debug!("Toggle ignored: ...")` 的位置），添加：

```rust
        Stage::StoppingPolish { .. } => {
            debug!("Toggle ignored: waiting for polish to complete");
        }
```

- [ ] **Step 2: 构建验证**

Run: `cargo build -p octopus-desktop --features embedded,cloud 2>&1 | tail -5`
Expected: PASS

---

## Task 11: 全量构建 + 测试验证

**Files:**
- 无文件修改，仅验证

- [ ] **Step 1: 全量构建（cloud + 非 cloud）**

Run:
```bash
cargo build -p octopus-desktop --features embedded,cloud 2>&1 | tail -10
cargo build -p octopus-desktop --features embedded 2>&1 | tail -10
```
Expected: 两个构建均 PASS，0 warnings

- [ ] **Step 2: 运行测试**

Run:
```bash
cargo test -p octopus-desktop --features embedded,cloud 2>&1 | tail -15
```
Expected: 所有测试 PASS（67+ passed）

- [ ] **Step 3: 检查 warnings**

Run: `cargo build -p octopus-desktop --features embedded,cloud 2>&1 | grep -i warning`
Expected: 无输出（0 warnings）

---

## Task 12: 同步 architecture.md 文档

**Files:**
- Modify: `docs/architecture.md`（核心状态机章节 + 取消录音章节）

- [ ] **Step 1: 更新核心状态机章节**

在 `docs/architecture.md` 的「核心状态机（Coordinator）」章节，更新模式说明：

找到：
```
- 流式模式：Streaming → (Polishing) → Pasting
```

在其下方添加 `StoppingPolish` 的说明（在 `Polishing` 的过渡说明位置）：

```
- （新增）Toggle 停止时若有进行中的立即润色：Streaming/VadSegmented/CloudStreaming → StoppingPolish → (Polishing) → Pasting
```

- [ ] **Step 2: 更新「取消录音（Cancel）」章节**

找到 `docs/architecture.md` 中 `- **取消录音（Cancel）**` 的段落，在其末尾补充 `StoppingPolish` 的说明：

```
**StoppingPolish 阶段**（Toggle 停止时立即润色仍在途）：Cancel 丢弃在途润色结果 + 删除 DB 脏数据（同其他阶段的 Cancel 语义）。
```

- [ ] **Step 3: 在 spec/plan 中勾选完成**

回到本 plan 文档，把所有 checkbox 标记为 `[x]`。

---

## Task 13: 提交

- [ ] **Step 1: 提交所有改动**

```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/setting-ui2
git add -A
git commit -m "$(cat <<'EOF'
fix(desktop): 修复 Toggle 停止时立即润色结果丢失

根因：handle_toggle 的三个停止分支（Streaming/VadSegmented/CloudStreaming）
原执行 transcript.clear_polish_pending() 后走 final 路径，导致：
1. 立即润色的 Command::PolishDone 回来时 stage 已切换 → 结果被丢弃
2. polish_mode=0 时最终润色被跳过 → 只粘贴原文，DB 也只存原文

修复：新增 Stage::StoppingPolish 过渡阶段。Toggle 停止时若仍有 pending
的立即润色，进入 StoppingPolish 持有 transcript 等待 PolishDone，完成后
按 polish_mode 走 final 路径（mode=0 直接 paste display_text 含 polished+increase；
mode=1/2 触发最终润色）。抽取 finalize_after_stop 公共收尾函数统一三个分支。

spec: docs/superpowers/specs/2026-06-21-toggle-stop-polish-race-design.md
plan: docs/superpowers/plans/2026-06-21-toggle-stop-polish-race.md

💘 Generated with Crush

Assisted-by: Crush:glm-5.1
EOF
)"
```

- [ ] **Step 2: 同步到 main**

```bash
cd /Users/wudarui/workspace/agent/octopus
git merge --ff-only feature/setting-ui2
```

---

## Self-Review 检查

### Spec coverage

| Spec 章节 | 对应 Task |
|-----------|-----------|
| §2.3 新增 Stage | Task 1 |
| §2.4 Toggle 停止路径改造（Streaming） | Task 3 |
| §2.4 Toggle 停止路径改造（VadSegmented） | Task 4 |
| §2.4 Toggle 停止路径改造（WaitingCompletion 收齐） | Task 5 |
| §2.4 Toggle 停止路径改造（CloudStreaming 无 session） | Task 6 |
| §2.4 移除所有 clear_polish_pending | Task 3/4/5 |
| §2.4 抽取 finalize_after_stop | Task 2 |
| §2.5 handle_polish_done 改造 | Task 7 |
| §2.6 Cancel 处理 | Task 8 |
| §2.6 Discard 处理 | Task 9 |
| §2.6 Toggle 忽略 | Task 10 |
| §2.7 UI 反馈 | Task 2（finalize_after_stop 内置） |
| §5 验证方法 | Task 11 |
| 文档同步 | Task 12 |

### Placeholder scan

- 无 TBD/TODO ✓
- 每个 Step 都有具体代码或命令 ✓
- Task 5/9 的"查找"步骤有具体 grep 命令 ✓

### Type consistency

- `Stage::StoppingPolish { transcript: Transcript }` 全程一致 ✓
- `finalize_after_stop(stage, transcript, config, app_handle, tx)` 签名全程一致 ✓
- `on_polish_done` / `on_polish_failed` / `polish_pending` 方法名与 transcript.rs 一致 ✓
