# 润色提示词文件引用实施计划

> **Spec:** `docs/superpowers/specs/2026-07-24-polish-prompt-file-ref.md`
> **状态**：✅ 已实现

---

## Task 概览

| # | Task 组 | 状态 |
|---|---|---|
| 1 | 后端：open_file_in_editor / list_prompt_files / resolve_prompt_reference 加 category 子目录 | ✅ |
| 2 | 后端：启动拷贝内置润色模板 + seed content 改文件名 + v49→v50 迁移 | ✅ |
| 3 | 后端：read_prompt_file + 启动加载 + set_active_prompt 改读文件 | ✅ |
| 4 | 后端：create_prompt_file 命令 | ✅ |
| 5 | 前端：PromptsPanel 编辑/查看合并 + 新建文件 | ✅ |
| 6 | 前端：PromptEditor 改 command 子目录 + 新建文件 | ✅ |
| 7 | 测试 + 文档 | ✅ |

---

## 详细 Task

### Task 1：后端命令泛化（category 子目录）

- [x] `open_file_in_editor(name, category)` — 路径含 category 子目录
- [x] `list_prompt_files(category)` — 扫对应子目录
- [x] `resolve_prompt_reference` 改读 command/ 子目录

### Task 2：启动拷贝内置润色模板

- [x] `load_prompt_seeds` 改：content 存文件名引用 + 拷贝 md 到 polish/（中文文件名）
- [x] `migrate_v49_to_v50`：旧 DB content 导出到 polish/ 文件 + 改文件名
- [x] 全新库设 v50 + 8 处 v49 断言改 v50

### Task 3：运行时读文件

- [x] `read_prompt_file(content)` 读 `~/.octopus/.sync/prompts/polish/<content>.md`
- [x] main.rs 启动加载改读文件
- [x] set_active_prompt / update_prompt / delete_prompt 改读文件

### Task 4：create_prompt_file

- [x] `create_prompt_file(category, name)` 创建空 md（已存在报错）

### Task 5：前端 PromptsPanel 重构

- [x] 编辑/查看合并 → 调 open_file_in_editor(content, "polish")
- [x] 新建表单：title + 文件名 + description → create_prompt_file + create_prompt + 自动打开编辑器
- [x] 列表展示改文件名引用（FileText icon + content.md）

### Task 6：PromptEditor 改 command 子目录

- [x] list_prompt_files / open_file_in_editor 加 category="command"
- [x] 路径提示改 command/
- [x] 新建文件入口（空目录 + 下拉旁 Plus 按钮 → input → create_prompt_file）

### Task 7：测试 + 文档

- [x] cargo build 0 error、desktop 400 pass、tsc 0 error、vite build 成功
- [x] spec 状态更新、architecture.md schema v50、AGENTS.md schema v50
- [ ] e2e 待用户验证
