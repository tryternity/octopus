# 模型选择 spec 实施计划

> 状态：✅ 全部完成（2026-06-16）。对应 spec：[`specs/2026-06-16-model-spec-prefix-design.md`](../specs/2026-06-16-model-spec-prefix-design.md)

## 阶段 A：infra 层 — ModelSpec + LLM 查询 ✅
- [x] `infra/src/db.rs` 新增 `ModelSpec` 枚举（`Local` / `Category` / `NameOnly`）+ `parse_model_spec` 函数
- [x] `ModelSpec::name()` 返回裸名（生命周期 `&'a str`，绑定原借用）
- [x] `load_llm_model_at` 改用 `parse_model_spec`，按两分支构建 SQL（`Local` 与 `NameOnly` 共用）：
  - `Local` / `NameOnly` → `domain='llm' AND is_local=1 AND name=?`
  - `Category` → `domain='llm' AND category=? AND name=?`
- [x] 提取 `parse_llm_row` 辅助函数减少重复

## 阶段 B：asr 层 — 引擎解析改走 spec ✅
- [x] `asr/src/config.rs` 新增 `engine_category_from_str`（5 个 ASR 类型映射）
- [x] 新增 `all_sections` 辅助函数（固定遍历顺序）
- [x] 新增 `resolve_engine_in_config(cfg, spec)` 统一解析入口（`Local`/`NameOnly` 合并 + `Category` 两分支）
- [x] `resolve_engine_category(spec)` 改委托 `resolve_engine_in_config`
- [x] `resolve_active_engine(spec)` 改委托 `resolve_engine_in_config`，返回裸名
- [x] `pub use` 导出 `parse_model_spec` / `ModelSpec` 供 asr 内部使用

## 阶段 C：引擎管理器 + 流式引擎 ✅
- [x] `asr/src/engine.rs` `AsrEngineManager.switch_model` 解析 spec → 裸名做缓存键
- [x] `asr/src/streaming_engine.rs` `StreamingSession::new` 解析 spec → 裸名传给 `StreamingParaformer::new` / `StreamingZipformer::new`

## 阶段 D：CLI 调用点 ✅
- [x] `cli/src/main.rs` `do_transcribe` — 剥离前缀后传给各引擎 `transcribe`
- [x] `cli/src/main.rs` `run_e2e` — 剥离前缀后传给流式构造器
- [x] `cli/src/main.rs` `stream_test` — 剥离前缀后传给流式测试函数

## 阶段 E：默认值 + 错误消息 ✅
- [x] `infra/src/config.rs` `polish_llm` 默认值 `glm-4-flashx` → `bigmodel:glm-4-flashx`
- [x] `infra/src/config.rs` `polish_llm` 字段注释更新（`PREFIX:NAME` 格式说明）
- [x] `llm/examples/test_polish.rs` 默认值同步
- [x] `desktop/src/config.rs` 错误消息措辞适配

## 阶段 F：测试 ✅
- [x] `infra/src/db.rs` 测试：
  - `test_load_llm_model` 用 `deepseek:` / `aliyun:` 前缀验证两个同名 LLM
  - 新增 `local:` 前缀测试（插入 is_local=1 行验证命中）
  - 新增 `parse_model_spec_variants` + `model_spec_name_strips_prefix`
- [x] `asr/src/config.rs` 测试：
  - `parse_spec_local_prefix` / `parse_spec_category_prefix` / `parse_spec_bare_name`
  - `resolve_local_prefix_finds_local_model` / `resolve_category_prefix_matches_section`
  - `resolve_category_prefix_wrong_category_returns_none` / `resolve_bare_name_equivalent_to_local`
  - `resolve_bare_name_skips_non_local`（裸名跳过 is_local=false）
  - `resolve_unknown_category_prefix_returns_none`
  - `engine_category_from_str_maps_five_types`

## 阶段 G：文档同步 ✅
- [x] `docs/configuration.md` 新增「模型选择 spec」节 + `asr_engine` / `polish_llm` 表格行更新
- [x] `docs/configuration.md` 配置示例更新为新格式
- [x] `docs/architecture.md` 引擎选择段落更新
- [x] 本 spec + plan

## 验证

- [x] `cargo check --workspace` 通过（含 desktop embedded / cli / server）
- [x] `cargo test` 全部通过（59 tests passed, 0 failed, 3 ignored）
- [x] `cargo build --release -p octopus-server -p octopus-cli` 通过
- [x] `cargo build -p octopus-llm --example test_polish` 通过
