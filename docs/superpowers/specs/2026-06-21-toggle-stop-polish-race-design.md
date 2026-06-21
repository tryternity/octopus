# Toggle 停止时立即润色结果丢失修复

**日期**：2026-06-21
**类型**：Bug 修复（涉及状态机改造）
**相关文件**：`crates/desktop/src/coordinator.rs`、`crates/desktop/src/transcript.rs`

## 1. 问题描述

### 1.1 复现步骤

1. 用户说话（ASR 持续累积 `transcript.full`）
2. 用户点击工具栏「立即润色」按钮 → `handle_polish_now` 发起异步 LLM 润色请求（`Command::PolishDone` 待返回）
3. **LLM 请求在途时**（典型 1~3 秒），用户按快捷键 Toggle 结束录音

### 1.2 错误行为

- 插入到光标位置的是**原始 ASR 文本**，不是润色后的文本
- SQLite 数据库 `transcriptions` 表只存了 `raw_text`，`polished_text` 为空
- 用户主动点的「立即润色」结果完全丢失

### 1.3 根因分析

`handle_toggle` 的三个停止分支（Streaming / VadSegmented / CloudStreaming）在停止录音时都会执行：

```rust
transcript.clear_polish_pending();  // 第 827/889/957 行
```

然后调用 `start_final_polish_or_paste`。

这导致两个问题：

**问题 A：立即润色的 `Command::PolishDone` 被丢弃**

`handle_polish_done`（第 2385 行）要求当前 stage 仍是活跃录音阶段（Streaming / VadSegmented / WaitingCompletion / CloudStreaming / CloudClosing），否则：

```rust
_ => {
    debug!("PolishDone ignored: stage={} 不是录音/等待阶段，润色结果丢弃", ...);
    let _ = app_handle.emit("polish-done", ());
    return;
}
```

Toggle 后 stage 已切换到 `Polishing` / `Pasting` / `Idle`，立即润色的结果到达时被直接丢弃。

**问题 B：最终润色被 `polish_mode` 跳过**

`start_final_polish_or_paste` 调用 `crate::config::llm_config(config)` 判断是否润色：

```rust
pub fn llm_config(cfg: &AppConfig) -> Option<...> {
    if cfg.polish_mode == PolishMode::Disabled {
        return None;  // mode=0 时直接返回 None
    }
    ...
}
```

当 `polish_mode=0`（Disabled）时返回 `None`，`start_final_polish_or_paste` 走 `None => do_paste` 分支，**跳过润色**直接粘贴。此时 `final_text = transcript.db_text()` = 原始 ASR 文本。

**两个问题叠加**：立即润色结果被丢弃 + 最终润色被跳过 → 用户得到的是原文。

### 1.4 影响范围

- `polish_mode=0` + 立即润色 + Toggle：**必现**，用户主动润色的结果丢失
- `polish_mode=1/2` + 立即润色 + Toggle（LLM 未及时返回）：立即润色结果丢失，但最终润色会重新润色全量文本（部分恢复，但浪费了一次 LLM 调用 + 用户看到的是第二次润色的结果而非第一次）

## 2. 设计

### 2.1 核心思路

**不再清除 `polish_pending`，而是等待立即润色完成后再走 final 路径。**

引入新 stage `Stage::StoppingPolish { transcript }`：Toggle 停止时，若 `transcript.polish_pending() == true`，把 transcript 移入此 stage 等待 `Command::PolishDone`；PolishDone 到达后按 `polish_mode` 决定后续路径。

### 2.2 立即润色语义澄清

**立即润色** = 中途触发的一次 LLM 润色，等同 `polish_mode=2` 的停顿润色。Toggle 停止后：

- **mode=0（Disabled）**：最终输出 = 立即润色结果（`polished`）+ 后续新增 ASR（`increase`）。**不再触发最终润色**。DB `polished_text` = 立即润色结果（不含 Toggle 后新增 ASR）。
- **mode=1/2**：触发最终润色（preserved 含已 polished 的部分 + increase 整体再润色一次）。DB `polished_text` = 最终润色结果。

### 2.3 新增 Stage

```rust
/// Toggle 停止录音后，仍有进行中的立即润色（PolishNow 未返回）。
/// 持有 transcript 等待 `Command::PolishDone` 到达，再按 polish_mode 决定后续路径。
StoppingPolish {
    transcript: Transcript,
},
```

### 2.4 Toggle 停止路径改造

三个 Toggle 停止分支（Streaming / VadSegmented / CloudStreaming）抽取公共收尾函数 `finalize_after_stop`：

```rust
fn finalize_after_stop(
    stage: &mut Stage,
    transcript: Transcript,
    config: &AppConfig,
    app_handle: &tauri::AppHandle,
    tx: &Sender<Command>,
) {
    // 1. 立即润色仍在途：等其完成再走 final 路径
    if transcript.polish_pending() {
        info!("Toggle stop: polish_pending=true, entering StoppingPolish");
        crate::tray::update_tray_label(app_handle, crate::tray::TrayState::Processing);
        crate::result_window::show_result(app_handle, "⏳ 等待润色完成...");
        *stage = Stage::StoppingPolish { transcript };
        return;
    }
    // 2. 无 pending：检查是否需要最终润色
    //    优化：若 polished 非空且无新增 ASR（has_increase=false），立即润色已覆盖全部文本，
    //    跳过最终润色（mode=1/2 也跳过），直接 paste，避免平白多一次 LLM 调用。
    let skip_final_polish = !transcript.polished().is_empty() && !transcript.has_increase();
    //    句末标点补全 + display_text 计算（与原 final 路径一致）
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
        // 立即润色已覆盖全部文本，直接 paste（polish_status="done"，DB polished_text=立即润色结果）
        info!("Toggle stop: skip final polish (polished covers all, no increase)");
        do_paste(stage, &transcript.display_text(), transcript.id, &transcript.db_text(), "done", config, app_handle, tx);
    } else {
        // 走原 final 路径（按 polish_mode 决定是否润色）
        start_final_polish_or_paste(stage, &combined, transcript, config, app_handle, tx);
    }
}
```

**`skip_final_polish` 判定逻辑**：
- `!transcript.polished().is_empty()`：立即润色成功（有 polished 文本）。若立即润色失败，polished 为空，仍需走最终润色兜底。
- `!transcript.has_increase()`：Toggle 时无新增 ASR 文本（`raw_len == full.len()`）。`has_increase` 仅在 `polish_mode=Intermediate` 时有意义（其他 mode 恒返回 false），但这不影响正确性——非 Intermediate mode 时 polished 已是全量润色，无 increase 概念，跳过最终润色同样正确。

**效果**：
- mode=0 + 立即润色成功 + 无新增：直接 paste display_text（polished），DB polished_text=立即润色结果
- mode=1/2 + 立即润色成功 + 无新增：**跳过最终润色**（原行为会再调一次 LLM），直接 paste display_text
- mode=1/2 + 立即润色成功 + 有新增：走最终润色（preserved=polished + increase 整体润色）
- mode=1/2 + 立即润色失败 + 任意：走最终润色（兜底）
- 任何 mode + 无立即润色：走原 final 路径（start_final_polish_or_paste）

### 2.5 `handle_polish_done` 改造

新增 `StoppingPolish` arm：

```rust
Stage::StoppingPolish { transcript } => {
    // 校验 session_id（跨会话护栏，与现有逻辑一致）
    if transcript.id != session_id { ...丢弃... return; }
    // 写入润色结果（on_polish_done / on_polish_failed）
    match result {
        Ok(polished) => { transcript.on_polish_done(polished); ...DB UpdatePolished... }
        Err(e) => { transcript.on_polish_failed(); }
    }
    // PolishDone 处理完成后，走 final 路径
    let tr = std::mem::replace(transcript, Transcript::new(0, PolishMode::Disabled));
    finalize_after_stop(stage, tr, config, app_handle, tx);
}
```

关键：`on_polish_done` 后 `polish_pending == false`，所以 `finalize_after_stop` 会走第 2 分支（无 pending），按 `polish_mode` 决定：
- mode=0：`llm_config` 返回 None → `do_paste(display_text)`（含 polished + increase）
- mode=1/2：`llm_config` 返回 Some → `start_final_polish_or_paste`（最终润色）

### 2.6 其他命令对 StoppingPolish 的处理

| 命令 | 处理 |
|------|------|
| `Toggle` | 忽略（busy，与现有 Polishing 一致） |
| `Cancel` | 删除 DB 脏数据（已有逻辑覆盖）+ 回 Idle |
| `Discard` | finalize DB 记录 + 回 Idle（与现有 Polishing stage 处理一致） |
| `PolishNow` | 忽略（已有 polish_pending，原有逻辑覆盖） |
| `PolishDone` | 见 §2.5 |
| `FinalPolishDone` | 忽略（此阶段不会有最终润色在途） |
| `StreamingTick` / `VadSegmentedTick` / `CloudStreamingTick` | 忽略（录音已停） |
| `TranscriptionDone` | 忽略（VadSegmented 已停） |

### 2.7 UI 反馈

进入 `StoppingPolish` 时：
- 托盘：`TrayState::Processing`（「处理中」）
- 结果窗：`show_result("⏳ 等待润色完成...")`
- 前端「立即润色」按钮：保持 disabled（polish_pending 期间）

PolishDone 到达后按 final 路径走，UI 由 `start_final_polish_or_paste` / `do_paste` 接管。

## 3. 不变量

1. **进入 StoppingPolish 前所有 ASR 源已停止**：Streaming 的 `finish()` / VadSegmented 的 tick 停止 / CloudStreaming 的 session 处理都在进 StoppingPolish 之前完成。StoppingPolish 期间 `transcript.full` 不会再增长。
2. **`polish_pending` 在 StoppingPolish 期间保持 true**：直到 `on_polish_done` / `on_polish_failed` 清除。
3. **PolishDone 的 session_id 护栏不变**：跨会话（Cancel + 重开）时旧 PolishDone 会被丢弃。
4. **VadSegmented active_count > 0 仍走 WaitingCompletion**：WaitingCompletion 收齐后会调 `finalize_after_stop`（不再 clear_polish_pending），若此时仍有 pending 则进 StoppingPolish。

## 4. 降级路径

- **LLM 失败**（PolishDone 返回 Err）：`on_polish_failed` 清 pending 但不写 polished → `finalize_after_stop` 走无 pending 分支 → mode=0 时 paste `display_text`（polished 为空则用 raw）；mode=1/2 时触发最终润色（兜底）。
- **用户 Cancel**：`handle_cancel` 已有逻辑覆盖（检测 `db_inserted` 并 Delete），StoppingPolish arm 加入即可。
- **用户 Discard**：`handle_discard` 已有逻辑覆盖（finalize DB 记录），StoppingPolish arm 加入即可。

## 5. 验证方法

- **单元测试**：transcript.rs 已有的 `take_polish_input` / `on_polish_done` 测试覆盖核心逻辑，无需新增
- **集成验证**（手动）：
  1. mode=0：说话 → 立即润色 → 立即 Toggle → 确认粘贴的是润色结果 + 新增 ASR
  2. mode=1：说话 → 立即润色 → 立即 Toggle → 确认触发最终润色 → 粘贴最终润色结果
  3. LLM 慢（模拟）：说话 → 立即润色 → 等 2s → Toggle → 确认进入「⏳ 等待润色完成...」→ PolishDone 后正常 paste
  4. Cancel during StoppingPolish：进入 StoppingPolish → 按 Esc → 确认 DB 记录被删除
- **构建验证**：`cargo build -p octopus-desktop --features embedded,cloud` + `cargo test -p octopus-desktop --features embedded,cloud`

## 6. 与现有代码的关系

- `Stage::Polishing`（最终润色）：**不变**。StoppingPolish 是其前置阶段（仅当 Toggle 时有 pending）
- `Stage::WaitingCompletion`（VadSegmented 等识别完成）：**不变**。收齐后调 `finalize_after_stop`
- `Stage::CloudClosing`（云端 close_async 等待）：**不变**。CloudStreamingDone 回来后调 `finalize_cloud` → `finalize_after_stop`
- `handle_polish_done`：**扩展**，新增 StoppingPolish arm
- `handle_cancel` / `handle_discard`：**扩展**，新增 StoppingPolish arm
- `handle_toggle`：**扩展**，新增 StoppingPolish arm（忽略，busy）
- `stage_name`：**扩展**，新增 StoppingPolish
