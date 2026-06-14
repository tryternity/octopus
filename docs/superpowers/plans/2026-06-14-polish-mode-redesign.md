# LLM 润色模式三档化（polish_mode）实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development（推荐）或 superpowers:executing-plans。Steps 用 checkbox（`- [ ]`）跟踪。

**Goal:** 将 `polish_enabled: bool` + `polish_interval` 的隐式三态收敛为显式枚举 `PolishMode`（0/1/2），desktop 三处判断点改用枚举，底层润色引擎与流式/伪流式共用路径不变。

**Architecture:** infra 新增 `PolishMode` 枚举（自定义 `Deserialize` 解整数 0/1/2，非法值回退 `Disabled`）+ `polish_mode` 字段；desktop 把 `llm_config` / `check_and_trigger_polish` / `main.rs` 启动校验三处从读 `polish_enabled` 改为 match `polish_mode`；最后删 `polish_enabled`。**增量顺序保证每步可编译**：先加 `polish_mode`（保留 `polish_enabled`）→ 改完所有 desktop 引用 → 再删 `polish_enabled`。

**Tech Stack:** Rust workspace, serde（自定义 Deserialize）, log

**Spec:** [2026-06-14-polish-mode-redesign-design.md](../specs/2026-06-14-polish-mode-redesign-design.md)

---

## File Structure

| 文件 | 职责 | 改动 |
|---|---|---|
| `crates/infra/Cargo.toml` | 依赖清单 | 加 `log = "0.4"` |
| `crates/infra/src/config.rs` | 统一 config schema | 加 `PolishMode` 枚举 + `Deserialize` impl + `polish_mode` 字段 + `Default` + 反序列化单测；Task 3 删 `polish_enabled` |
| `crates/desktop/src/config.rs` | desktop 配置接入 | re-export `PolishMode`；`llm_config` 判断改 `polish_mode` |
| `crates/desktop/src/coordinator.rs` | 录音协调器 | `check_and_trigger_polish` guard 改 `polish_mode`；interval 用 `.max(MIN_POLISH_INTERVAL_SEC)`；加常量 |
| `crates/desktop/src/main.rs` | 入口 | 启动校验改 `match polish_mode` |
| `docs/configuration.md` | 配置指南 | `polish_mode` 字段 + 注释示例 |
| `docs/architecture.md` | 架构概览 | 润色段落三档化 |

---

## Task 1: infra 新增 PolishMode 枚举 + polish_mode 字段 + 单测

**Files:**
- Modify: `crates/infra/Cargo.toml`
- Modify: `crates/infra/src/config.rs`

**说明：** 本 task **保留** `polish_enabled` 字段不动（仅新增 `polish_mode`），确保此步编译通过——desktop 仍在用 `polish_enabled`，Task 2 才改 desktop 引用，Task 3 才删 `polish_enabled`。

- [ ] **Step 1: 写失败测试。** 在 `crates/infra/src/config.rs` 末尾（`load_config` 函数之后）追加 test mod：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polish_mode_deserialize_values() {
        assert_eq!(serde_yaml::from_str::<PolishMode>("0").unwrap(), PolishMode::Disabled);
        assert_eq!(serde_yaml::from_str::<PolishMode>("1").unwrap(), PolishMode::FinalOnly);
        assert_eq!(serde_yaml::from_str::<PolishMode>("2").unwrap(), PolishMode::Intermediate);
    }

    #[test]
    fn polish_mode_invalid_falls_back_to_disabled() {
        assert_eq!(serde_yaml::from_str::<PolishMode>("3").unwrap(), PolishMode::Disabled);
        assert_eq!(serde_yaml::from_str::<PolishMode>("99").unwrap(), PolishMode::Disabled);
    }

    #[test]
    fn polish_mode_default_is_disabled() {
        assert_eq!(PolishMode::default(), PolishMode::Disabled);
    }
}
```

- [ ] **Step 2: 跑测试确认失败。**

Run: `cargo test -p octopus-infra`
Expected: 编译失败 `cannot find type \`PolishMode\` in this scope`（红）。

- [ ] **Step 3: 加 log 依赖。** `crates/infra/Cargo.toml` 的 `[dependencies]` 末尾加一行：

```toml
[dependencies]
once_cell = "1"
serde = { version = "1", features = ["derive"] }
serde_yaml = "0.9"
anyhow = "1"
log = "0.4"
```

- [ ] **Step 4: 实现 PolishMode 枚举 + Deserialize impl。** 在 `crates/infra/src/config.rs` 的 `use crate::octopus_config_home;`（约 :9）之后、`pub struct AppConfig`（约 :14）之前插入：

```rust
/// LLM 润色模式（config.yaml 的 polish_mode 字段，整数 0/1/2）
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

impl<'de> Deserialize<'de> for PolishMode {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let n = u8::deserialize(deserializer)?;
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

- [ ] **Step 5: AppConfig 加 polish_mode 字段。** 在 `polish_enabled` 字段块（约 :68-70）之后追加一个新字段：

```rust
    /// 润色模式：0=关闭 / 1=仅最终润色 / 2=中间润色+最终润色
    #[serde(default)]
    pub polish_mode: PolishMode,
```

- [ ] **Step 6: Default impl 加 polish_mode。** 在 `impl Default for AppConfig` 的 `polish_enabled: false,`（约 :149）之后加一行：

```rust
            polish_mode: PolishMode::default(),
```

- [ ] **Step 7: 跑测试确认通过。**

Run: `cargo test -p octopus-infra`
Expected: `3 passed`（绿）。

- [ ] **Step 8: commit。**

```bash
git add crates/infra/Cargo.toml crates/infra/src/config.rs
git commit -m "feat(infra): 新增 PolishMode 枚举 + polish_mode 字段"
```

---

## Task 2: desktop 三处判断改用 polish_mode

**Files:**
- Modify: `crates/desktop/src/config.rs`
- Modify: `crates/desktop/src/coordinator.rs`
- Modify: `crates/desktop/src/main.rs`

**说明：** 三处都是把 `polish_enabled`（bool）替换为 `polish_mode`（枚举）的语义判断。改完后 desktop 不再引用 `polish_enabled`，但 infra 里该字段仍在（Task 3 删）。每步后 `cargo check -p octopus-desktop` 必须通过。这些是配置判断分支，靠**类型系统（`polish_mode` 强类型枚举 + match 穷尽）+ cargo check** 保证正确性，不另写单测（mock LLM/协调器成本高于收益）。

- [ ] **Step 1: desktop/config.rs re-export PolishMode + 改 llm_config。**

把 `crates/desktop/src/config.rs:9` 的 re-export：
```rust
pub use octopus_infra::config::AppConfig;
```
改为：
```rust
pub use octopus_infra::config::{AppConfig, PolishMode};
```

把 `crates/desktop/src/config.rs:22-27` 的 `llm_config` 开头：
```rust
/// 构建 LLM 配置，用于传给 octopus_llm::polish()。
/// 如果 polish_enabled 为 false 或 secret_key 为空，返回 None。
pub fn llm_config(cfg: &AppConfig) -> Option<octopus_llm::CompatibleLlmConfig> {
    if !cfg.polish_enabled || cfg.llm_secret_key.is_empty() {
        return None;
    }
```
改为：
```rust
/// 构建 LLM 配置，用于传给 octopus_llm::polish()。
/// polish_mode 为 Disabled 或 secret_key 为空时返回 None（模式 1/2 都启用最终润色）。
pub fn llm_config(cfg: &AppConfig) -> Option<octopus_llm::CompatibleLlmConfig> {
    if cfg.polish_mode == PolishMode::Disabled || cfg.llm_secret_key.is_empty() {
        return None;
    }
```

- [ ] **Step 2: coordinator.rs 加 import + 常量 + 改 check_and_trigger_polish。**

在 `crates/desktop/src/coordinator.rs` 顶部 import 区（`use crate::config::AppConfig;` 约 :4 附近）加：
```rust
use crate::config::PolishMode;
```

在常量区（`const VAD_SEGMENTED_TICK_INTERVAL_MS: u64 = 300;` 约 :127 之后）加：
```rust
/// 中间润色最小间隔下限（秒）：polish_mode=2 且 polish_interval<=0 时回退到此值，避免每 tick 刷爆 LLM。
pub(crate) const MIN_POLISH_INTERVAL_SEC: f64 = 1.0;
```

把 `check_and_trigger_polish`（约 :922-933）的 guard + interval 判断：
```rust
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
```
改为：
```rust
    if config.polish_mode != PolishMode::Intermediate
        || *polish_pending
        || accumulated_text.is_empty()
    {
        return;
    }

    let elapsed = last_polish_time.elapsed().as_secs_f64();
    // interval<=0 时用下限，避免每 tick 触发刷爆 LLM
    if elapsed < config.polish_interval.max(MIN_POLISH_INTERVAL_SEC) {
        return;
    }
```

> 下方 `current_len <= *polish_base_len`（新增字符数检测，约 :936-939）**不动**。

- [ ] **Step 3: main.rs 启动校验改 match。**

把 `crates/desktop/src/main.rs:49-61` 的润色校验：
```rust
    // 润色配置校验
    if config.polish_enabled {
        if config.llm_secret_key.is_empty() {
            log::warn!("polish_enabled=true 但 llm_secret_key 为空，润色功能将不生效");
        } else {
            log::info!(
                "润色已启用: provider={}, model={}, interval={}s",
                config.llm_provider,
                config.llm_model,
                config.polish_interval
            );
        }
    }
```
改为：
```rust
    // 润色配置校验（三档模式）
    use crate::config::PolishMode;
    match config.polish_mode {
        PolishMode::Disabled => {}
        PolishMode::FinalOnly => {
            if config.llm_secret_key.is_empty() {
                log::warn!("polish_mode=1 但 llm_secret_key 为空，润色不生效");
            } else {
                log::info!(
                    "润色模式: 仅最终润色 (provider={}, model={})",
                    config.llm_provider,
                    config.llm_model
                );
            }
        }
        PolishMode::Intermediate => {
            if config.polish_interval <= 0.0 {
                log::warn!(
                    "polish_mode=2 但 polish_interval={}<=0，将使用下限 {}s",
                    config.polish_interval,
                    coordinator::MIN_POLISH_INTERVAL_SEC
                );
            }
            if config.llm_secret_key.is_empty() {
                log::warn!("polish_mode=2 但 llm_secret_key 为空，润色不生效");
            } else {
                log::info!(
                    "润色模式: 中间+最终 (interval={}s, provider={}, model={})",
                    config.polish_interval,
                    config.llm_provider,
                    config.llm_model
                );
            }
        }
    }
```

- [ ] **Step 4: 编译校验。**

Run: `cargo check -p octopus-desktop`
Expected: `0 error`。若报 `cannot find value polish_enabled`，说明有遗漏的引用——grep 定位后按同样模式改。

- [ ] **Step 5: commit。**

```bash
git add crates/desktop/src/config.rs crates/desktop/src/coordinator.rs crates/desktop/src/main.rs
git commit -m "refactor(desktop): 三处润色判断改用 polish_mode 枚举"
```

---

## Task 3: infra 删 polish_enabled + workspace 校验

**Files:**
- Modify: `crates/infra/src/config.rs`

**说明：** Task 2 已把所有 desktop 引用改完，此刻删 `polish_enabled` 安全。删后全 workspace 必须无残留引用。

- [ ] **Step 1: 删 polish_enabled 字段。** 删 `crates/infra/src/config.rs` 约 :68-70 的字段块：
```rust
    /// 润色总开关
    #[serde(default)]
    pub polish_enabled: bool,
```

- [ ] **Step 2: 删 Default 里的赋值。** 删 `impl Default for AppConfig` 里约 :149 的行：
```rust
            polish_enabled: false,
```

- [ ] **Step 3: workspace 编译校验。**

Run: `cargo check --workspace --all-targets`
Expected: `0 error`。若有残留 `polish_enabled` 引用报错，按报错定位修复（grep `polish_enabled` 确认清零）。

- [ ] **Step 4: grep 确认清零。**

Run: `grep -rn "polish_enabled" crates/ --include="*.rs"`
Expected: 无输出（已彻底移除）。

- [ ] **Step 5: commit。**

```bash
git add crates/infra/src/config.rs
git commit -m "refactor(infra): 删除已废弃的 polish_enabled 字段"
```

---

## Task 4: 文档同步

**Files:**
- Modify: `docs/configuration.md`
- Modify: `docs/architecture.md`

- [ ] **Step 1: configuration.md 字段表。** 把约 :83-84 两行：
```
| `polish_enabled` | bool | `false` | desktop | LLM 润色总开关 |
| `polish_interval` | f64 | `5.0` | desktop | 中间润色间隔（秒），0 = 仅最终润色 |
```
改为：
```
| `polish_mode` | int | `0` | desktop | LLM 润色模式：0=关闭 / 1=仅最终润色 / 2=中间润色+最终润色 |
| `polish_interval` | f64 | `5.0` | desktop | 中间润色最小间隔（秒），仅 `polish_mode=2` 生效；`<=0` 回退 `1.0s` |
```

- [ ] **Step 2: configuration.md 完整示例。** 把约 :133-134 的示例段：
```yaml
# LLM 润色（可选）
polish_enabled: false
polish_interval: 5.0             # 秒，0 = 仅最终润色
```
改为：
```yaml
# LLM 润色（可选）
polish_mode: 0                   # 0=关闭 / 1=仅最终润色 / 2=中间润色+最终润色
polish_interval: 5.0             # 秒，仅 polish_mode=2 生效（中间润色最小间隔）
```

- [ ] **Step 3: configuration.md 顶部加迁移提示。** 在「## config.yaml」章节首段（约 :67「应用行为配置，文件不存在时使用默认值。」）之后插入：

> **⚠️ 迁移提示**：旧字段 `polish_enabled: true` 已废弃。请改用 `polish_mode`（`true` + interval>0 → `polish_mode: 2`；`true` + interval=0 → `polish_mode: 1`）。旧字段被忽略，未配置 `polish_mode` 时润色默认关闭。

- [ ] **Step 4: architecture.md 润色段落。** 在「核心状态机（Coordinator）」段的 `- `polish_status` 基于润色调用结果...`（约 :113）之后追加一行：

```
- 润色三档（`polish_mode`：0 关闭 / 1 仅最终 / 2 中间+最终）：中间润色由流式/伪流式 tick 共用 `check_and_trigger_polish` 触发，节流 `polish_interval`（下限 `MIN_POLISH_INTERVAL_SEC=1.0s`）+ 新增字符检测；最终润色在 `Stage::Pasting` 入口（`start_pasting`）。详见 [设计](superpowers/specs/2026-06-14-polish-mode-redesign-design.md)。
```

- [ ] **Step 5: commit。**

```bash
git add docs/configuration.md docs/architecture.md
git commit -m "docs: polish_mode 三档化同步"
```

---

## 验证

```bash
cargo check --workspace --all-targets   # 0 error
cargo test -p octopus-infra             # 3 passed（PolishMode 反序列化）
grep -rn "polish_enabled" crates/ --include="*.rs"   # 无输出
```

**手动 e2e**（备份 `~/.octopus/` 后，desktop 跑各档）：

| `polish_mode` | `polish_interval` | 预期 |
|---|---|---|
| `0` | 任意 | 不润色（`llm_config` 返回 None，日志无润色行） |
| `1` | 任意 | 仅最终润色（启动日志「仅最终润色」；中间不触发 PolishDone） |
| `2` | `5.0` | 中间润色每 ≥5s 触发一次 + 最终润色（启动日志「中间+最终 interval=5s」） |
| `2` | `0` | 启动 warn「将使用下限 1.0s」；中间润色按 1.0s 节流 |
| `3`（非法） | 任意 | 启动 warn「非法，回退 0(Disabled)」；润色关闭 |

---

## 自审记录

- **Spec coverage**：spec §2（枚举 + Deserialize）→ Task 1；§3.1（llm_config）→ Task 2 Step 1；§3.2（check_and_trigger_polish）→ Task 2 Step 2；§3.3（main 启动校验）→ Task 2 Step 3；§4（interval 边界）→ Task 2 Step 2 的 `.max(MIN_POLISH_INTERVAL_SEC)` + Task 2 Step 3 的 warn；§5（流式/伪流式不变）→ 无需改，已在 plan 说明；§6（影响范围）→ 全覆盖；§7（向后兼容）→ Task 4 Step 3 迁移提示；§8（验证）→ 验证节。✓
- **Placeholder**：无 TBD/TODO，所有代码块完整。✓
- **Type consistency**：`PolishMode` 变体名（`Disabled`/`FinalOnly`/`Intermediate`）在 Task 1 定义、Task 2 使用处一致；`MIN_POLISH_INTERVAL_SEC` 在 coordinator 定义（`pub(crate) const`）与 main 引用（`coordinator::MIN_POLISH_INTERVAL_SEC`）一致。✓
