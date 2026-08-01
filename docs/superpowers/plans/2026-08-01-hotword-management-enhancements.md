# 实施计划：热词管理增强（容量限制 + fuzzy 搜索 + 批量添加浮窗）

> **对应 spec**：[2026-08-01-hotword-management-enhancements.md](../specs/2026-08-01-hotword-management-enhancements.md)
> **分支**：`bugfix/pr-0801`
> **状态**：✅ 已完成

## 背景

PR 0801 热词页 UI 重构过程中的功能增强。事后补录 spec + plan（代码先于文档，本 plan 为实施记录）。

## 任务分解 + 实施记录

### Task 1：单词典容量限制 3000 词 ✅

- [x] `HOTWORD_SET_MAX_WORDS = 3000` 常量 + `ensure_within_capacity` 校验函数
- [x] 三写入口加校验：`set_hotword_set_words` / `add_word_to_set_at` / `add_words_to_set`
- [x] TDD 3 测试（边界 3000 通过 / 批量超限被拒不部分写入 / 单词满后再加被拒）
- commit `93c377df`

### Task 2：fuzzy 搜索复用 matcher::match_score ✅

- [x] Tauri command `filter_hotwords_fuzzy`，复用 `octopus_search::matcher::match_score`
- [x] 注册到 invoke_handler
- [x] TDD 5 测试（filters_and_sorts / pinyin_initials / exact_ranks / empty / no_match）
- [x] 前端：删本地 `isSubsequence`，改 async debounce 120ms + `fuzzyMatches` state
- [x] placeholder「搜索（模糊）」
- commit `635112cb`（前端 placeholder）+ `8b1bce89`（后端 + 前端 async）

### Task 3：批量添加浮窗 ✅

- [x] 操作行 4 按钮（追加/覆盖/挖掘/添加）改纯图标并入 CardHeader
- [x] 「添加」弹 textarea 浮层（任意空白分割批量 `add_words_to_set`）+ 添加/取消
- [x] 「挖掘」内联面板改浮窗（候选 chip + 手动补词 + 添加/取消）
- [x] 删除废弃的单词 `addWord`/`input` state
- commit `fde5d3cc`

## 与计划的偏差

无（事后补录，plan 反映实际实施）。

## 不在本次范围（spec §5）

- [ ] textarea 长度上限提示（P2）
- [ ] fuzzy 搜索 IPC 延迟压测（P2）
