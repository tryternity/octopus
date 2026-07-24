# 润色提示词文件引用实施计划

> **Spec:** `docs/superpowers/specs/2026-07-24-polish-prompt-file-ref.md`
> **状态**：待实施

---

## Task 概览

| # | Task 组 | 优先级 |
|---|---|---|
| 1 | 后端：open_file_in_editor / list_prompt_files / resolve_prompt_reference 加 category 子目录 | P0 |
| 2 | 后端：启动拷贝内置润色模板到 polish/ + seed content 改文件名 | P0 |
| 3 | 后端：read_prompt_file + 启动加载 + set_active_prompt 改读文件 | P0 |
| 4 | 后端：create_prompt_file 命令（新建空白 md） | P0 |
| 5 | 前端：PromptsPanel 编辑/查看合并 + 新建文件 | P0 |
| 6 | 前端：PromptEditor 改 command 子目录 + 新建文件 | P1 |
| 7 | 测试 + 文档 | P0 |

---

## Step 1：后端命令泛化（category 子目录）

### Task 1.1：open_file_in_editor 加 category

**文件**: `crates/desktop/src/action_bar_commands.rs`

- [ ] `open_file_in_editor(name, category)` — 路径 `~/.octopus/.sync/prompts/<category>/<name>.md`
- [ ] md5 hash 路径含 category
- [ ] main.rs 注册更新

### Task 1.2：list_prompt_files 加 category

- [ ] `list_prompt_files(category)` — 扫 `~/.octopus/.sync/prompts/<category>/*.md`

### Task 1.3：resolve_prompt_reference 改 command 子目录

- [ ] `resolve_prompt_reference` 路径改 `~/.octopus/.sync/prompts/command/<name>.md`

### Task 1.4：save_file / read_file_text 不变（已接收完整路径）

## Step 2：启动拷贝内置润色模板

### Task 2.1：seeds 拷贝 md 到 polish/

**文件**: `crates/infra/src/seeds.rs` + `crates/desktop/src/main.rs`

- [ ] `ensure_polish_prompt_files()`：确保 `~/.octopus/.sync/prompts/polish/` 存在 + 拷贝 `default-polish.md` → `润色-默认.md` + `advanced-polish.md` → `润色-进阶.md`（已存在跳过）
- [ ] 启动 setup 调用

### Task 2.2：seed prompts 表 content 改文件名

**文件**: `crates/infra/src/seeds.rs`

- [ ] `load_prompt_seeds` 的 INSERT content 从读 md 文件内容改为存文件名（`润色-默认` / `润色-进阶`）
- [ ] 现有 DB（content 存完整 md）迁移：v49→v50 迁移 UPDATE content 改文件名

## Step 3：运行时读文件

### Task 3.1：read_prompt_file 函数

**文件**: `crates/desktop/src/settings_commands.rs` 或 `action_bar_commands.rs`

- [ ] `read_prompt_file(content: &str) -> String`：读 `~/.octopus/.sync/prompts/polish/<content>.md`，失败降级空串

### Task 3.2：启动加载改读文件

**文件**: `crates/desktop/src/main.rs`

- [ ] 启动时 `load_prompt(active_id)` 拿 content（文件名）→ `read_prompt_file` 读文件 → `set_system_prompt`

### Task 3.3：set_active_prompt 改读文件

**文件**: `crates/desktop/src/settings_commands.rs`

- [ ] `set_active_prompt` 改为 `read_prompt_file(&record.content)` → `set_system_prompt`

## Step 4：create_prompt_file 命令

**文件**: `crates/desktop/src/action_bar_commands.rs`

- [ ] `create_prompt_file(category, name)`：创建空 md（已存在报错）
- [ ] main.rs 注册

## Step 5：前端 PromptsPanel 改造

**使用 frontend-design skill**

### Task 5.1：编辑/查看合并

**文件**: `crates/desktop/frontend/src/pages/Settings/PromptsPanel.tsx`

- [ ] 去掉查看/编辑两个按钮 → 合并为「编辑」调 `open_file_in_editor(content, "polish")`
- [ ] 去掉内部 textarea 编辑视图

### Task 5.2：新建 prompt（含创建文件）

- [ ] 新建表单：title + description + 文件名 input
- [ ] 保存调 `create_prompt_file("polish", name)` + `create_prompt(title, name, description)` + 自动打开 CompactEditor

## Step 6：PromptEditor 改 command 子目录

### Task 6.1：调用加 category

**文件**: `crates/desktop/frontend/src/pages/Settings/ActionBar/PromptEditor.tsx`

- [ ] `list_prompt_files("command")` + `open_file_in_editor(name, "command")`
- [ ] hover 浮层路径改 `~/.octopus/.sync/prompts/command/`

### Task 6.2：新建文件入口

- [ ] 引用模式空目录或下拉旁加「新建文件」按钮 → input 填名 → `create_prompt_file("command", name)` → 刷新

## Step 7：测试 + 文档

- [ ] cargo test + tsc + vite build
- [ ] architecture.md 更新
- [ ] e2e 验证
