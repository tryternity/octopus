# 结果展示区可编辑设计

> Date: 2026-06-18
> Branch: `worktree-editable-result`（worktree 路径 `.claude/worktrees/editable-result`）
> Status: 设计已确认，待 spec 复核 → 写 plan

## 1. 背景与目标

当前识别 + 润色后仍常出现错别字，而结果是录音停止后**立即自动粘贴**（`do_paste` 直接 spawn `paste::paste`），用户没有机会修正。

目标：在**会话进行中**允许用户随时编辑已识别文本——聚焦编辑 → ASR 硬暂停 → 改错 → 完成 → 继续识别。新识别的文本追加在用户编辑结果之后；后续润色也基于「编辑后文本 + 新增文本」。

非目标（明确不做）：
- **不在录音停止时新增「最终编辑闸门」**——停止流程维持现状（最终润色 → `do_paste`），只是粘贴文本天然包含会话中已编辑的内容。
- 不做全量历史回改（只编辑当前会话的展示文本）。

## 2. 现状（关键事实，设计依据）

- **`Transcript`**（`desktop/src/transcript.rs`）：三文本状态机。`full`（当前完整原始 ASR）+ `raw_len`（上次停顿快照的 char 边界）派生 `raw = full[..raw_len]` / `increase = full[raw_len..]`；`polished` 为润色结果。
  - `display_text()`：`polished` 非空或 mode=2 → `polished + increase`；否则 `full`。
  - `db_text()` = `full`（落库 raw_text）。
- **`PolishMode`**（`desktop/src/config.rs`）：`Disabled` / `FinalOnly` / `Intermediate`。编辑能力与润色模式**正交**，三个模式下都可编辑。
- **coordinator Stage**（`desktop/src/coordinator.rs`）：`Idle` / `Streaming` / `VadSegmented` / `WaitingCompletion` / `Polishing` / `Pasting`。无「编辑中」态。
  - `Streaming` 段含 `streaming_active: Arc<AtomicBool>`——置 false 即停 tick（音频不再喂引擎），这是天然的暂停杠杆。
  - `VadSegmented` 段由 tick 驱动分段；停 tick + 退出时 flush `audio_buffer` 即可避免编辑期音频被识别。
- **结果窗口前端**（`desktop/dist/result/index.html`）：`#result-text` 为 `<div>`，样式 `user-select:text; cursor:text` 但**非 `contenteditable`**，纯展示。监听 `show-result` / `update-result`（`resultText.textContent = payload`）/ `clear-result` / `hide-result`。已有工具栏（设置 / ASR / 降噪 / 润色模型 / 润色模式 / 立即润色）。
  - `container` 的 `mouseleave` 自动收起工具栏（`hide_toolbar=false` 时常驻）。
  - Esc → `cancel_recording` + hide。
- **`paste::paste`**（`desktop/src/paste.rs`）：`clipboard`（默认）/ `direct` / `none` 三种粘贴方式。编辑不影响粘贴机制，只改变喂给 `do_paste` 的文本。
- **DB**（`infra/src/db.sql`）：`transcriptions(id, created_at, engine, engine_mode, raw_text, polished_text, polish_status, polish_model, duration_ms, char_count)`。`finalize_transcription` 写 raw/polished/status/model/char_count/duration_ms。**无 edited 列**。
- **Tauri 命令**（`main.rs` `invoke_handler`）：已注册 `toolbar_state` / `cancel_recording` / `polish_now` / `result_window_ready` 等。

## 3. 交互设计

### 3.1 进入编辑（择一触发，防误触）

1. **文本区双击**：`#result-text` `dblclick` → 设 `contenteditable=true` + 聚焦 + `invoke('enter_edit_mode')`。
2. **工具栏 ✏️ 编辑按钮**：新增工具栏按钮，点击同上。

> 单击 / 单次聚焦**不**进入编辑（仅选中文本），降低误触。仅当处于活跃会话（Streaming / VadSegmented）且已有文本时按钮可点。

### 3.2 编辑态指示

- `#result-text:focus` 已有蓝色背景；编辑态额外加**边框**（如 `border: 1px solid #007aff`）。
- 工具栏高亮「✏️ 编辑中」（编辑按钮 `.active`）+ 显示「**完成编辑**」按钮。
- 编辑期间：禁用 `container.mouseleave` 自动收起；窗口保持可见并 `setFocus()`（保证键盘输入进入 webview）。

### 3.3 退出编辑（择一触发）

1. **快捷键** `CmdOrCtrl+Enter`。
2. 点「**完成编辑**」按钮。
3. **失焦**（`#result-text.blur`）：点屏幕别处 / 点 toolbar 任意按钮 / 点文本区外。

任一触发 → 前端取 `#result-text.innerText`，`invoke('commit_edit', { text })` → 后端提交（§5）→ 恢复识别。

### 3.4 ASR 暂停语义（硬暂停）

进入编辑 → coordinator **停止把音频喂给引擎**；编辑期间的音频**丢弃**（用户在打字/阅读而非说话，是噪声）。退出编辑 → 从新音频恢复喂入。

> 不做「软暂停 / 结果排队」：编辑期音频无价值，硬暂停最干净，且退出后引擎从干净边界续接，不污染段切分。

## 4. 三文本状态模型（核心）

`Transcript` 新增一个字段 `edited: String`，作为最高优先级层覆盖在 `polished` / `raw` 之上：

```rust
pub struct Transcript {
    full: String,       // 全部原始 ASR（不变；DB raw_text 来源）
    raw_len: usize,     // 已提交边界
    polished: String,   // committed 部分的润色结果（可空）
    edited: String,     // 【新增】用户编辑后的 committed 文本（可空；空=未编辑）
    // ...其余字段不变
}
```

**committed 前缀**（展示与润色的稳定基准，优先级递降）：

```
committed = edited 非空 ? edited
          : polished 非空 ? polished
          : full[..raw_len]            // raw
```

**increase** = `full[raw_len..]`（停顿后的新增 ASR，不变）。

| 派生量 | 定义 |
|---|---|
| `display_text()` | committed 前缀 + increase |
| polish 输入（停顿快照） | committed 前缀 + increase（= display） |
| `db_text()`（raw_text 落库） | `full`（原始 ASR，编辑/润色均不改） |
| `edited_text()`（新） | `edited`（空则 None） |

> 三文本**互不干扰**：`raw_text = full` 永远是原始 ASR；`polished_text` 仅润色成功填；`edited_text` 仅用户提交编辑填。三者独立字段、独立落库列。

## 5. 编辑提交语义（关键不变量）

用户在编辑态改的是 **display 全文**（= 进入编辑瞬间的 `display_text()`）。提交（commit_edit）时后端执行：

1. `edited = 用户文本`
2. `raw_len = full.chars().count()`（increase 清空）
3. `full`（raw ASR）**原样保留**

此后新 ASR 追加到 `full`，`increase = full[raw_len..]` = 新内容 → `display = edited + 新增`（满足"展示 = edited + 新识别"）。下次停顿润色的 snapshot 输入 = `display = edited + increase`（满足"润色输入 = edited + 新识别"）。

**多次编辑**：每次 commit 覆盖 `edited`、把 `raw_len` 推进到当时 `full` 末尾。空文本提交允许（`edited = ""` → committed 回退到 polished/raw）。

## 6. 数据流

```
活跃会话（Streaming/VadSegmented）：
  update-result(display) 持续刷新 ── 用户双击/点编辑按钮 ──▶ enter_edit_mode
                                                          │
                                              ① contenteditable=true + 聚焦 + setFocus
                                              ② coordinator: 硬暂停（streaming_active=false / 停 tick）
                                              ③ 冻结 display（不再 emit update-result）
                                                          │
                                            用户编辑 #result-text ◀── 键盘输入
                                                          │
  Cmd+Enter / 完成按钮 / blur ──▶ commit_edit(innerText)
                                                          │
                                              ① edited=text; raw_len=full.len(); full 不变
                                              ② coordinator: 恢复喂音频（streaming_active=true / 续 tick）
                                              ③ contenteditable=false
                                                          │
                                          ◀── 恢复 update-result(edited + 新增)
```

**停止时（不变）**：toggle 停止 → 最终润色 → `do_paste(display_text)`。粘贴文本天然含已编辑内容。

## 7. 边界与决策

| 情形 | 决策 |
|---|---|
| 编辑态收到 toggle（停止录音） | 先 `commit_edit` 提交，再走正常停止流程（用户改完直接按停止键可用） |
| 编辑态收到 `TranscriptionDone`（伪流式残留段） | 忽略（硬暂停下本不应有；防御性丢弃） |
| 编辑态收到 `update-result` 事件 | 前端忽略（ASR 已暂停，本不会来；防御性） |
| 编辑态点工具栏其他按钮（如切换 ASR） | blur → 先提交编辑，再执行该按钮动作 |
| 空文本提交 | `edited=""`，committed 回退 polished/raw |
| 失焦 vs 完成按钮竞态 | 提交统一走同一 `commit` 函数；完成按钮用 `mousedown preventDefault` 避免过早 blur 抢焦导致 click 丢失 |
| 窗口键盘焦点 | 进入编辑时 `currentWindow.setFocus()`，确保 contenteditable 能收键盘 |

## 8. DB 变更

`transcriptions` 加列：

```sql
edited_text TEXT,   -- 用户编辑后的最终文本（未编辑为 NULL）
```

- `db.sql` DDL 加列（开发阶段删库重建，不写 ALTER 迁移——与项目 db.sql 约定一致）。
- `finalize_transcription`：新增 `edited_text: Option<&str>` 参数；`char_count` 仍按 final display（`edited ?? polished ?? raw`）算。
- 读取/历史接口（`get_history` 等）：返回结构加 `edited_text` 字段；最终展示 = `edited_text ?? polished_text ?? raw_text`。

## 9. 波及面（重构清单）

- **`desktop/src/transcript.rs`**：
  - `Transcript` 加 `edited: String` 字段 + `new` 初始化。
  - `commit_edit(&mut self, text: &str)`：执行 §5 三步。
  - `display_text()` 改优先级链 `edited ≻ polished ≻ raw[..raw_len]` + increase。
  - 新增 `edited_text() -> Option<&str>`。
  - 现有测试更新（display 优先级）；新增 commit_edit 测试。
- **`desktop/src/coordinator.rs`**：
  - 新增 `Command::EnterEditMode` / `Command::CommitEdit { text }`；新命令方法。
  - 编辑态表示：coordinator 级 `editing: bool` 标志（或 `Stage::Editing` 暂存原 stage——plan 阶段定）。要求：编辑期间 Streaming 的 tick 不喂引擎（`streaming_active=false`）、VadSegmented 停 tick + 退出 flush `audio_buffer`。
  - `handle_toggle` 等停止路径：编辑态先 commit 再停止（§7）。
  - `update_result` 发送处：编辑态跳过（冻结）。
- **`desktop/src/result_window.rs`**：
  - 新增 `#[tauri::command] enter_edit_mode` / `commit_edit(text)`。
  - 编辑态冻结 `update-result`（前端配合）。
- **`desktop/dist/result/index.html`**：
  - `#result-text` 动态 `contenteditable` 切换；`dblclick` + 新增 ✏️ 编辑按钮 + 完成编辑按钮。
  - `CmdOrCtrl+Enter` / 完成按钮 / `blur` → `commit_edit`。
  - 编辑态：加边框、禁 `mouseleave` 收起、`setFocus`、忽略 `update-result`。
- **`desktop/src/main.rs`**：`invoke_handler` 注册 `enter_edit_mode` / `commit_edit`。
- **`infra/src/db.sql`** + **`infra/src/db.rs`**：`edited_text` 列 + `finalize_transcription` 参数；`TranscriptionRecord` 加字段；历史查询 SELECT 加列。
- **图标**：`dist/result/icons/` 加 `edit.svg`。

## 10. 测试策略

- **`transcript.rs` 单测**（核心）：
  - `commit_edit` 后：`edited` = 文本、`raw_len` = `full.len()`、`full` 不变、`increase` 清空。
  - `display_text()` 优先级：edited ≻ polished ≻ raw。
  - 编辑后续追加 ASR：`display = edited + 新 increase`。
  - 编辑后润色 snapshot 输入 = `display`。
  - 空提交回退；多次编辑覆盖。
- **coordinator**：编辑态停止音频喂入（streaming_active / tick 行为）；编辑态 toggle 先 commit 再停。
- **手动 e2e**：
  - 录音中双击编辑改错别字 → 完成 → 继续说 → 新文本追加在编辑结果后 → 停止 → 粘贴含修正。
  - 编辑态点工具栏切换 ASR → 先提交编辑再切换。
  - 三种 PolishMode 下编辑均生效。

## 11. 文档同步（CLAUDE.md 强制）

- **`docs/configuration.md`**：编辑能力说明（双击/按钮进入，Cmd+Enter/按钮/失焦退出，硬暂停语义）。
- **`docs/architecture.md`**：`Transcript` 三文本分层模型（edited ≻ polished ≻ raw）+ 编辑态 + DB `edited_text` 列。
- 本 spec + 对应 plan（`docs/superpowers/plans/2026-06-18-editable-result-window.md`）。
