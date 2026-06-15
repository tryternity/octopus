# 设计文档：嵌入式 DB 存储（识别历史 + 模型配置）

> 引入 rusqlite（SQLite），将识别历史（含原生 + AI 修正双份）与模型配置（model.json）迁入结构化存储，替代当前的纯文本 record.txt / history.txt / model.json。

> ⚠️ **本文为初版设计（2026-06-13），`models` 表 schema 已演进**——下文出现的 `is_active` 列 / 「每 domain 恰好一行 is_active=1」机制已**废弃**。当前实现：
> - DB 代码位于 `crates/infra/src/db.rs` + `crates/infra/src/db.sql`（经 desktop → asr → infra 三次下沉）。
> - `models` 表用 `is_local` / `is_enabled` / `is_streaming` 三列（**无 `is_active`**）；引擎激活改由 `config.yaml.asr_engine` 按 name 精确匹配。
> - schema 变更走「删库重初始化」（`user_version=0→1` 一次性建表 + seed），无 migration。
> - 新增 `domain='llm'` 行（LLM 润色模型，`load_llm_model` 读）。
>
> 当前真相以 [db-single-source 设计](2026-06-14-db-single-source-design.md) + [config-infra 设计](2026-06-14-config-infra-and-engine-truth-design.md) + [`architecture.md`](../../../architecture.md)「模型管理」段为准；本文保留作历史决策记录。

## 0. 背景

当前持久化全为纯文本文件：

| 文件 | 内容 | 读写 |
|------|------|------|
| `record.txt` | 当前会话识别文本（已被 polish 覆盖为修正版） | `save_record` 全量覆写 |
| `history.txt` | 归档历史，`--- 时间 ---\n内容` 分隔，FIFO 保留 20 条 | `archive_to_history` 手动解析 |
| `model.json` | 模型注册表（vad/asr 各引擎的 HF source） | `serde_json` 启动加载 |
| `config.yaml` | 运行配置（引擎、快捷键、LLM 连接） | `serde_yaml`，人可手编 |

问题：
- `accumulated_text` 在 polish 合并后被覆盖，**原生识别文本丢失**，无法「评估润色质量 / 留底」；
- 纯文本无结构、无查询、无事务，历史增长后难以检索/统计；
- 后续还有较多数据需要存储（运行时状态、统计等），需要一个统一的结构化后端。

## 1. 目标与范围

### 1.1 本次做

> 状态截至实现完成（提交 `70f1fd5` → `e69f918`，及修复 `efc6ef4` / `327e1de`）。

| 功能 | 状态 | 说明 |
|------|------|------|
| 引入 rusqlite | ✅ | `bundled` feature（自带 SQLite C 库，打包增量 ~1M，无系统依赖） |
| 识别历史表 `transcriptions` | ✅ | 每条完成识别存原生 + 修正双份 + 元数据 |
| 模型配置表 `models` | ✅ | model.json 拍平迁入，支持按 domain/category 查询、`is_active` 切换 |
| 双记录（raw + polished） | ✅ | coordinator 维护独立 `raw_text`，polish 不污染，入库双列 |
| 一次性迁移 | ✅ | 启动时若 DB 新建，从 history.txt + model.json 导入 |
| schema 版本管理 | ✅ | `PRAGMA user_version` 控制迁移 |
| 运行时模型查找接入 DB | ✅（修复 A，`efc6ef4`） | desktop 启动时从 DB 构造 `AppConfig` 并注入 `set_runtime_config`，asr 的 `load_config` 优先用注入版（见 §6） |

### 1.2 不做（本次）

| 不做 | 原因 |
|------|------|
| `config.yaml` 迁入 | 人可读可手编是核心价值；DB 只接管「数据」，配置仍走 yaml |
| 删除 model.json / record.txt 文件 | 迁移后自然废弃，desktop 代码不再读写；不强制删文件（cli/server 仍读 model.json） |
| 历史搜索 / 统计 UI | 表结构已支持，UI 后续做 |
| `duration_ms` 实际计时 | 表保留字段，首期 INSERT 填 NULL，未来补录音计时 |
| 通用 KV 表 | YAGNI，未来需要运行时状态存储时再加 |

> **更新（修复 A）**：早期版本将「运行时模型查找接入 DB」列为本期不做项；实际已实现（见 §1.1 末行与 §6）。当前分工：**desktop** 运行时模型查找走 DB（启动期注入），**cli/server** 仍读 `model.json`（不注入，保持兼容）。

## 2. 选型

**rusqlite（SQLite）**，`features = ["bundled"]`。

- 成熟稳定、单文件 DB、完整 SQL、事务安全；
- `bundled` 自带 C 库，项目已有 ONNX Runtime C++ 工具链，无额外构建负担；
- 可用任意 SQLite 客户端直接查看/编辑（开发期手编模型配置）；
- 不选 sled：长期 beta、KV 模型对结构化历史不直观、无 CLI。

## 3. 表结构

### 3.1 `transcriptions`（识别历史）

```sql
CREATE TABLE transcriptions (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    created_at    TEXT    NOT NULL,               -- 'YYYY-MM-DD HH:MM:SS'，TEXT 可排序
    engine        TEXT    NOT NULL,               -- 引擎名，如 'paraformer-streaming'
    engine_mode   TEXT,                           -- 'streaming' | 'vad_segmented'
    raw_text      TEXT    NOT NULL,               -- 原生识别（未经 polish）
    polished_text TEXT,                           -- AI 修正后；NULL = 未成功润色
    polish_status TEXT    NOT NULL DEFAULT 'off', -- 'off' | 'done' | 'failed'
    polish_model  TEXT,                           -- 润色用 LLM，如 'deepseek-v4-flash'
    duration_ms   INTEGER,                        -- 录音时长（首期 NULL，未实现计时）
    char_count    INTEGER                         -- 展示文本字符数（统计用）
);

CREATE INDEX idx_trans_created ON transcriptions(created_at DESC);
CREATE INDEX idx_trans_engine  ON transcriptions(engine);
```

字段语义：
- `raw_text` 必有（NOT NULL）；`polished_text` 仅在 `polish_status='done'` 时填值，否则 NULL。
- `polish_status`：`off`（未启用 polish）/ `done`（成功）/ `failed`（启用但失败）。支撑「评估润色质量」——可统计成功率、对比失败案例、按 LLM 模型分组。
- 展示 / 粘贴逻辑：优先 `polished_text`，fallback `raw_text`（在内存层处理，见 §5）。
- `engine` + `engine_mode` 索引：支持「Paraformer vs Zipformer」「流式 vs 伪流式」识别质量统计。
- `char_count`：INSERT 时由 `db::insert_transcription_at` 计算 `display = polished_text.unwrap_or(raw_text)` 的 `.chars().count()`（即展示文本字符数，非 polish 专属）。`duration_ms` 首期 NULL。

### 3.2 `models`（模型配置，model.json 拍平迁入）

```sql
CREATE TABLE models (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    domain       TEXT    NOT NULL,   -- 'asr' | 'vad'
    category     TEXT    NOT NULL,   -- 'whisper'|'sensevoice'|'paraformer'|'qwen3_asr'|'zipformer'|'silero'
    name         TEXT    NOT NULL,   -- 'paraformer-streaming'
    source       TEXT    NOT NULL,   -- HF source
    language     TEXT    NOT NULL DEFAULT '',
    description  TEXT    NOT NULL DEFAULT '',
    secret_key   TEXT    NOT NULL DEFAULT '',  -- 存储 API 形式下的 key，本地模型留空
    is_active    INTEGER NOT NULL DEFAULT 0,    -- 每个 domain 恰好一行 =1
    UNIQUE(domain, category, name)
);
```

- 嵌套映射 `domain → category → name → entry` 拍平为行；`is_active` 标志替代原 JSON 的 `active` 字段。
- 切换引擎 = `UPDATE models SET is_active = (id == ?) WHERE domain = ?`，比改 JSON 干净。
- `vad.active` 原为空串（用默认 silero）：迁移时该 domain 不设 `is_active=1` 行，代码侧查不到 active 则用默认。

### 3.3 schema 版本

```sql
PRAGMA user_version = 1;   -- 初始版本，未来迁移递增
```

启动时读 `PRAGMA user_version`：为 0 → 全新建表 + 迁移；为 1 → 直接使用；>1 → 未来增量迁移。

## 4. DB 管理

| 项 | 说明 |
|----|------|
| 文件位置 | `~/.octopus/octopus.db`（与现有文件同目录） |
| 连接 | 单连接，`Mutex<Connection>` 包装（与现有 `StreamingSession` 的 Mutex 模式一致） |
| 初始化 | 首次打开时建表 + 设 `user_version=1` |
| 依赖 | `crates/desktop/Cargo.toml` 增 `rusqlite = { version = "0.31", features = ["bundled"] }` |

## 5. 数据流改造

### 5.1 内存：新增 `raw_text`

`Stage::Streaming` 与 `Stage::VadSegmented` 各新增字段：

```rust
raw_text: String,   // 纯 ASR 原生增量，polish 不触碰
```

- 每次 `accept_samples` / `flush` 返回新文本 → 同时追加到 `raw_text`（delta）和 `accumulated_text`；
- `handle_polish_done` 合并只改 `accumulated_text`，**不动 `raw_text`**；
- 用户在结果窗口的编辑（`result-edited`）只更新内存 `accumulated_text`（展示版），不污染 `raw_text`。

> 关键不变量：`raw_text` 始终是完整、未经任何润色的原生识别全文。

### 5.2 INSERT 时机

> **实现状态**：已实现（修复 B，提交 `327e1de`）。

`Stage::Pasting` 为结构变体，持入库所需数据：`raw_text` / `polished_text` / `polish_status` / `engine` / `engine_mode`。

流程：`Toggle 停止 → 最终润色 → start_pasting（构造 Stage::Pasting，**暂不入库**）→ 粘贴 → 粘贴完成发 Command::PasteDone → 【INSERT transcriptions】`。

INSERT 时机在 **`PasteDone`（粘贴完成后）**，而非最初设想的「润色完成后、粘贴前」。延迟入库的好处：用户若在结果窗口编辑了文本，编辑后的 `polished_text` 会被写入入库。

`polish_status` 基于**润色调用结果**（`start_pasting` 内 `config.llm_config()` 调用返回），而非文本比较：

| polish 调用结果 | raw_text | polished_text | polish_status |
|----------------|----------|---------------|---------------|
| 未启用 polish（`llm_config()` 为 None） | 原生全文 | NULL | `off` |
| 启用且返回非空（Ok） | 原生全文 | 润色结果 | `done` |
| 启用但返回空（Ok）或调用失败（Err） | 原生全文 | NULL | `failed` |

- `engine` / `engine_mode` 取自当前会话配置（Pasting 阶段持有）；
- `polish_model` 取自 `config.llm_model`（仅 `done` 时入库，否则 NULL）；
- `polished_text` 仅 `done` 时入库（`Some`），`off` / `failed` 时为 `None`（NULL）；
- `char_count` = 展示文本（`polished_text.unwrap_or(raw_text)`）的 `.chars().count()`（见 §3.1）；
- `created_at` 由 `db::now_string()` 生成（`'YYYY-MM-DD HH:MM:SS'`）；
- `duration_ms` 首期 NULL。

> 粘贴交互本身（`paste.rs`）仍用润色结果 `final_text`（编辑前的版本）——即粘贴给目标窗口的文本与入库的 `polished_text` 可能在用户编辑后不一致；这是有意取舍，避免粘贴过程中再次延迟。

### 5.3 result_window.rs 改造

> ⚠️ **已移除（2026-06-14）— 用户编辑回写 polished_text**：本节及 §5.1 / §5.2 中「用户在结果窗口编辑 → 回写 `polished_text`」的链路已整体移除——编辑态与中间润色流耦合冲突（详见 `2026-06-12-squid-desktop-design-v2` 顶部注释）。现状：结果窗口只读，入库 `polished_text` = `start_pasting` 时的纯润色结果，无用户编辑叠加；INSERT 仍在 `PasteDone` 时机。`Command::ResultEdited` / `handle_result_edited` / `report_result_edit` 均已删除。原文保留以记录设计演进。

> **实现状态**：已实现（Task 9，提交 `e69f918`；编辑回写分支由修复 B 完善，`327e1de`）。

| 原 API | 改造 |
|--------|------|
| `save_record(text)` | **删除**（record.txt 废弃）。`result-edited` 事件改为发 `Command::ResultEdited { text }` 给 coordinator |
| `archive_to_history()` | **删除**，归档逻辑由 `db::insert_transcription` 接管 |
| `clear_result` | 不再归档；粘贴完成后清空 + 隐藏窗口 |
| `record_file_path` / `clear_record_file` / `parse_history_entries` / `history_file_path` / `chrono_now_string` 等共 9 个函数 | **删除**（时间格式化 `now_string`/`days_to_ymd`/`is_leap` 已移至 db.rs） |

> 编辑回写：前端 `result-edited` 事件经 `Coordinator::report_result_edit` 发 `Command::ResultEdited` → `handle_result_edited`。该 handler 在 `Stage::Pasting` 分支**更新 `polished_text`（不动 `raw_text`）**——即用户编辑会反映到最终入库的 `polished_text`。其他 Stage 分支忽略编辑事件。

## 6. 一次性迁移

> **实现状态**：已实现（Task 3/4/6，提交 `70f1fd5` → `e69f918`）。

启动时（DB 初始化阶段，`user_version == 0`）：

1. **建表** + `PRAGMA user_version = 1`。
2. **model.json → models**：若 `~/.octopus/model.json` 存在，`serde_json` 解析为 `AppConfig`，遍历 `vad` / `asr` 两域，每条 entry INSERT 一行（`domain`/`category`/`name`/`source`/`language`/`description`/`secret_key`，active 项置 `is_active=1`）。`INSERT OR IGNORE` + `UNIQUE(domain, category, name)` 保证幂等。
3. **history.txt → transcriptions**：若存在，用 `parse_history_entries` 解析每条，INSERT（`raw_text = polished_text = 原内容`，`polish_status='done'`，`created_at = 条目时间戳`，`engine`/`engine_mode` 留空）。事务原子。
4. 迁移完成后 model.json / record.txt / history.txt **desktop 不再读写**（自然废弃，不删文件）。

> 迁移是幂等的前提：仅在 `user_version == 0`（全新 DB）时执行。已初始化的 DB 重复启动不重跑。

### 6.1 迁移后运行时模型查找由 DB 注入（修复 A，`efc6ef4`）

迁移完成后，**desktop 运行时模型查找从 DB 读，不再读 model.json**：

- `crates/desktop/src/db.rs` 提供 `load_app_config()`：从 `models` 表构造 `AppConfig` 返回。**关键映射**：DB 的 `category` 列存 JSON key（迁移时直接取，如 `"qwen3-asr"` 带 dash），`AsrSection` 字段是 `qwen3_asr`（下划线）；`load_app_config_at` 按 dash 形式 category 分派到对应字段。空库返回 `None`。
- `crates/desktop/src/main.rs` 在 `db::init()` 后调用 `db::load_app_config()`：返回 `Some(cfg)` → `octopus_asr::config::set_runtime_config(cfg)` 注入；返回 `None` → warn 回退。
- `crates/asr/src/config.rs`：`static RUNTIME_CONFIG: OnceLock<AppConfig>`；`load_config()` 优先返回注入版（`cfg.clone()`），未注入回退读 model.json。`resolve_engine_category` / `find_silero_vad` / `list_engines` 等模型查找函数现从 DB（经注入）读。
- **cli/server 不注入**，仍读 model.json（保持兼容）。

> 注入为**启动期一次性**（`OnceLock`），运行中不可热更新。手编 `models` 表后需重启 desktop 才生效（见 §8 步骤 6）。

## 7. coordinator 集成点

> **实现状态**：全部已实现（Task 7/8 + 修复 B，`70f1fd5` → `327e1de`）。

| 位置 | 改动 | 状态 |
|------|------|------|
| `Stage::Streaming` / `VadSegmented` | 新增 `raw_text: String`，Toggle 开始时初始化为空 | ✅ |
| `Stage::Pasting`（新增结构变体） | 持 `raw_text` / `polished_text` / `polish_status` / `engine` / `engine_mode` | ✅（修复 B） |
| `handle_streaming_tick` / `handle_vad_segmented_tick` | 识别增量同时追加 `raw_text` 与 `accumulated_text` | ✅ |
| `handle_polish_done` | 仅合并 `accumulated_text`，不碰 `raw_text` | ✅ |
| `start_pasting` | 调用 `llm_config()` 得润色结果与 `polish_status`；构造 `Stage::Pasting`（**暂不入库**）；启动粘贴 | ✅（修复 B） |
| `Command::PasteDone` 分支 | 从 `Stage::Pasting` 取数据调 `insert_transcription(...)`（用户编辑已反映到 `polished_text`） | ✅（修复 B） |
| 新增 `Command::ResultEdited { text }` | 前端编辑回写 → `handle_result_edited` → `Stage::Pasting` 分支更新 `polished_text`（不动 `raw_text`） | ✅ |

## 8. 验证

1. `cargo build --package octopus-desktop --features embedded`（确认 rusqlite bundled 编译通过）
2. 删除 `~/.octopus/octopus.db`（若有），保留现有 history.txt + model.json → 启动 → 确认：
   - `octopus.db` 生成，`transcriptions` / `models` 表存在，`user_version=1`
   - 现有 8 条 history 已导入 `transcriptions`
   - model.json 各引擎已导入 `models`，`asr` 域 `paraformer-streaming` 的 `is_active=1`
3. 录一段音（启用 polish）→ 停止 → 确认 `transcriptions` 新增一行，`raw_text` 为原生、`polished_text` 为润色版、`polish_status='done'`
4. 关闭 polish 再录一段 → 确认 `polished_text=NULL`、`polish_status='off'`
5. **在结果窗口手动编辑文本 → 等待粘贴完成（PasteDone）→ 确认入库的 `polished_text` 为编辑后版本、`raw_text` 仍为原生**。此步骤现已成立（INSERT 推迟到 `PasteDone`，`handle_result_edited` 在 Pasting 阶段更新 `polished_text`）。
6. 用 SQLite 客户端打开 `octopus.db` 手编 `models`（加一个引擎）→ **需重启 desktop** 后确认程序读到新配置（运行时配置为启动期 `OnceLock` 注入，运行中不可热更新；见 §6.1）。
