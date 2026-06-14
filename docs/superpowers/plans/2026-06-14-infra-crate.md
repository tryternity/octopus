# infra crate 实施计划（跨 crate 基础设施收敛）

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development (recommended) or superpowers:executing-plans。**本计划已全部实现，存档备查。**

**Goal:** 新增 infra crate 收敛固定路径常量 + `octopus_config_home()`，消除三处 `handy_home()` 重复定义。

**Architecture:** infra 为依赖图底端（无项目内依赖），承载 `consts`（SILERO_VAD_PATH / DEFAULT_ASR_MODEL_DIR / VOICE_POLISH_FILE）+ `paths`（`octopus_config_home()`）。asr/llm/dlp/desktop/cli 改用 `octopus_infra::*`。

**Tech Stack:** Rust workspace, once_cell (Lazy)

---

## 任务分解（全部已完成 ✅）

### Task 1: 新建 infra crate

- [x] `crates/infra/Cargo.toml`：`name = "octopus-infra"`, dep `once_cell = "1"`
- [x] `crates/infra/src/consts.rs`：`SILERO_VAD_PATH` / `DEFAULT_ASR_MODEL_DIR` / `VOICE_POLISH_FILE`
- [x] `crates/infra/src/paths.rs`：`octopus_config_home()`（`Lazy<&'static Path>`）
- [x] `crates/infra/src/lib.rs`：模块声明 + `pub use paths::octopus_config_home`（root re-export）
- [x] workspace `Cargo.toml` members 加 `crates/infra`

### Task 2: asr 接入 infra

- [x] `asr/config.rs`：删 `static HANDY_HOME` + `fn handy_home()` + `once_cell::sync::Lazy` import；3 处调用（resolve_model_dir / find_silero_vad / load_app_config）改 `octopus_config_home()`；引入 `SILERO_VAD_PATH`
- [x] `asr/db.rs`：`DEFAULT_ASR_MODEL_DIR` + `octopus_config_home().join("octopus.db")`
- [x] `asr/Cargo.toml`：加 `octopus-infra = { path = "../infra" }`

### Task 3: dlp / llm / desktop / cli 接入

- [x] `dlp/main.rs`：删自建 `fn handy_home()`，3 处改 infra；`dlp/Cargo.toml` 加 dep
- [x] `llm/prompt.rs`：删 `VOICE_POLISH_FILE` 定义（移入 infra）；`llm/examples/test_polish.rs` 删 `fn octopus_home()` 改 infra；`llm/Cargo.toml` 加 dep
- [x] `desktop/config.rs` + `main.rs`：改 infra（main 用 `VOICE_POLISH_FILE`）；`desktop/Cargo.toml` 加 dep
- [x] `cli/main.rs`：2 处改 infra；`cli/Cargo.toml` 加 dep

### Task 4: 文档同步

- [x] `architecture.md`：infra 模块说明（consts + paths）+ 结构树注释
- [x] 新建 spec [`2026-06-14-infra-crate-design.md`](../specs/2026-06-14-infra-crate-design.md)
- [x] db-single-source spec：`handy_home` → `octopus_config_home` + 路径常量集中说明

## 验证

- [x] `cargo check --workspace --all-targets`：0 error（Finished）
- [x] `cargo test -p octopus-asr`：9 passed
- [x] grep 全仓确认 `handy_home` / `HANDY_HOME` / `octopus_home` 零残留

## 过程问题（记录备查）

1. **infra root 不可达**：`octopus_config_home` 定义在 `paths` 模块，但所有调用点用 root 级 `octopus_infra::octopus_config_home`（E0432）。→ `lib.rs` 加 `pub use paths::octopus_config_home;` re-export。先前 `cargo check` 走缓存未暴露，`cargo test` 完整编译才报出。
2. **cli 漏声明依赖**：cli 引用了 `octopus_infra` 但 `Cargo.toml` 未声明 → 补 `octopus-infra = { path = "../infra" }`。通过对比「引用 infra 的 crate」vs「声明 infra 依赖的 crate」差集发现。
