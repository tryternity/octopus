# 数据持久化与配置

> 嵌入式 SQLite（`~/.octopus/octopus.db`）是唯一存储——识别历史、剪贴板历史、模型配置、应用配置、润色 prompt、图片缩略图、vault 三表、热词（sets/words/hits）、方言规则、收藏、录屏记录、命令面板/agent 表全部在这一个库（22 张表 + FTS5 虚表）。WAL 模式 + ReentrantMutex 并发安全，schema v60。

源文件：`crates/infra/src/db/`（mod.rs + 按域拆分子模块：prompts / config / transcription / agent / hotword / action_bar / vault / models / clipboard_favorite）、`crates/infra/resources/sql/schema.sql`（2026-08-04 内联资源集中化时从 `infra/src/db.sql` 迁入 `infra/resources/`）、`crates/infra/src/config.rs`、`crates/desktop/src/core/db_queue.rs`。

---

## 1. SQLite 连接

- **存储**：`~/.octopus/octopus.db`（`crates/infra/src/db/mod.rs`，全局 `OnceLock<parking_lot::Mutex<Connection>>`）
- **WAL 模式**：`ensure_db` 打开后设 `PRAGMA journal_mode=WAL` + `busy_timeout=5000`（多任务并发友好，server 多连接不再 SQLITE_BUSY）
- **并发约束**：`with_db` 内部用 `parking_lot::ReentrantMutex`（**同线程可重入**，无毒化）——闭包内可安全地再调 `with_db`（如间接读 config / 查模型 meta）。回归测试 `with_db_reentrant_no_deadlock` 守护。仍为单连接排他。
- `with_db` 为公开 API 供其他 crate 调用。

---

## 2. schema v60 与迁移机制

**开发期简化**：`init_schema` 以 schema.sql 为唯一 schema 真相（分支详见 [architecture.md §octopus-infra 迁移机制](../architecture.md)）：

- `v == 0` 全新库：跑 schema.sql 建表 + seed + 旧 config.yaml 一次性导入 + `fill_manifests` + `load_external_seeds` → 直设 v60
- `v == 60` no-op
- `v < 60` 且 ≥54：**while 迁移链**逐版本升级（v54→v55 `asr_correct` 翻 true / v55→v56 `fuzzy_dialect_rules` / v56→v57 `hotword_words` 拆分 / v57→v58 set 软删（事务化）/ v58→v59 clipboard id 改 UUID **破坏性 bail** / v59→v60 vault is_deleted i64 化）
- `0 < v < 54`：bail 提示清库（不支持自动迁移）
- `v > 60`：bail（防高版本 binary 建库后回退读坏新 schema）

schema 变更直接改 schema.sql + 升 `CURRENT_SCHEMA_VERSION`。

---

## 3. 表结构（主要表）

### clipboard_history（统一存储）

统一存储 text/voice/ocr/image/file——`item_type` 枚举区分。

| 列 | 类型 | 说明 |
|---|---|---|
| `id` | `TEXT PRIMARY KEY` | **UUID v4**（v59 改，原 INTEGER 毫秒戳——改 UUID 作跨设备 sync 锚点） |
| `item_type` | `TEXT` | `text` / `voice` / `ocr` / `image` / `file` |
| `source` | `TEXT` | `clipboard` / `asr` |
| `content` | `TEXT` | voice/ocr/text 全文；image/file 为空串 |
| `ref_data` | `TEXT` | image=文件 hash；file=JSON 路径数组 |
| `meta_info` | `TEXT` | JSON，按 item_type 存不同 schema |
| `segments` | `TEXT` | 仅 voice 段 JSON |
| `is_deleted` | `INTEGER` | 软删标志（0=活跃；1=voice 软删标记——**仅 voice 用**，text/ocr/image/file 删除即物理删；详见 [clipboard.md](./clipboard.md)） |
| `is_favorite` / `is_rich` / `has_thumbnail` | `INTEGER` | 0/1 |
| `created_at` | `TEXT` | iso 时间戳 |

**三层模型**：`content`（扁平文本）+ `ref_data`（引用数据）+ `meta_info`（JSON 元信息）。收藏同步走独立 `clipboard_favorites` 表（tombstone 语义，详见 architecture.md §octopus-clipboard）。

v17 废弃原 `transcriptions` 表（schema.sql 不再含此表）。

### clipboard_history_fts（FTS5 虚表）

- trigram tokenizer，索引 `content` 列；id 改 TEXT 后用隐式 `rowid` JOIN（v59）
- voice/ocr/text 被搜索，image/file content 为空串自动跳过
- 3 触发器自动同步：`clip_fts_ai`（INSERT）、`clip_fts_ad`（DELETE）、`clip_fts_au`（AFTER UPDATE OF content）

### image_data（图片缩略图）

**2026-07-29 起原图改文件系统存储**（`~/Documents/octopus/screens/<hash>.jpg`，MD5 命名），DB 只存缩略图：

| 列 | 类型 |
|---|---|
| `hash` | `TEXT PRIMARY KEY`（MD5，同时是原图文件名） |
| `thumb` | `BLOB`（240×240 缩略图） |
| `width` / `height` | `INTEGER` |
| `created_at` | `TEXT` |

删除条目时引用计数为 0 才删文件 + DB 行。

### models（模型目录）

唯一来源，schema 见 `crates/infra/resources/sql/schema.sql`，首次建库整体执行一次 seed。

| 列 | 说明 |
|---|---|
| `domain` | `asr` / `llm` / `ocr` / `translate` |
| `provider` | vendor/运行位置（local/aliyun/bytedance/tencent/baidu/deepseek/bigmodel） |
| `category` | 引擎族/模型系列（zipformer/whisper/paraformer 等） |
| `model_name` | 具体模型名 |
| `source` | 模型路径 / Resource ID / AppID（按 provider 不同语义） |
| `secret_key` | API Key / SHA256 清单（local 模型）/ HMAC 密钥 |
| `language` | 支持语种 |
| `source_type` | 0=builtin(内置) / 1=local(用户下载) / 2=cloud(云端)，v48（原 `is_local`） |
| `is_thinking` / `is_streaming` | 0/1 |
| `is_available` | 0/1 **可用**（文件就绪/配置完整，同域可多个） |
| `is_enabled` | 0/1 **激活**（每域仅 1 个=1，2026-07-17 语义重构后） |
| `description` | 描述 |

唯一键 `UNIQUE(domain, provider, category, model_name)`。

- ASR seed 14 行全 local/builtin（云端模型**不再 seed**，用户在设置页手填）；`is_enabled` 全 0（激活时设）
- builtin 兜底引擎 `zipformer-small`（source_type=0，27M）：seed 占一行，首次启动由 `ensure_builtin_seed` 注入 + `fill_manifests` 填 manifest
- `load_models_at` 仅读 `domain='asr' AND is_enabled=1 AND is_available=1 LIMIT 1`（激活的那一个）
- **引擎激活由 DB `is_enabled` 决定**：每域仅 1 个=1，经 `switch_active_model(domain, id)` 切换。`app_config` 的 `asr_engine` / `polish_llm` / `ocr_model` / `translate_engine` 4 字段已删除（2026-07-17）
- LLM provider 预设（7 家 base_url）外置 `crates/infra/seeds/llm_providers.json` 运行时加载（`load_external_seeds`），不在 schema.sql 里

### app_config（应用行为配置）

替代旧 `config.yaml`。**45 字段** key-value TEXT，含 `category` 分组列（默认 `'default'` / 环境变量 `'env'`）+ `description` 描述列。

由 schema.sql seed 默认值 + `load_app_config()` 按字段类型解析。写入用 `ON CONFLICT DO UPDATE SET config_value`（仅改值，保留 description + category；`save_app_config` serde 自动遍历全字段，包 `unchecked_transaction`）。

旧 `config.yaml` 首次启动时一次性导入 DB 后重命名为 `.bak`（迁移逻辑在 `init_schema` 中）。

另有 `active_polish_prompt` key（存 prompts 表 id 字符串，默认 `'1'`，由 `db::load_active_prompt_id()` 独立读取，不入 AppConfig struct）。

### prompts（润色提示词管理）

多 prompt 管理；**v50 改造**：`content` 从完整 md 文本改为**文件名引用**（不含 `.md`），运行时读 `~/.octopus/.sync/prompts/polish/<content>.md`。

| 列 | 说明 |
|---|---|
| `id` | PK AUTOINCREMENT（用户不可编辑） |
| `title` | 用户可读别名（允许重复） |
| `category` | 固定 `voice_text_polish` |
| `content` | **文件名引用**（v50）；运行时 `read_prompt_file(content)` 读 md 文件 |
| `description` | 描述 |
| `is_system` | 0/1 |
| `app_bundle_ids` | JSON 数组（如 `["com.tencent.xinWeChat"]`），空=全局——**按前台 app 绑定 prompt**（app-aware 润色，2026-08-01） |
| `inject_context` | 0/1——1 时 user prompt 头部注入「当前应用：名称」上下文 |
| 时间戳 | — |

seed 3 条系统内置：`id=1` faithful（忠实校对）/ `id=2` user-intent（意图整理）/ `id=3` app-casual（口语化整理），外置 `crates/infra/seeds/prompts/`，均 `is_system=1`。v50 后**可编辑**（CompactEditor 打开 md 文件编辑）。

**文件目录布局**（v50）：
```
~/.octopus/.sync/prompts/
├── polish/    # 润色 prompt（prompts 表 content 引用这里）
└── command/   # 命令面板 agent/ai 的 @文件名引用（action_data 以 @ 开头）
```

`llm::prompt::build_system_prompt(content) = content + EDITED_MARKER_RULE`（`[]` edited 标记规则代码常量强制拼接，用户不可见）。

其余表（hotword_sets / hotword_words / hotword_hits / fuzzy_dialect_rules / clipboard_favorites / vault_meta / vault_ciphers / vault_folders / action_bar_items / script_runs / agent_adapters / agent_tasks / launcher_index / search_frequency / recordings / recording_thumbnails）见各功能域文档：[clipboard.md](./clipboard.md) / [vault.md](./vault.md) / [architecture.md](../architecture.md)（热词 / Action Bar / 录屏章节）。

---

## 4. 非阻塞 DB 写入（actor 模式）

`crates/desktop/src/core/db_queue.rs`——ASR 识别结果的 DB 写入 actor。

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

**后台线程**：`DB_SENDER: OnceLock` 懒初始化 spawn；`recv_timeout` 轮询 `DB_SHUTDOWN`；mpsc FIFO 保证同 id 的 `Insert` 必在 `UpdateRaw` 之前被消费。

**关机优雅 drain**（`shutdown_db`）：置 `DB_SHUTDOWN` → 后台线程排空 `try_iter()` 后退出；`main.rs` 挂到 `tauri::RunEvent::ExitRequested`。

---

## 5. 过程增量入库

voice 条目入库时机分散到识别过程各事件（首次文本 `Insert` → 分段/partial `UpdateTextSegments` → 润色 `UpdatePolished` → 停止 `Finalize` 含 `duration_ms`），每次同步写 `segments`（真相源）+ `content`（扁平）+ `meta_info`（json_set 更新 char_count/polished/duration_ms）。DB 失败仅 `warn` 不阻塞识别（best-effort）。

---

## 6. AppConfig

`infra::config::AppConfig`（`octopus_infra::config::load_config()` → `db::load_app_config()`）——应用配置统一 schema，**45 字段**。完整字段表见 [configuration.md](../configuration.md)。

> **模型激活字段已移除（2026-07-17）**：`asr_engine` / `polish_llm` / `ocr_model` / `translate_engine` 4 字段已删除，激活态存 DB `models.is_enabled`。

主要字段（节选，全表见 configuration.md）：

| 字段 | 默认 | 说明 |
|------|------|------|
| `microphone` | `""` | 麦克风设备名（空=系统默认） |
| `ui_language` | `zh-CN` | 界面语言（zh-CN / en） |
| `polish_mode` | 0 | 0 关闭 / 1 仅最终 / 2 中间+最终 |
| `paste_method` | `clipboard` | `clipboard` / `direct` / `none` |
| `asr_correct` | `true` | 拼音纠错（2026-08-01 默认改 true） |
| `denoise_mode` | 1 | 0 关 / 1 RNNoise / 2 DeepFilterNet3 |
| `hide_toolbar` | ⚠️ 不一致 | schema seed `false`（全新库生效）vs config.rs default fn `true`（缺键时生效）——见 audit backlog |
| `clipboard_theme` | `light` | 剪贴板浮窗主题 |
| `action_bar_search_engine` | `google` | Action Bar 默认搜索引擎 |
| `record_shortcut` | `CmdOrCtrl+Shift+R` | 录屏 toggle（仅 macOS） |
| `vault_autotype_shortcut` | `CmdOrCtrl+Shift+S` | 密码箱 Auto-Type |
| `vault_lock_timeout_secs` | 180 | 密码箱自动锁定（秒） |
| `onboarding_completed` | `false` | 首次引导是否完成 |
| `terminal_font_size` / `terminal_font_family` | 13 / Menlo | 终端字体偏好 |
| ~~`image_preview_auto_ocr`~~ | `true` | **已无消费方（死字段）**——2026-08-14 ImagePreview OCR 改手动三态后不再读取，保留仅为 DB 兼容 |
| 快捷键们 | — | `asr_shortcut`（单键名）/ `clipboard_shortcut` / `screenshot_shortcut` / `edit_shortcut` / `edit_global_shortcut` / `action_bar_shortcut` / `paste_stack_shortcut` |

### 环境变量系统（v22 新增，`category='env'`）

`app_config` 表 `category='env'` 分组，与普通配置同表隔离。3 个内置变量：`huggingface`（默认 `https://hf-mirror.com`）/ `modeloscope` / `github`（key 均不可改），用户可自定义任意 key-value。

**模板替换规则**：ASR 模型下载 URL 中的 `{huggingface}`/`{modeloscope}` 等占位符在下载时替换。旧的 `download_mirror` 已废弃，启动时自动迁移到 `env.huggingface`。

---

## 7. SharedRuntimeConfig

`type SharedRuntimeConfig = Arc<RwLock<AppConfig>>`（挂 `tauri::State`）——**完整 `AppConfig` 的唯一真相源**，取代旧 `RuntimeConfig` 部分镜像（消除字段同步遗漏）。

工具栏可运行时切换（无需重启）：`polish_mode` / `denoise_mode`（**模型激活**走统一 `switch_active_model(domain, id)` 命令）。

Tauri 命令：`toolbar_state` / `switch_active_model`（统一激活命令：DB 单语句刷新 `is_enabled` → `reload_active_engine` 重载缓存）/ `set_polish_mode` / `set_denoise_mode` / `polish_now` 等。

读写共享 `AppConfig`（即时生效）+ `persist_*` best-effort 持久化回 DB。

---

## 8. 配置持久化

| 方式 | 语义 |
|------|------|
| `persist_*`（单键 `save_config_key`） | ON CONFLICT 仅改 config_value |
| `set_config`（全量 `save_app_config`） | 45 字段 serde 自动遍历 ON CONFLICT，包 `unchecked_transaction`（原子） |

均写 DB。旧 `write_config_yaml` / `model.json` / `history.txt` / `record.txt` 已移除——DB 是唯一配置/存储源。

---

## 9. 运行时文件布局

```
~/.octopus/
├── octopus.db          # 嵌入式 SQLite（22 表，唯一存储）
├── config.yaml.bak     # 旧 config.yaml 迁移后的备份（可安全删除）
└── models/
    ├── vad.onnx        # VAD 磁盘覆盖（可选——通用名放任意 VAD 模型覆盖内嵌 silero_vad_v6；不存在用编译期内嵌）
    ├── zipformer/      # 默认 ASR（27M，随包）
    └── <HF repo>/      # cli download 下的大模型

~/Documents/octopus/screens/   # 截图原图（JPEG q100，MD5 文件名）
~/.cache/huggingface/hub/      # 旧 hf-cli 大模型缓存（兼容：resolve 第 4 级仍查此处）
```
