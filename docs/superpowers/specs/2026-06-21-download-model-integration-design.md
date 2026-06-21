# octopus-download 接入模型管理（阶段1）设计

> 2026-06-21。4 阶段体积优化工程的第 1 阶段。整体方案见会话记录；本 spec 只覆盖阶段1。
> 相关：download crate spec `2026-06-21-model-download-design.md`、TLS/体积分析 `docs/download_architure.md`。

## 1. 背景与定位

`octopus-download` crate（main `a2bef60`）已完成：通用 HF 模型下载器（分块并发 + 断点续传 sidecar + SHA256 校验 + 镜像 fallback + HF 适配层 `api`/`glob`/`resolve`），替代 `huggingface-cli` 解三个终端痛点（装 Python、国内镜像切换、整库下载）。但 crate 目前**未接入任何消费者**——模型下载仍走 hf-cli，落到 `~/.cache/huggingface/hub/`。

本阶段把 download crate 接入模型管理：模型下到 `~/.octopus/models/<repo>/`，ASR 的 `resolve_model_dir` 能发现并加载。

这是 4 阶段工程的第 1 阶段：

| 阶段 | 内容 | 本 spec |
|---|---|---|
| **① download 接模型管理** | 替换 hf-cli；模型下到 `~/.octopus/models/`；resolve 扩展；cli `download` 子命令 | ✓ |
| ② ort load-dynamic | asr 不再静态含 ort，运行期 dlopen | 后续 |
| ③ download 拉 ort 运行时 | download 增加拉 `libonnxruntime` 能力 | 后续 |
| ④ 分发打包 | 三 binary 共享 `~/.octopus/bin/libonnxruntime` | 后续 |

阶段1 与 ②③④ 正交，可独立交付。

## 2. 现状（已探明）

### 2.1 resolve_model_dir
`crates/asr/src/config.rs:65`，三级查找：
1. `octopus_config_home().join(source)`（`~/.octopus/` 下相对路径，随包小模型如 silero_vad）
2. 绝对路径（`source` 是绝对路径且存在）
3. `find_hf_cache(source)`：`~/.cache/huggingface/hub/models--<repo>/snapshots/<hash>/`

### 2.2 调用点
- **13+ 处**引擎调 `resolve_model_dir(&entry.source)`：whisper / sensevoice / paraformer / streaming_paraformer / streaming_engine / streaming_zipformer / moonshine / qwen3_asr / zipformer / engine，以及 cli（×5）。
- **3 处绕过 resolve_model_dir 直接拼 `.cache/huggingface/hub`**（重复路径逻辑，异味）：
  - `streaming_paraformer.rs:797`
  - `zipformer.rs:1297`
  - `streaming_zipformer.rs:912`

### 2.3 目录约定
`infra/consts.rs` 已用 `~/.octopus/models/` 根：
- `SILERO_VAD_PATH = "models/silero_vad_v4.onnx"`
- `DEFAULT_ASR_MODEL_DIR = "models/zipformer"`

### 2.4 download crate 接口（复用，不改）
```rust
HfRequest { repo, include, exclude, source_url: Option<String>, target_dir: PathBuf }
resolve_tasks(&reqwest::Client, HfRequest) -> Result<Vec<DownloadTask>>
Downloader::new(DownloadConfig) -> Result<Downloader>
Downloader::download(&DownloadTask, mpsc::Sender<Progress>, Option<...>) -> Result
```
布局：`target_dir/<repo>/<files>`（integration 测试验证：`target_dir=dir`, `repo="org/m"` → `dir/org/m/model.onnx`）。

## 3. 设计

### 3.1 resolve_model_dir 扩展（config.rs:65）
在 HF cache 查找（原第 3 级）之前插入新一级：

```
1. ~/.octopus/<source>          （既有，随包小模型）
2. 绝对路径                      （既有）
3. ~/.octopus/models/<source>   （新增：download 下的 HF 模型）★
4. find_hf_cache(source)        （既有，兼容已用 hf-cli 下的模型）
```

**纯查找语义不变**——resolve 不发起网络请求、不下载，只查本地路径是否存在。新级放在 HF cache 之前，使 download 下的模型优先于旧 hf-cli 缓存。

### 3.2 统一 3 处直接拼路径
`streaming_paraformer.rs:797` / `zipformer.rs:1297` / `streaming_zipformer.rs:912` 直接 `join(".cache/huggingface/hub")` 的，改为调 `resolve_model_dir`。收拢重复路径逻辑，且自动享受 3.1 的新查找级。

> 实施时逐处确认它们解析的 source 与 resolve_model_dir 入参语义一致；若不一致（如用了不同形式的 repo 名），保留原逻辑并加注释说明原因。

### 3.3 显式下载，resolve 不透明触发（关键决策）
模型缺失时 resolve **报错**，错误信息提示：
> 模型 `<source>` 未在 `~/.octopus/models/` 或 HF cache 找到，请运行 `octopus-cli download <source>` 下载。

**不自动下载**。理由：
- resolve 保持纯查找语义（快、确定、本地 IO）；ASR 引擎加载时不会突然联网 / 卡住 / 因网络失败。
- 下载是显式、可观测、有进度的动作（cli 子命令 / 未来 GUI 按钮），符合 download crate 设计 + hf-cli 使用习惯。
- 错误边界清晰：resolve 失败 = 模型缺失；download 失败 = 网络/hash/镜像问题，两类不混淆。

### 3.4 cli 加 download 子命令
`crates/cli/src/main.rs` 的 `Commands` enum 加：
```rust
Download {
    /// HF repo，如 Systran/faster-whisper-large-v3（与 config.yaml 的 entry.source 一致）
    repo: String,
    /// 只下匹配的文件（glob，对齐 hf-cli，* 跨 /）。空 = 下整库
    #[arg(long)]
    include: Vec<String>,
    /// 排除匹配的文件
    #[arg(long)]
    exclude: Vec<String>,
    /// HF 镜像，如 hf-mirror.com。覆盖 config 默认
    #[arg(long)]
    mirror: Option<String>,
}
```
行为：薄封装 download crate——构 `HfRequest`（`target_dir=~/.octopus/models`，`repo`/`source_url` 由参数+config）→ `resolve_tasks` → 逐任务 `Downloader::download` → 打印进度 → 汇总结果。

- `crates/cli/Cargo.toml` 加 `octopus-download = { path = "../download" }`。
- download 是 async；cli 当前同步 main（仅 TranscribeUrl 用 tokio runtime）。沿用既有模式：Download 子命令建 `tokio::runtime::Runtime` + `block_on`。
- **include 默认行为**：`include` 空时下整库。实施时验证 `resolve_tasks` 对空 `include` 的语义（空 = 匹配全部 siblings），若不符则在 cli 层空时传通配 `*`。

### 3.5 镜像配置
优先级：`--mirror` 参数 > config.yaml 配置 > 默认 `huggingface.co`。
- config.yaml 新增可选字段（infra `Config`），如 `download.mirror: hf-mirror.com`（字段名实施时对齐 Config 结构）。
- 解国内"每次切镜像"痛点：配一次，所有 download 复用；`--mirror` 临时覆盖。

### 3.6 目录布局
- download 的 `target_dir = ~/.octopus/models`（`octopus_config_home().join("models")`）。
- repo 作子目录：`~/.octopus/models/<repo>/<files>`，如 `~/.octopus/models/Systran/faster-whisper-large-v3/model.onnx`。
- 与 resolve_model_dir 第 3 级（`~/.octopus/models/<source>`）一致，与既有 silero/zipformer 同根。

## 4. 接口契约

| 接口 | 变化 |
|---|---|
| `resolve_model_dir(source)` 签名 | **不变**（`&str -> Result<PathBuf>`），内部加 1 级查找 |
| cli `Commands` enum | 新增 `Download` 变体 |
| download crate lib 接口 | **不改**（复用 `HfRequest`/`resolve_tasks`/`Downloader`） |
| config.yaml | 新增可选 `download.mirror` |

## 5. 数据流

**下载**：`octopus-cli download <repo>` → 构 `HfRequest` → `resolve_tasks`（HF api 解析 siblings + glob 过滤 + 拼 resolve URL + 镜像）→ `Downloader::download`（probe/分块/并发/校验/rename/镜像 fallback）→ `~/.octopus/models/<repo>/`。

**加载**：ASR 引擎 `resolve_model_dir(&entry.source)` → 查 `~/.octopus/models/<source>` 命中 → 返回目录 → 引擎加载 onnx/tokenizer。

## 6. 错误处理

| 场景 | 行为 |
|---|---|
| resolve 模型缺失 | 报错 + 提示 `octopus-cli download <source>` |
| download 网络/镜像失败 | download crate 已有错误（`DownloadError`），cli 透传 + 镜像 fallback |
| hash 校验失败 | download crate 整文件重下（既有逻辑） |
| target_dir 不可写 | anyhow 透传 |

## 7. 范围边界（本阶段不做）

- **不加 DB models 表**：resolve 查文件系统即可；未来 GUI 模型管理页要列表/状态时再加（YAGNI）。
- **不动 ort**：阶段② 的 load-dynamic。
- **不做 GUI**：lib-first，desktop 消费（setting-ui2 若复活）留后续。
- **不删 HF cache 兼容**：resolve 第 4 级仍查 `~/.cache/huggingface`，兼容用户已用 hf-cli 下的模型。

## 8. 测试策略

- **resolve_model_dir 单测**（asr/config.rs 或 tests）：
  - `~/.octopus/models/<source>` 命中 → 返回该路径
  - 不在 models/ 但在 HF cache → fallback 返回 HF cache 路径
  - 都不在 → Err，信息含下载提示
- **cli download 集成测试**（httpmock，复用 download crate tests 模式）：
  - 单文件 resolve + download 成功
  - include glob 过滤
  - mirror fallback
- **引擎回归**：3 处路径统一后，跑现有 asr 引擎测试确认无回归。

## 9. 后续阶段（不属于本 spec）

- **② ort load-dynamic**：`asr/Cargo.toml` 的 ort 从 `download-binaries` 改 `load-dynamic`，初始化指向 dylib 路径（`~/.octopus/bin/`）。binary 各掉 ~20-35M 静态 ort。
- **③ download 拉 ort 运行时**：download 增加拉 `libonnxruntime` 能力（版本对齐 ort 2.0.0-rc.12、平台包 mac-universal2/linux-x64-gpu/win-x64、镜像 fallback）→ `~/.octopus/bin/`。
- **④ 分发打包**：三 binary 共享 `~/.octopus/bin/libonnxruntime`；发行包不含静态 ort；首次运行按需拉取。
