# 内置模型开箱即用设计（source_type 统一 + VAD 内嵌 + builtin 自动下载）

> **日期**：2026-07-22
> **状态**：✅ 已实现（Step 1 VAD 内嵌 + Step 2 source_type + Step 3 builtin 下载，2026-07-22）

---

## 0. 目标

1. **models 表 `is_local` 改为 `source_type`**——统一三种来源：builtin(0) / local(1) / cloud(2) ✅
2. **VAD 内嵌**——1.7MB `include_bytes!` 编译进二进制，从内存加载，不落盘 ✅
3. **builtin 模型自动下载**——zipformer-small（27MB）首次启动检测缺失 → 下载页 → 用户点「下载并进入系统」→ 全部完成后自动进入系统 ✅

## 1. source_type 枚举

### 1.1 定义

```sql
source_type INTEGER NOT NULL DEFAULT 1  -- 0=builtin(内置) 1=local(用户下载) 2=cloud(云端)
```

| source_type | 含义 | secret_key | is_available | 例子 |
|---|---|---|---|---|
| 0 (builtin) | 应用内置，开箱即用 | manifest JSON（下载清单） | 启动时检测本地文件 | VAD、zipformer CTC（兜底 ASR） |
| 1 (local) | 本地模型，用户主动下载 | manifest JSON | 下载后置 1 | whisper、paraformer、moonshine 等 |
| 2 (cloud) | 云端模型，API 调用 | API Key | 用户配置后置 1 | 阿里云 ASR、OpenAI LLM 等 |

### 1.2 与旧 `is_local` 的映射

| 旧 is_local | 新 source_type | 说明 |
|---|---|---|
| 1 (true) | 1 (local) | 绝大多数本地模型 |
| 0 (false) | 2 (cloud) | 所有云端模型 |
| — | 0 (builtin) | **新增**——VAD + zipformer CTC 兜底引擎 |

### 1.3 兜底引擎入 DB

当前兜底 zipformer CTC（`zipformer-small`）不在 DB 里——代码硬编码。改为 **DB 里加 source_type=0 的行**：

```sql
INSERT OR IGNORE INTO models (domain, provider, category, model_name, source, language, description, source_type, secret_key, is_available, is_streaming)
VALUES ('asr', 'local', 'zipformer', 'zipformer-small', 'asr/zipformer-small', 'zh',
        'zipformer-small 兜底引擎（27M，内置，开箱即用）', 0, '<manifest>', 1, 1);
```

secret_key（manifest）由 `fill_manifests` 从 model_manifests.rs 填充——需新增 `zipformer-small` 的 manifest 常量（指向 HF repo `csukuangfj/sherpa-onnx-streaming-zipformer-small-zh-int8-2025-04-01`）。

VAD **不进 DB**（保持现状——VAD 是固定路径/内嵌，不属于 models 表的引擎管理范畴）。

---

## 2. VAD 内嵌

### 2.1 模型文件

`crates/asr-local/models/silero_vad_v4.onnx`（1.7MB）。

### 2.2 VadSource enum

```rust
pub enum VadSource {
    File(PathBuf),  // 磁盘上的文件（用户自定义覆盖）
    Builtin,        // 内嵌字节
}
```

`find_silero_vad() -> Result<VadSource>`：磁盘存在 → File，否则 → Builtin。

### 2.3 SileroVad::new_builtin()

`include_bytes!` + ort `commit_from_memory`。不落盘。缓存 key `builtin://silero_vad_v4`。

### 2.4 调用点改造（12 处）

全部改为 match VadSource。

---

## 3. builtin 模型自动下载

### 3.1 流程

1. app 启动 → `ensure_db()` 之后
   - `ensure_db` 内调 `ensure_builtin_seed()`：幂等 `INSERT OR IGNORE` builtin 兜底引擎行 + `fill_manifests`（每次启动跑，防止历史库迁移时漏注入）
2. **完整性校验 + is_available 同步**（`sync_builtin_models_availability`，preheat 之前）：
   - 查 DB `WHERE source_type=0` 的模型（`list_builtin_models()`）
   - 逐个校验：`resolve_model_dir` 命中 + manifest 所有文件 sha256 通过（`verify_against_manifest`）→ ready
   - ready != is_available → `set_model_available` 同步
   - 结果缓存在 `OnceLock`（供 setup 阶段 check 复用，避免重复 sha256）
3. **缺失检测**（`check_builtin_models_missing`，setup 内）：读 OnceLock 缓存 → 返回缺失列表
4. 有缺失 → 显示下载页（独立 Tauri 窗口 `download_window`，不阻断主窗口创建）
5. 下载页列出缺失模型 + 大小 + 「下载并进入系统」按钮
6. 用户点「下载并进入系统」→ 并发下载 → 全部完成后自动关闭下载窗进入系统
   - 复用 `model_commands::download_model`（manifest 驱动）
   - **多文件并发**：`JoinSet` + `Semaphore(4)` 限并发，manifest 各文件并行下载
   - **增量下载**：known_broken 集合跳过完好文件（只下损坏/缺失的）
7. 下载完成 → `download_model` 内 `set_model_available(name, true)` → emit `download-done` 通知前端刷新

**模型管理页额外行为**：
- 校验（verify）失败 → 自动触发下载修复
- 激活（activate）前先校验完整性，损坏/缺失 → 自动下载修复后才激活
- **DownloadPopover 浮层**（hover 文件按钮）：展示模型所有文件的列表 + 文件级进度
  - `list_model_files` 命令返回 `[{path, size, exists}]`（exists = sha256 校验通过）
  - 监听 `download-progress`（文件级，带 `file` 字段）+ `download-file`（状态 start/done/error/skip）
  - 已存在文件显示 100% 绿勾，下载中显示进度条，待下载显示文件大小

**下载事件设计**（并发场景）：
- `download-progress`：`{repo, file, downloaded, total, speed}`——按文件区分进度
- `download-file`：`{repo, file, status}`——status = start/done/error/skip
- `download-done`：`{repo, already_ready?, error?}`——整个模型下载完成/失败

### 3.2 下载页 UI

简单页面：
- 标题：「需要下载内置模型」
- 列表：模型名 + 大小 + 状态（待下载/下载中/完成）
- 按钮：「下载并进入系统」/「稍后下载」
- 用户选「稍后下载」→ ASR 无兜底引擎，用户可后续手动下载

### 3.3 下载实现

**复用 `model_commands::download_model`**（manifest 驱动）：builtin 模型已在 DB（source_type=0 + manifest 填充），`download_model` 按 `source` 参数查 DB manifest → 逐文件 `octopus-download::Downloader` 下载 + sha256 校验 → `set_model_available`。

不新建独立下载逻辑——builtin 与 local 模型下载路径完全一致，仅 source_type 标签不同。前端下载页 `invoke("download_model", { repo: info.source })` 触发。

---

## 4. 迁移（schema v47 → v48）✅ 已实现

### 4.1 migrate_v47_to_v48 helper

迁移逻辑提取为独立函数 `migrate_v47_to_v48(conn)`，被 `init_schema` 在 **3 处**调用（v==47 库 / v==46 段末尾 / v17-v46 段末尾），确保所有老库迁移路径都能到 v48。

```sql
-- 幂等保护：检查列是否已迁移
ALTER TABLE models RENAME COLUMN is_local TO source_type;
-- 数据迁移：is_local=0 → source_type=2（cloud），is_local=1 → source_type=1（local）
UPDATE models SET source_type = CASE WHEN source_type = 0 THEN 2 ELSE 1 END;
-- 注入 builtin 兜底引擎 seed 行（source_type=0）
INSERT OR IGNORE INTO models (...) VALUES ('asr',...,'zipformer-small','asr/zipformer-small',... ,0,0,1);
-- 填充 manifest
fill_manifests(conn);
PRAGMA user_version = 48;
```

**关键约束**：v17-v46 老库走完 v46→v47 段后 `return Ok(())`，原本到不了 v48。helper 模式让 v46 段和 v17-v46 段末尾都调用 `migrate_v47_to_v48` 解决此问题。

### 4.2 ensure_builtin_seed 兜底

`ensure_db()` 每次启动调 `ensure_builtin_seed(conn)`：幂等 `INSERT OR IGNORE` builtin 行 + 检查 manifest 空则 `fill_manifests`。修复「迁移时代码不完整导致漏注入」的历史库（如本会话开发期间 DB 先迁移到 v48 但 builtin seed 代码未完成时跑过 ensure_db 的情况）。

### 4.3 db.sql 全新库

全新库直接用 `source_type` 列名 + 正确的值（builtin=0 / local=1 / cloud=2）。seed 的 ASR（13 local + 1 builtin）/ translate / ocr 本地模型 source_type=1，云端不 seed。

---

## 5. 代码改动影响面（实际 ~150 处）

### 5.1 is_local → source_type 重命名

| 位置 | 处数 | 改法 |
|---|---|---|
| `crates/infra/src/db.rs` | ~65 | SQL 语句 is_local → source_type；7 个 struct 字段 is_local: bool → source_type: i64；row mapping |
| `crates/asr-local/src/config.rs` | ~25 | EngineInfo 字段 + 排序 + fallback 字面量 + 测试 |
| `crates/desktop/src/runtime_config.rs` | ~21 | 3 个 struct (EngineOption/LlmOption/OcrOption) + engine_label + 测试 helper |
| `crates/desktop/src/config.rs` | 2 | LLM polish 热路径 `is_local` → `is_local_or_builtin()`（语义不变） |
| `crates/desktop/src/{translation_commands,action_bar_commands,settings_commands,vault_commands}.rs` | ~10 | translate 策略对称分支 + 测试 SQL |
| `crates/{translation,vault,llm/examples}` | ~5 | 构造点 + 测试 fixture |
| 前端（6 文件） | 18 | TypeScript interface + 条件判断 + 硬编码值 |

**关键语义变化**：原来 `is_local == true` 包含 builtin + local，现在拆开了。所有 `if is_local` 改为 `if is_local_or_builtin()`（语义不变）或 `if source_type == 1`（仅 local）。

### 5.2 Rust struct 改动（实际实现）

```rust
// 旧
pub struct ModelEntry {
    pub is_local: bool,
    ...
}
// 新（字段名改 + 类型改 bool → i64）
pub struct ModelEntry {
    #[serde(default = "default_local_source_type")]  // = 1，向后兼容旧 YAML/JSON
    pub source_type: i64,  // 0=builtin 1=local 2=cloud
    ...
}

impl ModelEntry {
    pub fn is_builtin(&self) -> bool { self.source_type == 0 }
    pub fn is_local(&self) -> bool { self.source_type == 1 }
    pub fn is_cloud(&self) -> bool { self.source_type == 2 }
    pub fn is_local_or_builtin(&self) -> bool { self.source_type <= 1 }
}
```

共 7 个 struct 改字段：`ModelEntry` / `CompatibleLlmConfig` / `AsrEngineRow` / `ModelRow` / `LlmModelInfo` / `OcrModelInfo`（`LocalAsrModelRow` 无 is_local 字段，无需改）。helper 仅加在 `ModelEntry`（语义入口），其他 struct 直接比较 `source_type: i64`。

---

## 6. 实施顺序 ✅ 全部完成

1. **Step 1：VAD 内嵌**（不依赖 source_type 改动，独立）✅ 其他 session 完成，merge 进本分支
2. **Step 2：source_type 重命名**（schema 迁移 + 全量 is_local → source_type）✅
3. **Step 3：zipformer builtin 入 DB**（依赖 source_type）+ 自动下载页 ✅

Step 1 与 Step 2 在 `crates/asr-local/src/config.rs` 有重叠（VAD 改 vad 相关函数，source_type 改 ModelEntry/EngineInfo），自动合并无冲突。

---

## 7. 已知风险（全部已解决）

- **~150 处改动量大** ✅ ——靠编译器报所有错误，一次性修完。实际改动分布见 §5.1
- **vault secret 加密用 is_local** ✅ ——`try_decrypt_secret_global` 本身**不查 is_local**（靠 `v1:` 前缀判定），调用方 `config.rs::llm_config_ignore_mode` 用 `is_local_or_builtin()` 判断（builtin+local 走 clone，仅 cloud 走 vault 解密）。语义正确
- **启动下载页是新 UI** ✅ ——独立 Tauri 窗口 `download_window`，capabilities/default.json 已注册，vite.config.ts 加 entry
- **迁移幂等性** ✅ ——`migrate_v47_to_v48` helper + `ensure_builtin_seed` 双保险，历史库漏注入也能补
