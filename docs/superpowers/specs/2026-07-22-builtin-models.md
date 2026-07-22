# 内置模型开箱即用设计（source_type 统一 + VAD 内嵌 + builtin 自动下载）

> **日期**：2026-07-22
> **状态**：设计阶段

---

## 0. 目标

1. **models 表 `is_local` 改为 `source_type`**——统一三种来源：builtin(0) / local(1) / cloud(2)
2. **VAD 内嵌**——1.7MB `include_bytes!` 编译进二进制，从内存加载，不落盘
3. **builtin 模型自动下载**——zipformer（25MB）首次启动检测缺失 → 下载页 → 用户点「后台下载」→ 进系统

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

当前兜底 zipformer CTC（`zipformer-small-ctc`）不在 DB 里——代码硬编码。改为 **DB 里加 source_type=0 的行**：

```sql
INSERT OR IGNORE INTO models (domain, provider, category, model_name, source, language, description, source_type, secret_key, is_available, is_streaming)
VALUES ('asr', 'local', 'zipformer', 'zipformer-small-ctc', 'models/zipformer', 'zh',
        'zipformer-small-ctc 兜底引擎（27M，内置，开箱即用）', 0, '<manifest>', 1, 1);
```

secret_key（manifest）由 `fill_manifests` 从 model_manifests.rs 填充——需新增 `zipformer-small-ctc` 的 manifest 常量（指向 HF repo `csukuangfj/sherpa-onnx-streaming-zipformer-small-ctc-zh-int8-2025-04-01`）。

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
2. 查 DB `WHERE source_type=0` 的模型
3. 逐个检查本地文件是否存在（`resolve_model_dir(source)` 命中？）
4. 有缺失 → 显示下载页（Tauri 窗口 or 前端路由）
5. 下载页列出缺失模型 + 大小 + 「后台下载」按钮
6. 用户点「后台下载」→ 关闭下载页 → 进系统 → 后台 `spawn` 下载
7. 下载完成 → `set_model_available(name, true)` → emit 事件通知前端刷新

### 3.2 下载页 UI

简单页面：
- 标题：「需要下载内置模型」
- 列表：模型名 + 大小 + 状态（待下载/下载中/完成）
- 按钮：「后台下载并进入系统」/「稍后下载」
- 用户选「稍后下载」→ ASR 无兜底引擎，用户可后续手动下载

### 3.3 下载实现

复用 `octopus-download`（HfRequest + resolve_tasks + Downloader），与 CLI download 子命令同模式。

---

## 4. 迁移（schema v47 → v48）

### 4.1 ALTER TABLE

SQLite 支持 `ALTER TABLE ... RENAME COLUMN`（3.25.0+，rusqlite bundled 版本满足）：

```sql
ALTER TABLE models RENAME COLUMN is_local TO source_type;
-- 数据迁移：is_local=0 → source_type=2（cloud），is_local=1 → source_type=1（local）
UPDATE models SET source_type = source_type * 2;  -- 0→0... 不对，1→2 不对
```

等等，映射是 `0→2, 1→1`，不能简单乘。用 CASE：
```sql
UPDATE models SET source_type = CASE WHEN source_type = 0 THEN 2 ELSE 1 END;
```

然后 INSERT 新的 builtin 行（zipformer-small-ctc）。

### 4.2 db.sql 全新库

全新库直接用 `source_type` 列名 + 正确的值（builtin=0 / local=1 / cloud=2）。seed 的 ASR/translate/ocr 本地模型 source_type=1，云端不 seed。

---

## 5. 代码改动影响面

### 5.1 is_local → source_type 重命名（~170 处）

| 位置 | 处数 | 改法 |
|---|---|---|
| `crates/infra/src/db.rs` | 65 | SQL 语句 is_local → source_type；struct 字段 is_local: bool → source_type: i64；row mapping |
| `crates/asr-local/src/config.rs` | 22 | is_local 判断改为 source_type == 1 |
| `crates/desktop/src/runtime_config.rs` | 21 | 同上 |
| `crates/desktop/src/vault_*.rs` | 15 | vault secret 加密用 is_local 判断（try_decrypt_secret_global） |
| 其他 Rust | ~29 | 各处 is_local 引用 |
| 前端 | 18 | TypeScript interface + 条件判断 |

**关键语义变化**：原来 `is_local == true` 包含 builtin + local，现在拆开了。所有 `if is_local` 要改为 `if source_type != 2`（非云端）或 `if source_type == 1`（仅 local），取决于语义。

### 5.2 Rust struct 改动

```rust
// 旧
pub struct Model {
    pub is_local: bool,
    ...
}
// 新
pub struct Model {
    pub source_type: i64,  // 0=builtin 1=local 2=cloud
    ...
}
```

或加一个 helper：
```rust
impl Model {
    pub fn is_builtin(&self) -> bool { self.source_type == 0 }
    pub fn is_local(&self) -> bool { self.source_type == 1 }
    pub fn is_cloud(&self) -> bool { self.source_type == 2 }
    pub fn is_local_or_builtin(&self) -> bool { self.source_type <= 1 }
}
```

这样旧代码 `model.is_local` 改为 `model.is_local_or_builtin()`（语义不变），或 `model.is_local()`（仅 local，更精确）。

---

## 6. 实施顺序

1. **Step 1：VAD 内嵌**（不依赖 source_type 改动，独立）
2. **Step 2：source_type 重命名**（schema 迁移 + 全量 is_local → source_type）
3. **Step 3：zipformer builtin 入 DB**（依赖 source_type）+ 自动下载页

每步 e2e 通过后再做下一步。

---

## 7. 已知风险

- **170 处改动量大**——is_local → source_type 是全局重命名，漏一处就编译错误（好在编译器会报所有错误）
- **vault secret 加密用 is_local**——`try_decrypt_secret_global` 判断 `is_local=0` 的云端模型 secret_key 不加密。改 source_type 后要确保 builtin(0) 和 local(1) 的 secret_key 都加密，cloud(2) 不加密
- **启动下载页是新 UI**——需要新建 Tauri 窗口或前端路由，涉及 capabilities 注册
