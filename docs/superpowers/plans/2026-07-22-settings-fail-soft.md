# 设置页 fail-soft 健壮性实施计划

> **Spec:** `docs/superpowers/specs/2026-07-22-settings-fail-soft.md`

---

## Task 1：后端 get_config 各数据源独立容错 ✅

**文件**: `crates/desktop/src/settings_commands.rs`

- [x] `list_engines_from_db()` → `.unwrap_or_else(|e| { log::warn!(...); vec![] })`
- [x] `list_llm_models()` → 同上
- [x] `list_ocr_models()` → 同上
- [x] `list_prompts()` → 同上
- [x] `load_config()` 保持 `?`（致命）
- [x] `load_active_prompt_id()` 保持 `.unwrap_or(1)`

## Task 2：前端 !configResp 降级路由 ✅

**文件**: `crates/desktop/frontend/src/pages/Settings/index.tsx`

- [x] models/prompts/actionbar/agent/vault 移到 `!configResp` 之前
- [x] 只有 settings(GeneralPanel) 和 hotword 留在 `!configResp` 后面

## Task 3：验证 ✅

- [x] cargo check 0 error
- [x] tsc 0 error
- [ ] 文档同步（architecture.md fail-soft 说明）
