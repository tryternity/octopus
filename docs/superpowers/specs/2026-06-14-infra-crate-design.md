# infra crate 设计（跨 crate 共享基础设施）

> 状态：✅ 已实现（2026-06-14）。

## 背景

DB 单一源重构（[db-single-source](2026-06-14-db-single-source-design.md)）之后，路径常量与 `~/.octopus` 路径解析散落多个 crate：

- `handy_home()` 在三处独立实现，行为需各自维护一致：
  - `asr/config.rs`（`Lazy<PathBuf>` 缓存）
  - `dlp/main.rs`（每次解析环境变量）
  - `llm/examples/test_polish.rs`（每次解析，命名 `octopus_home`）
- 路径字符串硬编码在调用点：`"models/silero_vad_v4.onnx"`、`"models/zipformer"`、`"VOICE_POLISH.md"`，调整需多处搜索。

## 目标

1. 新增 `infra` crate 作为**最底层基础设施层**（无项目内依赖，可被任意项目 crate 依赖）
2. 收敛固定路径常量到单一文件（开发时一处调整）
3. 统一 `~/.octopus` 路径解析，消除三处重复定义

## 设计决策

### 定位与依赖约束

- `infra` 是依赖图的**底端**：不依赖任何项目 crate（asr / llm / desktop / ...）；任何项目 crate 都可依赖它。
- 当前依赖图：`infra ← {asr, llm, dlp}`，`asr ← {cli, server, desktop}`，`llm ← desktop`。
- 仅依赖外部 crate：`once_cell`（Lazy 缓存路径）。

### 模块结构

```
crates/infra/
├── Cargo.toml          # name = "octopus-infra"
└── src/
    ├── lib.rs          # 模块声明 + pub use re-export
    ├── consts.rs       # 固定路径常量
    └── paths.rs        # octopus_config_home() 路径工具
```

### consts.rs —— 固定路径常量

| 常量 | 值 | 用途 |
|---|---|---|
| `SILERO_VAD_PATH` | `"models/silero_vad_v4.onnx"` | VAD 模型相对路径（`find_silero_vad` 固定加载，随应用打包） |
| `DEFAULT_ASR_MODEL_DIR` | `"models/zipformer"` | 默认 ASR 模型目录（seed zipformer-small-ctc，随应用打包） |
| `VOICE_POLISH_FILE` | `"VOICE_POLISH.md"` | 润色 system prompt 外部覆盖文件名（desktop 启动读取） |

均为相对路径字符串，使用时与 `octopus_config_home()` join 成绝对路径。

### paths.rs —— octopus_config_home()

```rust
static OCTOPUS_HOME: Lazy<PathBuf> = Lazy::new(|| {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".octopus")
});

pub fn octopus_config_home() -> &'static Path {
    OCTOPUS_HOME.as_path()
}
```

- `Lazy<PathBuf>`：进程内首次调用后固定，避免每次解析环境变量。
- 返回 `&'static Path`：可直接 join 构造路径，无需 `PathBuf` 拷贝。
- 原名 `handy_home()` → 改名 `octopus_config_home()`，语义更明确（指向配置根目录 `~/.octopus`）。

### root re-export

`lib.rs` 中 `pub use paths::octopus_config_home;`，调用点用 `octopus_infra::octopus_config_home()`（高频函数免 `paths::` 前缀）；`consts` 保留模块前缀（分组名有语义：VAD / ASR / 润色）。

## 迁移影响

| crate | 改动 |
|---|---|
| asr | 删 `handy_home()` + `static HANDY_HOME`；config.rs / db.rs 改 `octopus_config_home()`；引入 `SILERO_VAD_PATH` / `DEFAULT_ASR_MODEL_DIR` |
| dlp | 删自建 `handy_home()`；3 处改 infra |
| llm | 删 `VOICE_POLISH_FILE` 定义（移入 infra）；example 删 `octopus_home()` 改 infra |
| desktop | config.rs / main.rs 改 infra；main.rs 用 `VOICE_POLISH_FILE` |
| cli | 2 处改 infra |
| 全部 | `Cargo.toml` 加 `octopus-infra = { path = "../infra" }` |

共消除 3 处 `handy_home()` 重复定义，6 个 crate 统一到 `octopus_infra::octopus_config_home()`。

## 未来扩展

infra 作为基础层，后续可下沉（当前均未迁移，待多 crate 复用时再动）：

- **时间工具**（中优先级）：`asr/db.rs` 的 `now_string()` / `days_to_ymd()` / `is_leap()` 目前仅 asr 用，无重复，暂留原处。
- 其他跨 crate 共享的纯基础操作（无业务逻辑）。

## 关键约束

- infra **不得引入项目内依赖**（保持底端纯净），否则破坏依赖图。
- infra **不放业务逻辑 / 配置 schema**（如 `DesktopConfig` 属 app 层；DB schema 属 asr 层），只放无业务语义的基础工具。
