# 设计文档：LLM 润色模式三档化（polish_mode）

> 将 `polish_enabled: bool` + `polish_interval` 的隐式三态收敛为显式枚举 `polish_mode: PolishMode`（0/1/2）；底层润色引擎与流式/伪流式共用路径不变。

## 0. 背景

现状用两个配置项隐式表达三种润色行为：

| 现状配置 | 行为 |
|---|---|
| `polish_enabled: false` | 完全不润色 |
| `polish_enabled: true` + `polish_interval <= 0` | 仅最终润色 |
| `polish_enabled: true` + `polish_interval > 0` | 中间润色 + 最终润色 |

**问题：**

1. 三档语义隐藏在「bool + interval 组合」里，不直观——`interval<=0` 表示「仅最终润色」是个隐式约定，必须靠文档专门解释，用户配置时易困惑。
2. `polish_interval` 职责混叠：既当「是否做中间润色」的开关（`<=0`），又当中间润色的节流间隔。

**底层润色逻辑已正确**（本次不动）：

- 流式与伪流式**共用** `check_and_trigger_polish`（`coordinator.rs:913`），流式 tick（`:1019`）与伪流式 tick（`:766`）都调它——「伪流式与流式润色逻辑一致」已是现状。
- 节流条件 `elapsed >= polish_interval` 且 `新增字符数 > polish_base_len`——「累加到下次，避免嗯、啊频繁触发空润色」已是现状。

## 1. 目标

1. 将隐式三态收敛为**显式枚举** `polish_mode: PolishMode`（0/1/2），YAML 加注释说明每档含义。
2. `polish_interval` 退回纯粹的节流参数，**仅模式 2 生效**。
3. 底层润色触发逻辑、流式/伪流式共用路径**原样保留**。
4. 直接替换字段（删 `polish_enabled`），项目早期接受一次性 breaking change。

## 2. PolishMode 枚举设计

定义在 `infra/src/config.rs`（与 `AppConfig` 同模块；desktop 经 `octopus_infra::config::PolishMode` 引用）：

```rust
/// LLM 润色模式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PolishMode {
    /// 0 — 完全不润色（默认）
    #[default]
    Disabled,
    /// 1 — 仅最终润色（识别结束后润色一次）
    FinalOnly,
    /// 2 — 中间润色 + 最终润色
    Intermediate,
}
```

**反序列化**：自定义 `Deserialize` impl，YAML 写整数 0/1/2。不引入 `serde_repr` 依赖（config.yaml 只读不写，只需 `Deserialize`）。非法值 `log::warn` + 回退 `Disabled`：

```rust
impl<'de> serde::Deserialize<'de> for PolishMode {
    fn deserialize<D>(d: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let n = u8::deserialize(d)?;
        Ok(match n {
            0 => PolishMode::Disabled,
            1 => PolishMode::FinalOnly,
            2 => PolishMode::Intermediate,
            other => {
                log::warn!("polish_mode={} 非法（应为 0/1/2），回退 0(Disabled)", other);
                PolishMode::Disabled
            }
        })
    }
}
```

`AppConfig` 字段：

```rust
/// 润色模式：0=关闭 / 1=仅最终润色 / 2=中间润色+最终润色
#[serde(default)]
pub polish_mode: PolishMode,

/// 中间润色最小间隔（秒），仅 polish_mode=2 生效
#[serde(default = "default_polish_interval")]
pub polish_interval: f64,
```

`Default` impl：`polish_enabled: false` → `polish_mode: PolishMode::default()`（即 `Disabled`）。

## 3. 判断点改造（3 处）

### 3.1 最终润色开关（`desktop/src/config.rs` `llm_config`）

模式 1、2 都启用最终润色；仅模式 0 关闭：

```rust
// 现状
if !cfg.polish_enabled || cfg.llm_secret_key.is_empty() {
    return None;
}
// 改造后
if cfg.polish_mode == octopus_infra::config::PolishMode::Disabled
    || cfg.llm_secret_key.is_empty()
{
    return None;
}
```

### 3.2 中间润色开关（`coordinator.rs` `check_and_trigger_polish`）

**删掉 `interval <= 0` 判断**——是否做中间润色改由 `polish_mode` 决定。`polish_interval` 退回纯节流参数。模式 2 下 `interval <= 0` 用下限 clamp（避免每 tick 刷爆 LLM）：

```rust
// 现状
if !config.polish_enabled
    || config.polish_interval <= 0.0
    || *polish_pending
    || accumulated_text.is_empty()
{
    return;
}
let elapsed = last_polish_time.elapsed().as_secs_f64();
if elapsed < config.polish_interval {
    return;
}

// 改造后
use octopus_infra::config::PolishMode;
if config.polish_mode != PolishMode::Intermediate
    || *polish_pending
    || accumulated_text.is_empty()
{
    return;
}
let elapsed = last_polish_time.elapsed().as_secs_f64();
// 模式 2 下 interval<=0 → 用下限 1.0s，避免每 tick 触发刷爆 LLM
let effective_interval = config.polish_interval.max(MIN_POLISH_INTERVAL_SEC);
if elapsed < effective_interval {
    return;
}
```

新增常量 `const MIN_POLISH_INTERVAL_SEC: f64 = 1.0;`（与同文件其他常量如 `VAD_SPEECH_THRESHOLD` 并列）。

> 下方 `current_len <= *polish_base_len`（新增字符数检测）判断**原样保留**——本次只替换上方的模式/节流 guard，增量检测逻辑不动。

### 3.3 启动校验（`desktop/src/main.rs`）

```rust
// 现状
if config.polish_enabled {
    if config.llm_secret_key.is_empty() {
        log::warn!("polish_enabled=true 但 llm_secret_key 为空，润色功能将不生效");
    } else {
        log::info!("润色已启用: provider={}, model={}, interval={}s", ...);
    }
}

// 改造后
use octopus_infra::config::PolishMode;
match config.polish_mode {
    PolishMode::Disabled => {}
    PolishMode::FinalOnly => {
        if config.llm_secret_key.is_empty() {
            log::warn!("polish_mode=1 但 llm_secret_key 为空，润色不生效");
        } else {
            log::info!("润色模式: 仅最终润色 (provider={}, model={})",
                config.llm_provider, config.llm_model);
        }
    }
    PolishMode::Intermediate => {
        if config.polish_interval <= 0.0 {
            log::warn!("polish_mode=2 但 polish_interval={}<=0，将使用下限 1.0s",
                config.polish_interval);
        }
        if config.llm_secret_key.is_empty() {
            log::warn!("polish_mode=2 但 llm_secret_key 为空，润色不生效");
        } else {
            log::info!("润色模式: 中间+最终 (interval={}s, provider={}, model={})",
                config.polish_interval, config.llm_provider, config.llm_model);
        }
    }
}
```

## 4. polish_interval 语义

| `polish_mode` | `polish_interval` | 行为 |
|---|---|---|
| 0 `Disabled` | 任意 | 忽略，不润色 |
| 1 `FinalOnly` | 任意 | 忽略，仅最终润色 |
| 2 `Intermediate` | `> 0` | 中间润色节流间隔（秒） |
| 2 `Intermediate` | `<= 0` | warn + 使用下限 `1.0s` |

## 5. 流式 / 伪流式润色（不变，确认现状）

- 两者共用 `check_and_trigger_polish`（`coordinator.rs:913`）。
- 流式 tick（`:1019`）与伪流式 tick（`:766`）调用点不变。
- 节流条件不变：`elapsed >= effective_interval` 且 `新增字符数 > polish_base_len`。

模式 2 下两种引擎模式都触发中间润色，行为完全对称——契合「伪流式与流式润色逻辑一致」。

## 6. 影响范围

| 文件 | 改动 |
|---|---|
| `crates/infra/src/config.rs` | 删 `polish_enabled: bool`；新增 `PolishMode` 枚举 + `Deserialize` impl + `polish_mode` 字段；`Default` 改 `polish_mode: PolishMode::default()`；`polish_interval` 注释更新 |
| `crates/infra/Cargo.toml` | 加 `log = "0.4"`（`PolishMode::deserialize` 非法值 warn 日志需要） |
| `crates/desktop/src/config.rs` | `llm_config()`：`!polish_enabled` → `polish_mode == Disabled` |
| `crates/desktop/src/coordinator.rs` | `check_and_trigger_polish`：`!polish_enabled \|\| interval<=0` → `polish_mode != Intermediate`；interval 用 `.max(MIN_POLISH_INTERVAL_SEC)`；新增常量 |
| `crates/desktop/src/main.rs` | 启动校验改 `match polish_mode`；模式 2 + interval<=0 warn |
| `docs/configuration.md` | `polish_enabled` 行 → `polish_mode`（0/1/2 + 注释示例）；`polish_interval` 注明仅模式 2 |
| `docs/architecture.md` | 润色段落改三档模式描述 |
| spec + plan | 新建本文档 + 实施计划 |

## 7. 向后兼容（breaking change）

`polish_enabled` 字段**直接删除替换**（用户已确认）。现有 `config.yaml` 写 `polish_enabled: true` 的用户：

- serde 遇到旧字段名 `polish_enabled` → 该字段在 `AppConfig` 已不存在，serde 默认**忽略未知字段**（不报错）。
- `polish_mode` 未配置 → 走 `#[serde(default)]` → `Disabled` → **润色静默关闭**。

**不会报错，但润色会静默关闭**——必须在文档（`docs/configuration.md` 顶部 + 完整示例）显著标注迁移：`polish_enabled: true` → `polish_mode: 2`（或 `1`）。

> 不做自动迁移（如检测旧 key warn）：YAGNI，项目早期用户少，文档提示即可。

## 8. 验证

- `cargo check --workspace --all-targets`：0 error
- `cargo test -p octopus-infra`：新增 `PolishMode` 反序列化单测：
  - `0 → Disabled`、`1 → FinalOnly`、`2 → Intermediate`
  - 非法值（如 `3`）→ `Disabled` + warn
  - 缺失 → `Disabled`（default）
- e2e（备份 `~/.octopus/` 后）：

| `polish_mode` | `polish_interval` | 预期 |
|---|---|---|
| `0` | 任意 | 不润色（`llm_config` 返回 None） |
| `1` | 任意 | 仅最终润色（中间 tick 不触发） |
| `2` | `5.0` | 中间润色每 ≥5s 触发一次 + 最终润色 |
| `2` | `0` | warn + 实际按 1.0s 节流 |

详见实施计划（下一步 writing-plans 生成）。
