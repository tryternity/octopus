# Prompt 外部文件引用实施计划

> **Spec:** `docs/superpowers/specs/2026-07-23-prompt-file-reference.md`
> **状态**：✅ 已实现

---

## Task 1：resolve_prompt_reference + list_prompt_files + open_file_in_editor

**文件**: `crates/desktop/src/action_bar_commands.rs`

- [x] `resolve_prompt_reference(action_data) -> String`（@文件名 → 读 ~/.octopus/.sync/prompts/<name>.md；失败降级原文）
- [x] `list_prompt_files` Tauri 命令（扫描 ~/.octopus/.sync/prompts/*.md 返回 {name, fileName, preview 前 500 字符}）
- [x] `open_file_in_editor` Tauri 命令（读全文 → CompactEditor source="file" 打开，按路径 md5 去重）
- [x] main.rs 注册 list_prompt_files + open_file_in_editor
- [x] 测试：4 个 resolve_prompt_reference 纯逻辑测试

## Task 2：注入 agent/ai 执行路径

- [x] ai 非 auto_translate 分支：resolve_prompt_reference
- [x] agent 非语音分支：resolve_prompt_reference 后再 render_agent_prompt
- [x] agent 语音路径 context JSON：prompt_template 用 resolved

## Task 3：前端 PromptEditor 组件

**文件**: `crates/desktop/frontend/src/pages/Settings/ActionBar/PromptEditor.tsx`（新建）

- [x] Segmented 切换（内联 / 引用文件），独立 mode state（切换不碰 value）
- [x] 内联模式：textarea（原行为）
- [x] 引用模式：文件下拉（调 list_prompt_files）+ 路径展示 + hover 浮层预览（1s 延迟消失，向上弹出）
- [x] 「查看更多/编辑内容」按钮调 open_file_in_editor（CompactEditor 打开全文）
- [x] 空目录状态提示（Inbox 图标 + 路径指引）
- [x] 父组件 key={form.id} 让切换菜单项时重新 mount

## Task 4：CompactEditor file source

**文件**: `crates/desktop/src/compact_editor_commands.rs` + `crates/desktop/frontend/src/pages/CompactEditor/index.tsx`

- [x] 后端 `store_pending_file`（source="file"，不查 DB，text 直接携带）
- [x] 前端 Tab source 类型加 `'file'`
- [x] 前端 open-tab 事件加 `source === 'file'` 分支（按 `file:<itemId>` 去重，已存在激活不覆盖）

## Task 5：ActionBarPanel 集成

- [x] import PromptEditor
- [x] isPromptType 判断（agent + ai）
- [x] prompt 类型用 PromptEditor（key={form.id}），url/script 保持原 textarea

## Task 6：i18n + 文档

- [x] en.yaml + zh-CN.yaml 8 个新 key（promptInline/promptRef/promptSelectFile/promptDirEmpty/promptFileMissing/promptPreview/promptViewMore + contentLabel 复用）
- [x] spec 状态更新 + §3.4 接口表 + §3.5 UI 设计 + INV-P5
- [x] architecture.md agent/ai 段补「@文件名引用 prompt」+ hover 浮层 + open_file_in_editor
- [ ] e2e：Tolaria action_data 改 @tolaria → 执行 → prompt 被正确展开
