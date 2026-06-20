# 归档设计文档（2026-06-18 ~ 2026-06-19）

> **归档说明**（2026-06-19）：以下 7 个 spec 对应功能均已实现并合并 main，各自文档原样合并归档于此，原独立文件已删除。每个章节以 `📄 <原文件名>` 标注来源。
> **交叉引用**：正文内 `[xxx.md](./xxx.md)` 链接为合并前原文件名，现指向本归档文件内同名章节；对应 plans 见 `docs/superpowers/plans/2026-06-19-archived-plans.md`。

---


## 📄 `2026-06-18-config-db-migration-design.md`

# 配置 DB 迁移设计

## 背景

config.yaml 与 octopus.db 两套存储系统并存，yaml 需独立维护序列化/字段迁移逻辑。每次字段重命名都需在 `load_config()` 中添加 yaml Value 层手动迁移代码（serde alias 在两键共存时 panic），维护成本高。

## 方案

将 config.yaml 全部 21 个字段迁移到 SQLite `app_config` 表（key-value TEXT 存储），与模型配置/识别历史共用同一 `octopus.db`。

### 表结构

```sql
CREATE TABLE IF NOT EXISTS app_config (
    category     TEXT NOT NULL DEFAULT 'default',
    config_key   TEXT PRIMARY KEY,
    config_value TEXT NOT NULL,
    description  TEXT
);
```

值统一 TEXT 存储，由 `load_app_config()` 按字段类型解析（bool.parse()、f64.parse()、u8 枚举映射）。`category` 列用于后续分组扩展（当前全部 `'default'`），`description` 由 seed 填充、写入时保留。

### 关键决策

1. **TEXT 统一存储**：bool/f64/u8 序列化为 TEXT，load 时按字段类型 parse。避免 BLOB/多类型列的复杂度。
2. **seed 幂等**：`INSERT OR IGNORE`，已有配置不被覆盖。db.sql 21 行 seed 保证首次启动即有默认值。
3. **yaml 一次性迁移**：`init_schema` 在 v0/v1→v2 升级时检测旧 config.yaml → serde 解析（含字段名迁移 shortcut→asr_shortcut 等）→ `save_app_config_at` 覆盖 seed → 重命名 config.yaml 为 config.yaml.bak。
4. **v1→v2 迁移策略**：INIT_SQL 全部是 `CREATE TABLE IF NOT EXISTS` + `INSERT OR IGNORE`，幂等安全重跑。v1 数据库直接重跑 INIT_SQL 即可补建 app_config 表 + seed，无需单独迁移 SQL。
5. **写策略（ON CONFLICT DO UPDATE）**：所有写入用 `INSERT ... ON CONFLICT(config_key) DO UPDATE SET config_value = excluded.config_value`，**仅更新 config_value**，保留 description + category。不用 `INSERT OR REPLACE`（它会 DELETE+INSERT 整行，清空非指定列）。
6. **单键写 vs 全量写**：`persist_*`（工具栏切换）用 `save_config_key`（单键 ON CONFLICT，避免全量回写）；`set_config`（设置窗口表单）用 `save_app_config`（全量 21 字段 ON CONFLICT）。
7. **category 列**：预留分组能力，当前全部 `'default'`。v2→v3 迁移用 `ALTER TABLE ADD COLUMN`（DEFAULT 自动填入存量行）。
6. **AppConfig struct 保持不变**：仍然是 serde Serialize/Deserialize，用于：a) 前端 JSON 序列化（get_config 命令）；b) yaml 迁移路径中的 `serde_yaml::from_value` 解析旧 config.yaml。

### 排除方案

- **serde alias 迁移字段名**：两键共存时 duplicate field panic，改为 yaml Value 层手动迁移（在 `migrate_yaml_to_db` 中一次性完成）。
- **INSERT OR REPLACE**：会 DELETE+INSERT 整行，导致 description / category 被清空。改为 `ON CONFLICT DO UPDATE SET config_value`。
- **多类型列（TEXT/INTEGER/REAL 分列）**：增加 schema 复杂度，parse 开销可忽略。

## 涉及文件

| 文件 | 变更 |
|------|------|
| `crates/infra/src/db.sql` | 新增 `app_config` 表 + 21 行 seed |
| `crates/infra/src/db.rs` | 新增 `load_app_config` / `save_app_config` / `save_config_key` / `migrate_yaml_to_db`；更新 `init_schema`（v0→v2, v1→v2） |
| `crates/infra/src/config.rs` | `load_config()` 改为薄包装 `db::load_app_config()`；移除 yaml 解析 + `migrate_key` |
| `crates/desktop/src/runtime_config.rs` | `persist_*` 改用 `db::save_config_key`；移除 `write_config_yaml` |
| `crates/desktop/src/settings_commands.rs` | `set_config` 改用 `db::save_app_config`；移除本地 `write_config_yaml` |

## 迁移流程

```
首次启动 (v0/v1 → v3):
  init_schema()
    → execute_batch(INIT_SQL)        // 幂等：建表 + seed（含 app_config + category 列）
    → migrate_yaml_to_db(conn)       // 检测 config.yaml
        → 存在？ → serde_yaml 解析 + 字段名迁移
                 → save_app_config_at() ON CONFLICT 覆盖 seed value
                 → rename config.yaml → config.yaml.bak
        → 不存在？ → 直接返回
    → PRAGMA user_version = 3

v2 升级 (v2 → v3):
  init_schema()
    → ALTER TABLE app_config ADD COLUMN category TEXT NOT NULL DEFAULT 'default'
    → PRAGMA user_version = 3

后续启动 (v3+):
  init_schema() → v >= 3 → 跳过
```


## 📄 `2026-06-18-dashscope-streaming-design.md`

# DashScope 云端流式 ASR（VAD-gated per-utterance streaming）

> Date: 2026-06-18（2026-06-19 更新：Qwen-ASR Realtime 协议 + 非阻塞 finish + partial 分离；2026-06-20 更新：审查三1 Toggle 停止改非阻塞 close_async + CloudClosing/session_id 护栏；d9a303d 补 DB INSERT 兜底）
> 状态：✅ 已实现 — 3 套云端协议 + 流式 bug 修复 + Toggle 非阻塞收尾 + DB INSERT 兜底完成（详见 §2.2/§3）

## 1. 背景与目标

DashScope 云端实时 ASR 支持 **3 套接口**（共用 DashScope API Key），通过 endpoint 路径自动分发：

| 接口 | endpoint | 协议 | DB model_name |
|---|---|---|---|
| Fun-ASR | `/api-ws/v1/inference` | 任务型（run-task/finish-task） | `fun-asr-realtime` |
| Paraformer | `/api-ws/v1/inference` | 任务型（与 Fun-ASR 共用） | `paraformer-realtime-v2` |
| Qwen-ASR | `/api-ws/v1/realtime` | OpenAI Realtime 风格 | `qwen3-asr-flash-realtime` |

**目标**：VAD-gated per-utterance streaming——VAD 检测到语音 onset → 开一条长连接 WSS，持续推 PCM 收 partial；静音 ≥ `pause_polish_threshold_ms` → 发 finish 信号（**非阻塞**），后续 tick drain 最终结果。

## 2. 架构设计

### 2.1 Stage：`CloudStreaming`

```
audio.drain_samples → VAD 检测 → 语音？
  ├─ 否（静音中）→ 更新 pre_roll_buffer + silence_duration + speech_confirm_count=0
  │    └─ 有活跃 WSS + 静音 ≥ threshold → session.finish()（非阻塞）→ is_closing=true
  └─ 是（语音中）→ speech_confirm_count++ + silence_duration=0
       ├─ 无活跃 WSS + 连续 2 tick 确认 → open WSS + 推 pre-roll 100ms + push PCM
       └─ 有活跃 WSS（持续）→ push PCM + drain events:
            ├─ StreamEvent::Text(partial) → current_partial = partial（不碰 transcript）
            └─ StreamEvent::Finished → transcript.append_segment(current_partial) → drop session
```

**partial 与 transcript 分离**（关键设计决策）：
- `current_partial`：当前 session 的实时 partial 预览（UI 显示 transcript + partial）
- `transcript`：已提交的历史文本，只在 `Finished` 事件时 append
- 这解决了 partial 覆盖历史文本 + close 结果重复 append 的 bug

### 2.2 `DashScopeStreamSession`（`dashscope_stream.rs`）

有状态 WS 会话句柄，coordinator 通过同步接口操作：

| 方法 | 语义 | 协议 |
|---|---|---|
| `open()` | 建连 + 初始化（run-task / session.update）+ pre-roll | 自动分发 |
| `push_pcm(&[f32])` | 非阻塞推 PCM（二进制 / base64） | 自动分发 |
| `finish()` | **非阻塞**发 finish 信号（finish-task / session.finish） | 自动分发 |
| `try_recv_text()` | 非阻塞取 partial（`Option<StreamEvent>`） | — |
| `close_async(self)` | **非阻塞**发 finish + 8s 超时 recv loop 拿最终结果（async，消费 `self`）。Toggle 停止路径 spawn 之，结果经 `Command::CloudStreamingDone { text, session_id }` 回传 + 进 `Stage::CloudClosing` 承载等待态（审查三1）。旧同步 `close()`（`block_on`）已删 | 自动分发 |

**非阻塞 finish / close**（关键修复，审查三1）：tick handler 用 `finish()`（停顿收尾，结果由后续 tick `try_recv_text()` 异步取）；Toggle 停止用 `close_async`（spawn，结果经 `CloudStreamingDone` 回传）。两者都不在 coordinator 同步线程 `block_on`——曾导致 UI 冻结 20 秒。旧同步 `close()` 的阻塞实现已删（无调用方，消除 footgun）。

### 2.3 三套协议自动分发

`is_qwen_realtime_endpoint(endpoint)` 按 URL 路径分流：
- 含 `/v1/realtime` → Qwen-ASR Realtime 协议
- 否则 → Fun-ASR/Paraformer 任务型协议

**Fun-ASR / Paraformer**（`run_ws_session`）：
- 二进制 PCM 帧（s16le）
- run-task / finish-task / result-generated / task-finished
- **句边界检测用 `sentence_id` + `sentence_end`**（非靠 text 变空）：
  - `sentence_id` 变化 = 新句，提交前一句到 `committed`
  - `sentence_end=true` = 最终结果，立即提交
  - `heartbeat=true` 跳过心跳包

**Qwen-ASR Realtime**（`run_qwen_realtime_session`）：
- base64 PCM via `input_audio_buffer.append`（文本帧）
- session.update（server_vad 模式，silence_duration_ms=600）
- partial = `conversation.item.input_audio_transcription.text`（text + stash 拼接）
- final = `conversation.item.input_audio_transcription.completed`（transcript 字段）
- 结束 = `session.finish` → `session.finished`

### 2.4 onset 抗噪

连续 2 个 tick（~200ms）检测到语音才开 WSS（`speech_confirm_count >= 2`），消除单次噪声脉冲导致的空 session 误触发。

## 3. Toggle 停止时的收尾

Toggle（停止录音）从 `CloudStreaming` 收尾（审查三1 起非阻塞，详见 desktop-audit spec §3.6/§4）：
1. 停 tick 线程 + `audio.stop()` 排空剩余音频
2. 有活跃 WSS：spawn `close_async`（**非阻塞**）+ 进 `Stage::CloudClosing { transcript, current_partial }`（持有收尾态）+ `return`——不阻塞 coordinator 处理其他命令；无活跃 WSS：直接 `finalize_cloud`
3. close_async 完成 → `Command::CloudStreamingDone { text, session_id }` 回传 → `handle_cloud_streaming_done` 校验 `CloudClosing.transcript.id == session_id`（跨会话护栏，见 desktop-audit §3.1 / followups §4）→ `set_full` + `finalize_cloud`
4. `finalize_cloud`：append `current_partial`（未提交的 partial）→ 空文本→Idle / 否则 `start_final_polish_or_paste`

**CloudClosing 期间语义**：Toggle 忽略（close 完成自动 finalize+粘贴）；Cancel→Idle（不粘贴不写库）；Discard→写库保历史不粘贴。三条均正确。

### 3.1 DB INSERT 时机与兜底（d9a303d）

CloudStreaming 只在 `StreamEvent::Finished` 时 `update_transcription_raw`（INSERT/UPDATE raw_text）——与本地 Streaming 路径每次 `accept_samples` 都 INSERT 不同。若整个录音从未触发 Finished（用户没停顿够就 Toggle stop / 点立即润色），记录从未创建 → 后续 `Finalize` / `UpdatePolished`（均 `UPDATE WHERE id=?`）静默 0 行，**数据丢失**。

修复（d9a303d）：
- `finalize_cloud`：append partial 后、`start_final_polish_or_paste` 之前先调 `update_transcription_raw` 确保 INSERT
- `handle_polish_now`：`take_polish_input` 之前也调 `update_transcription_raw`（本地路径已 INSERT 为 no-op，CloudStreaming 路径补 INSERT）


## 📄 `2026-06-18-editable-result-window-design.md`

# 结果展示区可编辑设计

> Date: 2026-06-18
> Status: ✅ 已实现（2026-06-18，plan 2026-06-18-editable-result-window.md v2）。会话中编辑（快捷键 edit_shortcut/按钮进入，快捷键/按钮退出，硬暂停）+ 三文本分层 + 编辑×润色折回 + DB edited_text 均已落地。

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

### 3.1 进入编辑（择一触发）

1. **快捷键** `edit_shortcut`（config.yaml 可配，默认 `Cmd+E`；窗口内、仅结果窗聚焦时生效）：→ 设 `contenteditable=true` + 聚焦 + `invoke('enter_edit_mode')`。
2. **工具栏 ✏️ 编辑按钮**：点击同上。

> 不用双击——WKWebView 在 `user-select:text` 区域 `dblclick` 难触发（浏览器走选词手势，dblclick 判定常不成立），实测"难触发"后弃用。仅当处于活跃会话（Streaming / VadSegmented）且已有文本时按钮可点。

### 3.2 编辑态指示

- 编辑态：`#text-wrapper` 加**淡蓝边框**（`border: 1px solid rgba(0,122,255,0.5)`）+ 淡蓝底，包住整个展示区；`#result-text` 仅保留右侧内边距（避让完成按钮），不再自带边框；「完成编辑」按钮落在框内（GUI e2e 反馈：饱和蓝 `#007aff` 画在单行 `#result-text` 上太窄、颜色偏深，改为淡蓝 + 上移到 `#text-wrapper`）。
- 工具栏高亮「✏️ 编辑中」（编辑按钮 `.active`）+ 显示「**完成编辑**」按钮。
- 编辑期间：禁用 `container.mouseleave` 自动收起；窗口保持可见并 `setFocus()`（保证键盘输入进入 webview）。

### 3.3 退出编辑（择一触发）

1. **快捷键** `CmdOrCtrl+Enter`。
2. 点「**完成编辑**」按钮。

> e2e 反馈：完成按钮已足够显眼，失焦 / 点 toolbar 自动退出非必要，已去除 `blur` 触发——点别处或点工具栏不再自动提交，改完请显式 Cmd+Enter 或点完成按钮。后端 Toggle/Cancel 停止路径仍用 `edit_buffer` 兜底提交（§7），不会丢编辑。

> **快捷键演进（2026-06-19）**：退出编辑已从「固定 `CmdOrCtrl+Enter`」统一为 `edit_shortcut` **toggle**——进入与保存（退出）用同一个键（默认 Cmd+Enter，可配）。本节下方 `CmdOrCtrl+Enter` / 完成按钮为原始设计记录（完成按钮亦已于布局调整删除，见 §9 L186 演进注）。当前权威：`docs/configuration.md` 的 `edit_shortcut`。

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

> **raw_len 推进时机**（flicker 修复）：`take_polish_input` 记录快照边界 `polish_snapshot_len` 但**不立即推进 `raw_len`**——避免润色 pending 期间 `display_text()` 因 `raw_len` 已推进而丢失 increase（展示区文字变少的 flicker）。`raw_len` 推进延迟到 `on_polish_done`（润色完成时 `raw_len = polish_snapshot_len`）。

| 派生量 | 定义 |
|---|---|
| `display_text()` | committed 前缀 + increase |
| polish 输入（停顿快照） | `take_polish_input()`：has_edit → `(Some(edited), increase)`；否则 `(None, full)`（保持现状）。详见 §12 |
| `db_text()`（raw_text 落库） | `full`（原始 ASR，编辑/润色均不改） |
| `edited_text()`（新） | `edited`（空则 None） |

> 三文本**互不干扰**：`raw_text = full` 永远是原始 ASR；`polished_text` 仅润色成功填；`edited_text` 仅用户提交编辑填。三者独立字段、独立落库列。

## 5. 编辑提交语义（关键不变量）

用户在编辑态改的是 **display 全文**（= 进入编辑瞬间的 `display_text()`）。提交（commit_edit）时后端执行：

1. `edited = 用户文本`
2. `raw_len = full.chars().count()`（increase 清空）
3. `full`（raw ASR）**原样保留**

此后新 ASR 追加到 `full`，`increase = full[raw_len..]` = 新内容 → `display = edited + 新增`（满足"展示 = edited + 新识别"）。下次停顿润色经 `take_polish_input()` 取 `(Some(edited), increase)`，LLM 仅润色 increase、保留 edited，结果折回 `edited`（满足"润色输入 = edited + 新识别"，详见 §12）。

**多次编辑**：每次 commit 覆盖 `edited`、把 `raw_len` 推进到当时 `full` 末尾。空文本提交允许（`edited = ""` → committed 回退到 polished/raw）。

**编辑后的润色（§12 核心）**：`on_polish_done` 在 `has_edit()` 时把润色结果**折回 `edited`**（`edited = result`），而非写 `polished`——否则 `edited ≻ polished` 会永久遮蔽 polished、丢失被润色吞掉的新增文本。折回后 `display = edited（= edited + 润色后新增）+ increase`，无丢字。

## 6. 数据流

```
活跃会话（Streaming/VadSegmented）：
  update-result(display) 持续刷新 ── 用户按 edit_shortcut(Cmd+E) / 点编辑按钮 ──▶ enter_edit_mode
                                                          │
                                              ① contenteditable=true + 聚焦 + setFocus
                                              ② coordinator: 硬暂停（streaming_active=false / 停 tick）
                                              ③ 冻结 display（不再 emit update-result）
                                                          │
                                            用户编辑 #result-text ◀── 键盘输入
                                                          │
  Cmd+Enter / 完成按钮 ──▶ commit_edit(innerText)
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
| 编辑态点工具栏其他按钮（如切换 ASR） | 不退出编辑，直接执行按钮动作（e2e：完成按钮已足够显眼，点 toolbar 退出非必要） |
| 空文本提交 | `edited=""`，committed 回退 polished/raw |
| 完成按钮 click 可靠性 | 提交统一走同一 `commitEdit`；完成按钮用 `mousedown preventDefault` 避免抢焦导致 click 丢失（无 blur 触发，故无失焦竞态） |
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
  - `edited_text() -> Option<&str>` / `has_edit() -> bool`。
  - `take_polish_input(&mut self) -> (Option<String>, String)` 替代 `snapshot_for_polish`（§12）。
  - `on_polish_done`：`has_edit` 时折回 `edited = result`，否则 `polished = result`（§12）；推进 `raw_len = polish_snapshot_len`（take_polish_input 时记录的快照边界）。
  - `edited_display() -> Option<String>`：edited 非空返回 display（停止粘贴/兜底用）。
  - 现有 `snapshot_for_polish` 测试迁移到 `take_polish_input`；新增 commit_edit / 折回 / display 优先级测试。
- **`llm/src/prompt.rs` + `llm/src/client.rs`**（§12）：
  - `user_prompt(preserved: Option<&str>, to_polish: &str)`：有 preserved 时分块提示（已确认原样保留 + 新增润色）。
  - `polish(preserved: Option<&str>, to_polish: &str, config)`：签名加 preserved。
  - system prompt 加一条「已确认部分不得修改」。
- **`desktop/src/coordinator.rs`**：
  - 新增 `Command::EnterEditMode` / `UpdateEditBuffer` / `CommitEdit { text }`；`DbCommand::UpdateEdited`；3 个 Tauri 命令 + `invoke_handler` 注册。
  - 编辑态表示：主循环局部 `editing: bool` + `edit_buffer: Option<String>`。编辑期间 Streaming/VadSegmented 的 tick 跳过喂引擎、只 `audio.drain_samples()` 丢弃（硬暂停）。
  - `handle_toggle` 停止路径：编辑态先用 `edit_buffer` commit 再停止（§7）。
  - `update_result` 发送处：编辑态跳过（冻结）。
  - **润色接线（§12）**：`check_and_trigger_polish` / `handle_polish_now` / 最终润色入口改用 `transcript.take_polish_input()` 取 `(preserved, to_polish)` 喂 `polish(preserved, to_polish, ..)`；`spawn_polish_thread` 签名加 `preserved`。`handle_polish_done`：`has_edit` 时折回（`on_polish_done` 已折）+ 走 `DbCommand::UpdateEdited`，否则 `UpdatePolished`。停止路径 3 处无润色/兜底粘贴用 `edited_display()`。
- **`desktop/src/result_window.rs`**：
  - 新增 `#[tauri::command] enter_edit_mode` / `commit_edit(text)`。
  - 编辑态冻结 `update-result`（前端配合）。
- **`desktop/dist/result/index.html`**：
  - `#result-text` 动态 `contenteditable` 切换；`edit_shortcut`（默认 Cmd+E）+ ✏️ 编辑按钮（编辑态 toggle 为保存，图标 ✏️→💾）。
  - `edit_shortcut` toggle（再按一次）/ ✏️(💾) → `commit_edit`。
  - > **布局演进（2026-06-19）**：原「完成编辑」按钮（浮文本区右上）已删除，保存入口迁 ✏️ toggle；编辑态文字不再水平重排（移除 `padding-right:90px`）、编辑态 toolbar 强制常驻。详见 [`edit-layout spec`](2026-06-19-result-window-edit-layout-design.md)。本节下方如仍提「完成编辑按钮」为历史记录。
  - 编辑态：加边框、禁 `mouseleave` 收起、`setFocus`、忽略 `update-result`。
- **`desktop/src/main.rs`**：`invoke_handler` 注册 `enter_edit_mode` / `commit_edit`。
- **`infra/src/db.sql`** + **`infra/src/db.rs`**：`edited_text` 列 + `finalize_transcription` 参数；`TranscriptionRecord` 加字段；历史查询 SELECT 加列。
- **图标**：`dist/result/icons/` 加 `edit.svg`。

## 10. 测试策略

- **`transcript.rs` 单测**（核心）：
  - `commit_edit` 后：`edited` = 文本、`raw_len` = `full.len()`、`full` 不变、`increase` 清空。
  - `display_text()` 优先级：edited ≻ polished ≻ raw。
  - 编辑后续追加 ASR：`display = edited + 新 increase`。
  - `take_polish_input`：has_edit → `(Some(edited), increase)`；无编辑 → `(None, full)`。
  - `on_polish_done` 折回：has_edit 时 `edited = result`、display 不丢字；无编辑时 `polished = result`（现状）。
  - 空提交回退；多次编辑覆盖。
  - 现有 `snapshot_for_polish` 测试迁移到 `take_polish_input`。
- **llm prompt**：`user_prompt(None, text)` 无 preserved（现状）；`user_prompt(Some(p), new)` 含分块标记 + 保留指令。
- **coordinator**：编辑态停止音频喂入（streaming_active / tick 行为）；编辑态 toggle 先 commit 再停；`handle_polish_done` 折回时走 `UpdateEdited`。
- **手动 e2e**：
  - 录音中按 Cmd+E 编辑改错别字 → 完成 → 继续说 → 新文本追加在编辑结果后 → 停止 → 粘贴含修正。
  - 编辑态点工具栏切换 ASR → 先提交编辑再切换。
  - 三种 PolishMode 下编辑均生效。

## 11. 文档同步（CLAUDE.md 强制）

- **`docs/configuration.md`**：编辑能力说明（`edit_shortcut` toggle 进入/保存同键、硬暂停语义）+ `edit_shortcut` 字段。
- **`docs/architecture.md`**：`Transcript` 三文本分层模型（edited ≻ polished ≻ raw）+ 编辑态 + DB `edited_text` 列。
- 本 spec + 对应 plan（`docs/superpowers/plans/2026-06-18-editable-result-window.md`）。

## 12. 编辑×润色交互（折回 + 边界提示词）

> 补充于设计复核。解决「编辑后触发润色时新增文本/润色结果被 `edited ≻ polished` 永久遮蔽而丢失」的缺陷。

### 12.1 问题

编辑提交后 `edited` 非空，继续说话触发中间润色（mode=2）：`snapshot` 把 `raw_len` 推到 `full` 末尾（increase 清空）→ `on_polish_done` 写 `polished` → 但 `display = edited ≻ polished`，polished 被永久遮蔽 → **新增文本从 display 丢失**。最终润色（停止时）虽在 `Stage::Polishing` 丢 transcript，但输入已 = display（edited+increase）、粘贴 = LLM 结果，故编辑被尊重、**最终润色无需折回**；折回仅针对中间润色。

### 12.2 目标行为（用户确认）

- 润色输入 = `(edited, 新增 increase)`，**期望 LLM 只润色新增、保留 edited 原样**（需提示词告知边界）。
- 润色结果 = `edited + 追加润色部分`，**折回 `edited`**：`on_polish_done` 在 `has_edit()` 时 `edited = result`，否则 `polished = result`（保持现状）。
- 折回后 `display = edited + increase`，无丢字；LLM 若未严格遵守（动了 edited），也接受。

### 12.3 实现

**Transcript**：
- `take_polish_input(&mut self) -> (Option<String>, String)`：`has_edit` → `(Some(edited.clone()), increase)`；否则 `(None, full)`。副作用推进 `raw_len`（清 increase）。替代 `snapshot_for_polish`。
- `on_polish_done`：`has_edit` → `edited = result`（折回）；否则 `polished = result`。

**llm crate**：
- `user_prompt(preserved: Option<&str>, to_polish: &str)`：有 `preserved` 时构造分块提示（「已确认部分原样保留」+「新增部分请润色」+ 输出拼接完整文本）。
- `polish(preserved, to_polish, config)`：签名加 `preserved`，传入 `user_prompt`。
- system prompt 加规则：「若提示含【已确认部分】，该部分必须原样保留，仅润色其余部分」。

**coordinator**：
- `spawn_polish_thread(preserved: Option<String>, to_polish: String, ..)` → `polish(preserved.as_deref(), &to_polish, ..)`。
- `check_and_trigger_polish` / `handle_polish_now`：`let (preserved, to_polish) = transcript.take_polish_input();`。
- 最终润色入口 `start_final_polish_or_paste`：polish 分支用 `transcript.take_polish_input()`（持有 owned transcript），无润色分支仍用调用方传入的 `text`（= `edited_display()`）。
- `handle_polish_done`：折回时 DB 走 `UpdateEdited`（保持 `edited_text` 与 display 一致），否则 `UpdatePolished`。
- 最终润色失败兜底：`Stage::Polishing` 加 `fallback_text`（= 停止时 display），`handle_final_polish_done` 的 Err 分支 `do_paste(&fallback_text)` 而非 raw ASR，保留编辑（DB raw 仍 raw_text）。

### 12.4 DB 一致性说明

- `edited_text` 写入时机：commit_edit（用户编辑）+ 中间润色折回（= 润色结果）。最终润色不触碰 `edited_text`（保持最后一次用户/折回值）。
- 历史展示优先级 `edited_text ?? polished_text ?? raw_text`。最终润色+编辑后，`edited_text`（用户/折回值）与粘贴文本（最终润色结果）可能略有差异——已知次要不一致，不在本次修复范围。


## 📄 `2026-06-19-connection-test-async-design.md`

# 连接测试命令 async 重构设计

> Date: 2026-06-19
> 状态：已实现（commits `b2b67b3` + `6bd791a`，merge main；GUI 已验证）
> 关联：[2026-06-19-connection-test-design.md](./2026-06-19-connection-test-design.md)（连接测试功能本身，已实现）

## 1. 背景

连接测试命令（`test_llm_connection` / `test_asr_connection`，由 settings-ui 分支引入）当前实现为同步 `#[tauri::command] fn`：

- `test_llm_connection`（`settings_commands.rs:263`）：`std::thread::spawn(move || test_connection(&llm_cfg))` + `handle.join()`。注释称「避免 Tauri 命令超时」，但 `join()` 仍阻塞命令线程直至阻塞请求返回——spawn 只是把阻塞挪到子线程再等回，徒增线程创建/切换开销，命令线程仍被占住。
- `test_asr_connection`（`settings_commands.rs:283`）：`std::thread::spawn` + `tokio::runtime::Runtime::new()` + `block_on`。每次测试新建一个独立 tokio runtime，与 Tauri 内置 `tauri::async_runtime` 并存，开销且语义混乱（nested runtime 隐患）。

Tauri 2 的命令原生支持 `async fn`，且 `tauri::async_runtime` 已是项目在用的 tokio runtime（`coordinator.rs:905` 直接 `tauri::async_runtime::handle()`）。改 async 后命令跑在该 runtime 上，无需手动 spawn / new-runtime。

## 2. 目标

- 两个命令改 `async fn`，跑在 `tauri::async_runtime` 上。
- 删除 `std::thread::spawn + join`（LLM）与 `Runtime::new() + block_on`（ASR）。
- 前端 `invoke` 契约不变（命令名、入参、`Result<String, String>` 返回、错误文案）。

## 3. 非目标

- 不改连接测试业务逻辑（请求内容、超时阈值、错误文案）。
- 不改错误返回类型（保持 `Result<String, String>`，前端依赖字符串文案 showToast）。
- 不改 `main.rs` 的 `generate_handler!` 注册（async command 注册方式与 sync 相同，Tauri 自动适配）。
- 不改前端 `index.html`。

## 4. 方案

### 4.1 test_llm_connection

```rust
#[tauri::command]
pub async fn test_llm_connection(spec: String) -> Result<String, String> {
    if spec.is_empty() {
        return Err("未选择润色模型".into());
    }
    let llm_cfg = octopus_infra::db::load_llm_model(&spec)
        .map_err(|e| format!("从 DB 加载 LLM 配置失败: {}", e))?
        .ok_or_else(|| format!("DB 中未找到 LLM 模型 '{}'", spec))?;

    // reqwest::blocking 客户端跑在 spawn_blocking 线程池，不占用 async runtime worker。
    // test_connection 返回 Result<(), anyhow::Error>：闭包内先 map_err 转 String，
    // 使 spawn_blocking 返回 JoinHandle<Result<(), String>>，.await 后链式匹配 Result<String, String>。
    tauri::async_runtime::spawn_blocking(move || {
        octopus_llm::test_connection(&llm_cfg).map_err(|e| format!("{}", e))
    })
        .await
        .map_err(|_| "测试线程异常终止".to_string())?  // JoinError
        .map(|_| "连接成功".to_string())                  // test_connection: Result<()>
}
```

说明：`spawn_blocking` 返回 `JoinHandle<Result<()>>`，`.await` 得 `Result<Result<()>, JoinError>`：外层 `map_err` 处理线程 panic/取消，内层 `map` 处理 `test_connection` 成功。

### 4.2 test_asr_connection

前置校验（`is_local`、`entry`、`secret_key` 空）逻辑不变。WS 测试段改为直接 await：

```rust
#[tauri::command]
pub async fn test_asr_connection(bare_name: String) -> Result<String, String> {
    // ... 前置校验同现状（list_engines / is_local / entry / secret_key 空）...
    #[cfg(feature = "dashscope")]
    {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let mut req = endpoint.into_client_request()
            .map_err(|e| format!("WS 端点无效: {}", e))?;
        req.headers_mut().insert(
            "Authorization",
            format!("bearer {}", key).parse().unwrap(),
        );
        // 直接在 tauri::async_runtime 上 await，删除 Runtime::new + block_on
        match tokio::time::timeout(
            std::time::Duration::from_secs(3),
            tokio_tungstenite::connect_async(req),
        ).await {
            Ok(Ok(_)) => Ok("连接成功".into()),
            Ok(Err(e)) => Err(format!("WS 连接失败: {}", e)),
            Err(_) => Err("WS 连接超时（3s）".into()),
        }
    }
    #[cfg(not(feature = "dashscope"))]
    { Err("远程 ASR 连接测试需要 dashscope feature".into()) }
}
```

说明：async command 在 `tauri::async_runtime`（tokio）上下文执行，`connect_async` 对 tokio runtime 的要求天然满足，删除 `Runtime::new` 后无 nested runtime 问题。

### 4.3 契约不变性

前端 `crates/desktop/dist/settings/index.html` 的 `testLlmConnection` / `testAsrConnection` 调 `invoke('test_llm_connection', { spec })` / `invoke('test_asr_connection', { bareName })`，返回 `Promise<Result<string,string>>`。Tauri 对 sync / async command 在前端侧表现完全一致（自动 wrap 为 Promise）。无需改前端。

## 5. 文件清单

| 文件 | 改动 |
|---|---|
| `crates/desktop/src/settings_commands.rs` | 2 个 fn 改 `async fn`；LLM 用 `spawn_blocking`，ASR 删 `Runtime::new` 直接 await；纯逻辑单测保持 |
| `crates/desktop/src/main.rs` | 不动（`generate_handler!` 注册不变） |
| 前端 `index.html` | 不动 |

## 6. 风险

- **低**。`reqwest::blocking` 跑在 `spawn_blocking` 线程池，不污染 async runtime；`connect_async` 在 tauri runtime 上，删 nested runtime 反而更安全。
- 单测覆盖纯逻辑（spec 解析、`is_local` 判定、`secret_key` 空检查）；WS 连通不便单测，沿用现有手动验证（设置窗口点测试按钮）。

## 7. 验证（已通过）

- `cargo check --workspace --all-targets`：clean
- `cargo test -p octopus-desktop`：纯逻辑单测全过（spec 解析 / `is_local` 判定 / `secret_key` 空检查不受 async 改造影响）
- 手动 GUI：设置窗口点「测试连接」，LLM + ASR（aliyun 远程）成功/失败文案与重构前一致（用户本地确认 OK）
- 前端 `invoke` 契约未变（async command 自动 wrap Promise），前端零改动


## 📄 `2026-06-19-connection-test-design.md`

# 设置页连接测试按钮设计

> Date: 2026-06-19
> 状态：已实现（commits `819777d` + `3f96a31` + `e2cd7a8`）
> ⚠️ 命令实现为 `async fn`——见 [2026-06-19-connection-test-async-design.md](./2026-06-19-connection-test-async-design.md)（`spawn_blocking` / 直接 `await connect_async`）。本文档保留功能设计、接口契约、前端 UI 与关键决策；命令实现细节（删 `thread::spawn` + `Runtime::new`）见 async 文档 §4。

## 1. 背景

设置页「语音识别引擎」和「文本润色模型」两个 select 切到远程模型后，用户没有直观方式确认配置（endpoint + API Key）是否有效——只能开始录音、等报错才知道。新增两个连接测试按钮，让用户在保存配置前先验证连通性。

## 2. 目标

- ASR 引擎 select 右侧加一个测试按钮：
  - **本地模型** → 灰掉（`disabled`，`pointer-events:none`，title「本地模型无需连接测试」）
  - **远程模型**（provider=aliyun）→ 可点，3s WS 握手连通性检测
  - select 切换时按 `is_local` 动态刷新按钮 disabled 状态
- 润色模型 select 右侧加一个测试按钮：始终可点，发一个 `max_tokens=1` 的极简 chat 请求（10s 超时）
- 三态视觉反馈：默认（灰边框）/ 成功（绿 #22c55e）/ 失败（红 #ef4444）；点击中 `loading`（半透明 + 禁用）
- 不消耗大量 API 额度（LLM 仅 1 token；ASR 仅握手不发协议帧）

## 3. 接口

### 3.1 新增 Tauri 命令

文件：`crates/desktop/src/settings_commands.rs`

```rust
// 当前为 async fn（详见 connection-test-async-design §4）；签名/入参/返回/语义如下：
#[tauri::command]
pub async fn test_llm_connection(spec: String) -> Result<String, String>;
//   入参：spec = polish_llm 配置值（3-part spec 或裸名）
//   返回：Ok("连接成功") / Err("<错误信息>")
//   语义：load_llm_model(spec) → octopus_llm::test_connection（spawn_blocking 包阻塞请求）

#[tauri::command]
pub async fn test_asr_connection(bare_name: String) -> Result<String, String>;
//   入参：bare_name = ASR 引擎裸名（前端 select 的 value）
//   返回：Ok("连接成功") / Err("<错误信息>")
//   语义：list_engines().find(name) → is_local 则 Err("本地模型无需连接测试")
//         否则取 DB endpoint+key → tokio::time::timeout(3s, connect_async(req))
```

注册：`crates/desktop/src/main.rs::run` 的 `invoke_handler` 列表追加两个命令。

### 3.2 LLM 测试实现

文件：`crates/llm/src/client.rs`

```rust
pub fn test_connection(config: &CompatibleLlmConfig) -> Result<()>;
```

- 复用 `ChatRequest` 结构，messages=[{"user","Hi"}]，`max_tokens=1`，`temperature=0.0`
- 按 `needs_disable_thinking()` 走与 `polish` 一致的 thinking 关闭逻辑（DeepSeek 用 `thinking.kind="disabled"`，其他用 `enable_thinking=false`）
- `reqwest::blocking::Client`，10s 超时
- 失败路径：网络/构建错误 → `anyhow::context`；非 2xx → `bail!("LLM API 返回错误 {}: {}", status, body)`

`crates/llm/src/lib.rs` 导出：`pub use client::{polish, test_connection};`

### 3.3 ASR 测试实现

内联在 `test_asr_connection` 内（当前 async 直接 `await connect_async`，删 `Runtime::new`，详见 connection-test-async-design §4.2）：

- `parse_model_spec(&bare_name).model_name()` 取裸名 → 查 `cfg.asr.aliyun[model_name]`
- `secret_key` 空 → Err 提示
- `#[cfg(feature = "dashscope")]` 分支：req 经 `IntoClientRequest` + 追加 `Authorization: bearer <key>` header → `tokio::time::timeout(3s, connect_async(req))`
- `#[cfg(not(feature = "dashscope"))]` → Err「远程 ASR 连接测试需要 dashscope feature」

**关键：仅验证 WS 握手成功，不发任何协议帧（run-task / session.update 都不发）**——避免消耗 DashScope 推理额度。握手成功即代表 endpoint + key 有效。

## 4. 前端 UI

文件：`crates/desktop/dist/settings/index.html`

### 4.1 DOM 结构

每个 select 包一层 `.select-with-test` flex 容器，select + 32×32 `.test-btn`：

```html
<div class="select-with-test">
  <select id="asr-engine-select" onchange="setVal('asr_engine', this.value); updateAsrTestBtn(this.value)">
    ...options...
  </select>
  <button class="test-btn disabled" id="asr-test-btn" onclick="testAsrConnection()" title="本地模型无需连接测试">
    <svg>...check.svg path...</svg>
  </button>
</div>
```

### 4.2 CSS（`.test-btn`）

- 默认：白底 + `var(--border)` 1px 边框 + 6px 圆角 + hover 边框/图标变 primary
- 三态：`.ok`（绿边框 + 绿图标）/ `.fail`（红边框 + 红图标）/ `.loading`（半透明 + `pointer-events:none`）
- `.disabled`：`opacity:0.3` + `pointer-events:none`（ASR 本地模型用）

### 4.3 JS 逻辑

- **LLM 测试**（`testLlmConnection`）：取 polish-llm-select 裸名 → **先 `set_config('polish_llm', value)` 持久化**（确保后端从 DB 读到最新 spec）→ `invoke('test_llm_connection', {spec: bareName})` → 切 ok/fail + `showToast`
- **ASR 测试**（`testAsrConnection`）：取 asr-engine-select 裸名 → `disabled` class 直接 return → `invoke('test_asr_connection', {bareName})` → 切 ok/fail + `showToast`
- **按钮状态联动**（`updateAsrTestBtn(bareName)`）：从缓存的 `asrEnginesData`（`renderSettings` 时缓存 `resp.asr_engines`）查 `is_local` → 本地加 `disabled` + title「本地模型无需连接测试」；远程移除 `disabled` + title「测试连接」。同时清掉历史 ok/fail 残留态。

## 5. 关键决策

1. **不抽独立引擎类、不发协议帧**：ASR 测试仅握手（connect_async），不进 `run-task` / `session.update`。理由：握手成功 ⇔ endpoint+key 有效，足够回答「能不能用」的问题，不消耗推理额度。
2. **阻塞请求不占 async runtime**：LLM 用 `reqwest::blocking` 包 `tauri::async_runtime::spawn_blocking`，ASR WS 直接 `await connect_async`——v1 曾用独立线程 + 独立 tokio runtime（避免 Tauri 命令超时 + 隔离 runtime），已重构为 async（详见 connection-test-async-design §1）。
3. **LLM 测试前先 `set_config` 持久化**：`test_llm_connection` 后端按 spec 从 DB 加载配置，必须确保 DB 中 `polish_llm` 是用户刚选中的值（与 set_config 内部 `build_polish_llm_spec` 一致的裸名）。
4. **ASR 测试不持久化**：`test_asr_connection` 接收 `bare_name` 直接查 DB endpoint——若用户改了 select 但还没触发 `setVal`，会测旧值；但用户从 select 切换到点按钮中间一般有 setVal 触发，可接受。
5. **图标源**：`crates/desktop/dist/result/icons/check.svg`（FontAwesome check，640×640 viewBox）——前端内联 SVG path（避免运行时加载），独立 SVG 文件保留作资源备份。

## 6. 已知限制

- ASR 测试只验 WS 握手，不验协议帧正确性（如 model_name 拼写错只能等真录音报错）
- LLM 测试发的是真实 chat 请求（即使 `max_tokens=1`），极小额度消耗
- 测试期间用户可重复点击——靠 `loading` class 的 `pointer-events:none` 拦截，但若回调丢失（极罕见）按钮会卡 loading


## 📄 `2026-06-19-result-window-edit-layout-design.md`

# 结果窗编辑布局调整设计（编辑态文字不动 + 保存按钮移 toolbar）

> Date: 2026-06-19
> 状态：✅ 已实现（2026-06-19，commit `d4401cb`：✏️ toggle + 删 edit-done + 文字不重排 + 编辑态 toolbar 常驻；e2e 通过。快捷键后续统一为 `edit_shortcut` toggle，`370e21e`）。plan：[2026-06-19-result-window-edit-layout.md](./2026-06-19-result-window-edit-layout.md)

## 1. 背景

editable-result 功能已实现（`edit_shortcut` 进入编辑 + ✏️ 按钮进入；快捷键后已统一为 **toggle**——进入/保存同键，见 §4.5）。当前编辑态有两个布局体验问题：

1. **进入编辑时文字水平重排**：编辑态 CSS 给 `#result-text` 加 `padding-right: 90px`（给浮在文本区右上的「完成编辑」按钮让位），文字内容区从 520px 变 430px → 换行位置改变 → 视觉上文字"动了"。
2. **保存按钮位置**：「完成编辑」按钮浮在文本区右上，用户希望移到 toolbar（顶栏工具区）。

## 2. 目标

- 进入/退出编辑时，**文字水平位置不变**（不重排）——主要诉求。
- 「保存编辑」入口移到 toolbar。
- ✏️ 按钮复用 toggle（进入 ↔ 保存），编辑态图标切为 💾（`save.svg`）。
- 垂直跳动（Cmd+Enter 进入时 toolbar 出现）可接受（次要）。

## 3. 现状（关键事实）

文件：`crates/desktop/dist/result/index.html`（单文件前端，无构建）。

- 编辑态 CSS（L200-209）：`#container.editing #text-wrapper` 淡蓝边框；`#container.editing #result-text { padding: 1px 90px 7px 13px }`（**重排根因**）。
- `#edit-done` 按钮（L184-198 CSS + L241 DOM）：浮文本区右上，编辑态显示，点它 `commitEdit()`。
- ✏️ 按钮（`#tool-edit`，L234）：点 `enterEdit()`；编辑态加 `.active`。
- 窗口尺寸驱动 toolbar 显隐（L257-275）：`HIDDEN_H=100` / `TOOLBAR_H=132`；`showToolbar()` 加 `toolbar-visible` + `setSize(132)`；`hideToolbar()` 有 `editing` 拦截（编辑中不隐藏）。
- **文字区宽度恒 520px（`WIN_W`），不受 toolbar 显隐影响**——水平换行只由 `#result-text` 的 padding 决定。
- `enterEdit()`（L428）：contenteditable=true + `editing` class + 显示 edit-done + focus + 光标置末尾 + `invoke('enter_edit_mode')`。
- `commitEdit()`（L447）+ `edit_shortcut` toggle 再按一次（keydown L468-480）。
- 编辑 toggle 快捷键 `edit_shortcut`（默认 Cmd+Enter）：进入与保存（退出）都用此键。

## 4. 设计

### 4.1 删除文本区「完成编辑」按钮

- 删 DOM `<button id="edit-done">`（L241）。
- 删 CSS `#edit-done` 全部规则（L184-198）。
- 删 JS `btnEditDone` 引用与事件绑定（L426、L433、L454、L482-483、L497）。

### 4.2 移除编辑态 padding（文字不重排核心）

- 删 `#container.editing #result-text { padding: 1px 90px 7px 13px }`（L206-208）。
- 编辑态 `#result-text` 沿用非编辑态默认 padding → 宽度恒 520px → **水平不重排**。
- 保留 `#container.editing #text-wrapper` 淡蓝边框（编辑中视觉提示）。
- 保留 `#container.editing #result-text:focus { background: transparent }`。

### 4.3 ✏️ 按钮复用 toggle + 图标切换

- CSS 新增（编辑态 `#tool-edit` 图标换 `save.svg`）：
  ```css
  #container.editing #tool-edit .icon {
    -webkit-mask-image: url(icons/save.svg?v=1);
    mask-image: url(icons/save.svg?v=1);
  }
  ```
  非编辑态沿用 `edit.svg`（L103）。`save.svg` 已就位（`icons/save.svg`，Font Awesome 软盘，单色 mask 源）。
- JS `tool-edit` click 改 toggle 语义：
  ```js
  btnEdit.addEventListener('click', (e) => {
    e.preventDefault();
    editing ? commitEdit() : enterEdit();
  });
  ```
- `title`/`aria-label` 编辑态切为「保存编辑」（可访问性，可选）。
- 保留 `btnEdit.classList.add/remove('active')`（编辑态高亮提示）。

### 4.4 编辑态 toolbar 强制常驻（方案 X）

- `enterEdit()` 末尾调 `showToolbar()`：保证编辑态 toolbar 可见，保存按钮（💾）恒可见。
  - 点 ✏️ 进入：toolbar 已 visible（鼠标在按钮上），`showToolbar()` 内 `if (toolbarVisible) return` no-op → **无跳动**。
  - Cmd+Enter 进入：toolbar 可能 hidden → `showToolbar()` → 窗口 100→132、文字顶部 8→32px（下移 24px，**用户已确认可接受**）。
- `hideToolbar()` 已有 `editing` 拦截（L270），编辑中不隐藏。✓ 无需改。
- `commitEdit()` 后不主动 `hideToolbar()`：toolbar 保持 visible，下次 `mouseleave` 才隐藏（恢复正常 hover 行为，避免退出编辑立即跳变）。

### 4.5 不变项

- 进入方式：`edit_shortcut`（Cmd+Enter）+ ✏️ 点击。
- 保存（退出）：`edit_shortcut` toggle 再按一次（与进入同键）。
- 后端命令 `enter_edit_mode` / `commit_edit` / `update_edit_buffer` 不变。
- 编辑态硬暂停 ASR（coordinator `editing` 标志）不变。

## 5. 交互流

```
非编辑态（✏️ edit.svg）:
  点 ✏️ 或 Cmd+Enter → enterEdit():
    contenteditable=true, .editing class, focus 光标置末尾
    图标 edit.svg → save.svg, .active 高亮
    showToolbar()（若此前 hidden：窗口增高、文字下移 24px）
    invoke('enter_edit_mode')

编辑态（💾 save.svg）:
  点 💾 或再按 `edit_shortcut` → commitEdit():
    contenteditable=false, 移除 .editing class
    图标 save.svg → edit.svg, 移除 .active
    invoke('commit_edit', {text})
    （toolbar 保持 visible，mouseleave 后隐藏）
```

## 6. 边界

- **编辑中结果窗隐藏**：新录音触发 `edit-force-exit` 事件清理——需确保清理时同步恢复图标（save.svg → edit.svg）+ 移除 `.active`（当前 `edit-force-exit` 处理 L491-500 已移除 editing class/contenteditable，需补图标恢复）。
- **`edit_shortcut` 在编辑态**：keydown 统一 toggle（L468-470），编辑态再按一次触发保存。同键 toggle，不冲突。
- **save.svg 加载失败**：mask 源缺失 → 图标空白（按钮仍在、可点）。降级可接受。

## 7. 测试

手动 e2e（GUI，需本地 `cargo run`）：

1. 识别出文字 → ✏️ 进入编辑 → **文字水平位置不变（不重排）** ✓
2. 编辑态图标为 💾 → 点 💾 保存 → 退出，图标回 ✏️ ✓
3. `edit_shortcut` 进入 → 再按 `edit_shortcut` 保存（toggle）✓
4. Cmd+Enter 进入（toolbar 此前 hidden）→ toolbar 出现（窗口增高）→ 💾 可见可点 ✓
5. 编辑中 mouseleave 窗口 → toolbar 不隐藏（editing 拦截）✓
6. 编辑中触发新录音（结果窗 hide）→ `edit-force-exit` → 图标恢复 ✏️、退出编辑态 ✓

无单元测试（纯前端 HTML/CSS/JS 改动，后端命令不变）。

## 8. 影响范围

- 仅 `crates/desktop/dist/result/index.html`（前端单文件）+ 新增 `icons/save.svg`（已就位）。
- 不改 Rust（coordinator / runtime_config / commands 不变）。
- 不改 config（`edit_shortcut` 不变）。
- 不改 editable-result spec（本设计是其布局调整，机制不变）。


## 📄 `2026-06-19-vad-preheat-design.md`

# 启动/录音性能优化设计：VAD session 缓存（①），lock-free 音频（③可选）

> Date: 2026-06-19
> 状态：已实现（commits `c15c159` + `569f94b` + `07a1503`）
> 修订：v3。v2 主方案 `Arc<Session>`（论据「`Session::run(&self)`」）有误——ort 源码 `session/mod.rs:212` 确认 `Session::run` 是 `&mut self`，`Arc<Session>` 编译失败（deref 只给 `&Session`）。v3 主方案改为 `Arc<Mutex<Session>>`（Mutex 提供内部可变性）。`Session: Send + Sync` 断言通过——回退非因 Send/Sync，纯因 `run &mut self`。

## 1. 背景

### 1.1 VAD 重复加载（① 主项）

coordinator 每次 Toggle（开始录音）实时构造 VAD：

- `coordinator.rs:606 / 656`（detection / streaming vad）+ filter_vad（VadSegmented 场景）调 `octopus_asr::vad::SileroVad::new(&path)`——内部 `Session::commit_from_file` 同步加载 ONNX，百 ms 级。**首次按快捷键 → 录音启动有明显延迟**。
- filter_vad 每个语音段都重新加载一次。
- `main.rs:210-226` preheat 只 preheat ASR model，不碰 VAD。

### 1.2 根因

`SileroVad::new()` 每次都 `Session::builder().commit_from_file(model_path)`——ONNX 加载慢。而 `SileroVad` 的可变状态只有 `h`/`c`（LSTM hidden/cell，各 128 元素）+ `sr`（标量），zeros 是纳秒级。重加载成本全在 Session。

### 1.3 音频回调锁（③，可选）

`audio.rs` 回调里 `mono.collect()` + 写 `Arc<Mutex<Vec<f32>>>`，coordinator `drain_samples` 持锁读。16kHz 语音，写远多于读，锁竞争低——项目长期稳定，无证据锁是瓶颈。

## 2. 目标

- **①**：VAD 的 ONNX Session 全局缓存（按 path），`SileroVad::new()` 廉价化；`main.rs` preheat 预加载，首次按下不再卡。
- detection/filter 语义完全不变（各自 owned `SileroVad`，h/c 独立）。
- **③**：方案记录但默认不实现。
- **#4**：保留。

## 3. 设计：session 级缓存（coordinator 零改动）

`SileroVad::compute(&mut self)` 只更新 `self.h`/`self.c`（自身字段）；`self.session.run(...)` 需要 `&mut Session`（ort 2.x `Session::run(&mut self)`，源码 `session/mod.rs:212`）。故：

- **Session 共享**：多个 `SileroVad` 实例共享同一 `Arc<Mutex<Session>>`——`Mutex` 提供内部可变性，满足 `run` 的 `&mut self`；`Arc` 提供共享所有权。
- **实例 owned**：detection 与 filter 各自 owned `SileroVad`，h/c 跨 tick 累积（detection）/ 每段 reset（filter）独立——无需两份缓存。
- `Session: Send + Sync`（编译期静态断言保证），可入全局 `OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<Session>>>>>`。

coordinator 调 `SileroVad::new(&path)`（签名/语义不变），**零改动**。

> 对比 v2 错误论据：「`run(&self)` 线程安全，故 `Arc<Session>` 可共享」——实际 `run` 是 `&mut self`。v3 改 `Arc<Mutex<Session>>`。

## 4. 方案（已实现）

### 4.1 vad.rs：session 缓存 + Arc<Mutex<Session>>

```rust
static VAD_SESSIONS: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<Session>>>>> = OnceLock::new();

pub struct SileroVad {
    session: Arc<Mutex<Session>>,
    h: Array3<f32>,
    c: Array3<f32>,
    sr: Array1<i64>,
}

impl SileroVad {
    pub fn new(model_path: &Path) -> Result<Self> {
        // 持 cache lock 完成 get-or-insert（消除 TOCTOU）：
        // 并发 miss 时只有一个线程加载，其余等锁后命中同一 Arc。
        let session = {
            let mut cache = vad_sessions().lock().unwrap();
            if let Some(s) = cache.get(model_path) {
                s.clone()
            } else {
                let s = Arc::new(Mutex::new(
                    Session::builder()?
                        .commit_from_file(model_path)?,
                ));
                cache.insert(model_path.to_path_buf(), s.clone());
                s
            }
        };
        Ok(Self { session, h: zeros, c: zeros, sr })
    }

    pub fn compute(&mut self, samples: &[f32]) -> Result<f32> {
        // ... 准备 input/h/c/sr tensor ...
        let mut session = self.session.lock().unwrap();   // &mut Session for run
        let outputs = session.run(...)?;
        // outputs 是 owned，guard 在函数末 drop；h/c 更新不依赖 session lock
    }

    pub fn reset(&mut self) { /* 只清 h/c，不碰 session，不需 lock */ }
}
```

要点：
- **持锁 get-or-insert**（commit `07a1503`）：消除原两步模式（`get→drop→load→re-lock insert`）的 TOCTOU；并发 miss 只加载一次。持锁期间 `commit_from_file`（~100ms）仅冷启动，可接受。
- `compute` 持 session lock 仅覆盖 `run`（outputs owned，h/c 更新独立）；`reset` 不锁 session。
- 单线程 coordinator 调用，session lock 无竞争。

### 4.2 coordinator：不改

`SileroVad::new(&path)` 签名/语义不变，coordinator.rs:606/656/675 调用点零改动。Stage enum 的 vad 字段仍是 owned `SileroVad`。

### 4.3 main.rs preheat（已实现）

preheat 后台线程闭包内，ASR `switch_model` 之后追加 VAD 预加载（`main.rs:227-234`）：

```rust
if let Ok(vad_path) = octopus_asr::config::find_silero_vad() {
    match octopus_asr::vad::SileroVad::new(&vad_path) {
        Ok(_) => info!("VAD session preheated"),
        Err(e) => log::warn!("VAD 预加载失败（不影响启动，首次录音懒加载）: {}", e),
    }
}
```

### 4.4 ③ lock-free 音频（可选，未实现）

无证据锁是瓶颈；①完成后若 profiling 显示热路径延迟再启动。

### 4.5 #4 paste sleep(50ms) — 保留

时序保证，不动。

## 5. 文件清单（实际）

| 文件 | 改动 |
|---|---|
| `crates/asr/src/vad.rs` | `session: Arc<Mutex<Session>>`；`VAD_SESSIONS` 缓存 + 持锁 get-or-insert；compute lock；2 单测；Send+Sync 断言 |
| `crates/desktop/src/main.rs` | preheat 后台线程加 VAD 预加载 |
| `crates/desktop/src/coordinator.rs` | 不改（零改动已验证） |

## 6. 风险（已验证）

- ~~`Session: Send + Sync` 不确定~~：断言通过（commit `c15c159`）。
- ~~`run &self`~~：实际 `&mut self` → `Arc<Mutex<Session>>`（v3 修正）。
- ~~TOCTOU（`get→drop→load→re-lock insert`）~~：持锁 get-or-insert 修复（commit `07a1503`），3 次多线程测试无 flake。
- poison panic（`.lock().unwrap()`）：compute/new panic 会 poison，下次 new panic。生产可接受（孤立 panic 本就需重启）；未做 `into_inner()` 容错（可选，非阻塞）。

## 7. 验证

- `cargo test -p octopus-asr`：42 passed, 6 ignored（dashscope 等需真实 key）。
- `cargo check --workspace --all-targets`：clean。
- coordinator 零改动（`git diff` 空）。
- 手动：首次按快捷键录音启动延迟显著降低（待用户本地确认）。
