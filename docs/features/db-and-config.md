# 数据持久化与配置

> 嵌入式 SQLite（`~/.octopus/octopus.db`）是唯一存储——识别历史、剪贴板历史、模型配置、应用配置、润色 prompt、图片 BLOB 全部在这一个库。WAL 模式 + ReentrantMutex 并发安全，schema v18。

源文件：`crates/infra/src/db.rs`、`crates/infra/src/db.sql`、`crates/infra/src/config.rs`、`crates/desktop/src/db_queue.rs`。

---

## 1. SQLite 连接

- **存储**：`~/.octopus/octopus.db`（`crates/infra/src/db.rs`，全局 `OnceLock<parking_lot::Mutex<Connection>>`）
- **WAL 模式**：`ensure_db` 打开后设 `PRAGMA journal_mode=WAL` + `busy_timeout=5000`（多任务并发友好，server 多连接不再 SQLITE_BUSY）
- **并发约束**：`with_db` 内部用 `parking_lot::ReentrantMutex`（**同线程可重入**，无毒化）——闭包内可安全地再调 `with_db`（如间接读 config / 查模型 meta）。回归测试 `with_db_reentrant_no_deadlock` 守护。仍为单连接排他。
- `with_db` 为公开 API 供其他 crate 调用。

---

## 2. schema v18

**开发期简化**：`init_schema` 以 db.sql 为唯一 schema 真相：
- `user_version >= 18` 跳过
- v17 库跑 FTS5 backfill 升到 v18
- 其他（含全新库）跑 db.sql 建表+seed+yaml 导入一次性到 v18
- 无历史迁移链、无 DROP 兜底（开发期无旧库需兼容）

schema 变更直接改 db.sql + 升 user_version，删库重初始化。

---

## 3. 表结构

### clipboard_history（统一存储）

统一存储 text/voice/ocr/image/file——`item_type` 枚举区分。

| 列 | 类型 | 说明 |
|---|---|---|
| `id` | `INTEGER PRIMARY KEY` | voice = 识别开始毫秒时间戳；其他 = 自增 |
| `item_type` | `TEXT` | `text` / `voice` / `ocr` / `image` / `file` |
| `source` | `TEXT` | `clipboard` / `asr` |
| `content` | `TEXT` | voice/ocr/text 全文；image/file 为空串 |
| `ref_data` | `TEXT` | image=blob_hash；file=JSON 路径数组 |
| `meta_info` | `TEXT` | JSON，按 item_type 存不同 schema |
| `segments` | `TEXT` | 仅 voice 段 JSON `[{kind, text}]` |
| `is_favorite` / `is_rich` / `has_thumbnail` | `INTEGER` | 0/1 |
| `created_at` | `TEXT` | iso 时间戳 |

**三层模型**：`content`（扁平文本）+ `ref_data`（引用数据）+ `meta_info`（JSON 元信息）。

v17 废弃原 `transcriptions` 表（db.sql 不再含此表）。

### clipboard_history_fts（FTS5 虚表）

- trigram tokenizer，索引 `content` 列
- voice/ocr/text 被搜索，image/file content 为空串自动跳过
- 3 触发器自动同步：`clip_fts_ai`（INSERT）、`clip_fts_ad`（DELETE）、`clip_fts_au`（AFTER UPDATE OF content）

### image_data（图片 BLOB）

| 列 | 类型 |
|---|---|
| `hash` | `TEXT PRIMARY KEY`（SHA-256） |
| `blob` | `BLOB`（WebP 编码） |
| `thumb` | `BLOB`（240×240 缩略图） |
| `image_type` | `TEXT` |
| `width` / `height` | `INTEGER` |
| `created_at` | `TEXT` |

`clipboard_history.ref_data` 引用 `image_data.hash`；删除条目时引用计数为 0 才删 image_data 行。

### models（模型目录）

唯一来源，schema 见 `crates/infra/src/db.sql`，首次建库 `user_version=0` 时整体执行一次 seed。

| 列 | 说明 |
|---|---|
| `domain` | `asr` / `llm` |
| `provider` | vendor/运行位置（local/aliyun/bytedance/tencent/baidu/deepseek/bigmodel） |
| `category` | 引擎族/模型系列（zipformer/whisper/paraformer 等） |
| `model_name` | 具体模型名 |
| `source` | 模型路径 / Resource ID / AppID（按 provider 不同语义） |
| `secret_key` | API Key / SHA256 清单（local 模型）/ HMAC 密钥 |
| `language` | 支持语种 |
| `is_local` | 0/1 |
| `is_thinking` | 0/1 |
| `is_streaming` | 0/1 |
| `is_enabled` | 0/1（文件就绪语义，v2） |
| `description` | 描述 |

唯一键 `UNIQUE(domain, provider, category, model_name)`。

- 本地 ASR（is_local=1，12 行）初始**全 `is_enabled=0`**（待下载就绪）
- 默认兜底引擎 `zipformer-small-ctc` 代码写死（`FALLBACK_ASR_ENGINE_NAME`）不占 seed 行
- `load_models_at` 仅读 `domain='asr' AND is_enabled=1`
- `domain='llm'` 经 `load_llm_model(spec)` 按 3-part spec 读
- 引擎激活由 `app_config.asr_engine` 决定，无 `is_active` 列

### app_config（应用行为配置）

v3+，替代旧 `config.yaml`。29 字段 key-value TEXT，含 `category` 分组列默认 `'default'` + `description` 描述列。

由 db.sql seed 默认值 + `load_app_config()` 按字段类型解析。写入用 `ON CONFLICT DO UPDATE SET config_value`（仅改值，保留 description + category）。

旧 `config.yaml` 首次启动时一次性导入 DB 后重命名为 `.bak`（迁移逻辑在 `init_schema` 中）。

另有 `active_polish_prompt` key（存 prompts 表 id 字符串，默认 `'1'`）。

### prompts（润色提示词管理）

v4+，多 prompt 管理（替代旧单文件 `VOICE_POLISH.md`）。

| 列 | 说明 |
|---|---|
| `id` | PK AUTOINCREMENT（用户不可编辑） |
| `title` | 用户可读别名（允许重复） |
| `category` | 固定 `voice_text_polish` |
| `content` | 风格规则（不含增量逻辑） |
| `description` | 描述 |
| `is_system` | 0/1 |
| 时间戳 | — |

seed 2 条系统内置：`id=1` 默认润色 + `id=2` 进阶润色（断续纠正），均 `is_system=1`（不可编辑/删除）。

`app_config.active_polish_prompt` 存激活 id（默认 `'1'`）。

`llm::prompt::build_system_prompt(content) = content + INCREMENTAL_RULE`（第 7 条增量规则代码常量强制拼接，用户不可见）。

---

## 4. 非阻塞 DB 写入（actor 模式）

`crates/desktop/src/db_queue.rs`——ASR 识别结果的 DB 写入 actor。

ASR 过程中的 `INSERT`/`UPDATE`/`finalize` 不在协调器线程同步执行——调用方仅 `db_queue::get_db_sender().send(DbCommand)` 入队后立即返回，真实落库由后台 DB 写线程单线程消费。

**DbCommand enum**：

| 变体 | 语义 |
|---|---|
| `Insert` | 首次有 ASR 文本时 INSERT voice 条目 |
| `UpdateTextSegments` | 分段 / 流式 partial 增量更新 |
| `UpdatePolished` | 停顿润色 / 立即润色完成 |
| `Finalize` | 停止时完整写入 |
| `UpdateEditedSegments` | 用户编辑提交 |
| `Delete` | Cancel 时删除未完成记录 |

**后台线程**：
- `DB_SENDER: OnceLock<Sender<DbCommand>>` 懒初始化 spawn
- `recv_timeout` 轮询 `DB_SHUTDOWN: AtomicBool`
- mpsc FIFO 保证同 id 的 `Insert` 必在 `UpdateRaw` 之前被消费

**关机优雅 drain**（`shutdown_db`）：
- 置 `DB_SHUTDOWN` → 后台线程排空 `try_iter()` 后退出
- `DB_HANDLE: OnceLock<Mutex<Option<JoinHandle>>>` take join
- `main.rs` 挂到 `tauri::RunEvent::ExitRequested`，保证退出前队列清空

---

## 5. 过程增量入库

voice 条目 id = 识别开始毫秒时间戳。入库时机分散到识别过程各事件，每次同步写 `segments`（真相源）+ `content`（扁平）+ `meta_info`（json_set 更新 char_count/polished/duration_ms）：

| 事件 | DbCommand |
|------|-----------|
| 首次有 ASR 文本 | `Insert`（INSERT voice 条目） |
| 分段 / 流式 partial | `UpdateTextSegments` |
| 停顿润色完成 | `UpdatePolished` |
| 停止 | `Finalize`（含 `duration_ms`） |

DB 失败仅 `warn` log 不阻塞识别（best-effort）。

`mark_db_inserted()` 在 `send` 后即置位仍安全——真实顺序由 mpsc channel 保，不由标志位保。

---

## 6. AppConfig

`infra::config::AppConfig`（`octopus_infra::config::load_config()` → `db::load_app_config()`）——应用配置统一 schema，29 字段：

| 字段 | 默认 | 说明 |
|------|------|------|
| `microphone_device` | — | 麦克风设备名 |
| `asr_engine` | `local:zipformer:zipformer-small-ctc` | 3-part 引擎 spec |
| `polish_llm` | — | LLM 3-part spec |
| `polish_mode` | 0 | 0 关闭 / 1 仅最终 / 2 中间+最终 |
| `paste_method` | `clipboard` | `clipboard` / `direct` / `none` |
| `write_to_clipboard` | `true` | 粘贴后是否留剪贴板 |
| `asr_hardware_accelerated` | `false` | GPU 加速 |
| `asr_correct` | `false` | 拼音纠错 |
| `denoise_mode` | 1 | 0 关 / 1 RNNoise / 2 DeepFilterNet3 |
| `output_simplified` | `true` | 简繁归一化 |
| `hide_toolbar` | `false` | 工具栏自动隐藏 |
| `segment_silence` | 400 | VAD 段静音阈值 ms |
| `pause_polish_threshold_ms` | 600 | 停顿润色阈值 ms |
| `clipboard_enabled` | `true` | 剪贴板监听 |
| `clipboard_max_items` | 1000 | 最大保留条数 |
| `clipboard_max_age_days` | 30 | 自动清理天数 |
| `ocr_model` | PP-OCRv5 | OCR 模型名 |
| `switch_input_source_on_paste` | true | 粘贴前临时切到 ABC 输入法（仅 macOS，防 CJK 乱码） |
| 快捷键们 | — | `asr_shortcut` / `clipboard_shortcut` / `screenshot_shortcut` / `edit_shortcut` / `edit_global_shortcut` / `polish_global_shortcut` / `action_bar_shortcut` |

### 环境变量系统（v22 新增，`category='env'`）

`app_config` 表新增 `category='env'` 分组，与普通配置同表隔离。3 个内置变量：
- `huggingface`（默认 `https://hf-mirror.com`，key 不可改）
- `modelscope`（默认 `https://modelscope.cn`，key 不可改）
- `github`（默认 `https://github.com`，key 不可改）
- 用户可自定义任意 key-value（均可改/删）。

**模板替换规则**：ASR 模型下载 URL 中的 `{huggingface}`/`{modelscope}` 等占位符在下载时替换为实际值。仅 ASR 模型下载替换，LLM/OCR source/API URL 不替换。旧的 `download_mirror`（`category='setting'`）已废弃，启动时自动迁移到 `env.huggingface`。

`active_polish_prompt` 由 `db::load_active_prompt_id()` 独立读取，不入 AppConfig struct。

---

## 7. SharedRuntimeConfig

`type SharedRuntimeConfig = Arc<RwLock<AppConfig>>`（挂 `tauri::State`）——**完整 `AppConfig` 的唯一真相源**，取代旧 `RuntimeConfig` 部分镜像（消除字段同步遗漏，新增运行时生效字段零同步代码）。

工具栏可运行时切换（无需重启）：`asr_engine` / `polish_mode` / `polish_llm` / `denoise_mode`。

8 个 Tauri 命令：`toolbar_state` / `list_asr_engines` / `switch_asr_engine` / `set_polish_mode` / `list_llm_models` / `switch_polish_llm` / `set_denoise_mode` / `polish_now`。

读写共享 `AppConfig`（即时生效）+ `persist_*` best-effort 持久化回 DB（写盘失败仅 `warn`，本次仍生效、重启回退）。

**`switch_asr_engine` / `switch_polish_llm`** 前端传裸 `model_name`，后端查 DB 取 `provider` / `category` 构造 3-part spec 写入，保证持久化值与 `parse_model_spec` 解析一致。

---

## 8. 配置持久化

| 方式 | 语义 |
|------|------|
| `persist_*`（单键 `save_config_key`） | ON CONFLICT 仅改 config_value |
| `set_config`（全量 `save_app_config`） | 30 字段 ON CONFLICT，包 `unchecked_transaction`（原子，中途崩溃全回滚） |

均写 DB。旧 `write_config_yaml` 已移除。

`model.json` / `history.txt` / `record.txt` 已从代码彻底删除——DB 是唯一配置/存储源。

---

## 9. 运行时文件布局

```
~/.octopus/
├── octopus.db          # 嵌入式 SQLite（models + clipboard_history + app_config + prompts + image_data 表，唯一存储）
├── config.yaml.bak     # 旧 config.yaml 迁移后的备份（首次启动自动生成，可安全删除）
└── models/
    ├── silero_vad_v4.onnx   # VAD（1.8M，find_silero_vad 固定加载，随包）
    ├── zipformer/           # 默认 ASR（27M，随包）
    └── <HF repo>/           # cli download 下的大模型

~/.cache/huggingface/hub/   # 旧 hf-cli 大模型缓存（兼容：resolve 第 4 级仍查此处）
```
