# 润色提示词文件引用设计（prompts 表 content → 文件引用）

> **日期**：2026-07-24
> **状态**：设计阶段

---

## 0. 目标

提示词文件统一管理，按用途分子目录：
- **润色提示词**：`~/.octopus/.sync/prompts/polish/`（系统管理 → 提示配方，prompts 表引用）
- **命令菜单 prompt**：`~/.octopus/.sync/prompts/command/`（action_bar agent/ai 的 `@文件名` 引用）

润色提示词从 DB 存完整 content 改为引用文件。启动时拷贝内置模板。编辑/查看合并为一个按钮进 CompactEditor。支持新建空白 prompt 文件。

## 1. 关键决策（已确认）

| 决策 | 选择 | 理由 |
|---|---|---|
| prompts 表 | **保留**，content 字段改为文件引用（不存完整 md） | title/description 元数据仍在 DB；active_polish_prompt 仍按 id 存 |
| 润色文件目录 | `~/.octopus/.sync/prompts/polish/` | 和命令行 prompt 子目录隔离 |
| 命令文件目录 | `~/.octopus/.sync/prompts/command/` | 原来直接放 prompts/ 下，加 command/ 子目录隔离 |
| 内置文件名 | `润色-默认.md` / `润色-进阶.md`（中文） | 直观 |
| `open_file_in_editor` | 泛化加 `category` 参数（"polish" / "command"） | 支持不同子目录 |
| 新建空白文件 | UI 加「新建文件」入口，创建空 md + 打开 CompactEditor | 用户不用手动去磁盘创建 |

## 2. 数据模型变更

### 2.1 prompts 表 content 字段语义变更

```sql
-- 不改表结构，content 字段语义从「完整 prompt 文本」变为「文件引用名」
-- 如 content = "润色-默认" → 读 ~/.octopus/.sync/prompts/polish/润色-默认.md
```

- `content` 存**文件名**（不含路径、不含扩展名），如 `润色-默认`
- 运行时拼接路径 `~/.octopus/.sync/prompts/polish/<content>.md` 读取实际 prompt
- DB 不再存完整 prompt 文本——只存引用 key + title/description 元数据

### 2.2 启动拷贝内置模板

启动时（`ensure_db` 或 setup 阶段）：
1. 确保 `~/.octopus/.sync/prompts/polish/` 目录存在
2. 从应用内 `seeds/prompts/default-polish.md` 拷贝为 `润色-默认.md`（已存在则跳过——`!exists` 才拷贝，不覆盖用户编辑）
3. 从 `seeds/prompts/advanced-polish.md` 拷贝为 `润色-进阶.md`（同上）
4. seed prompts 表行：id=1 content=`润色-默认`，id=2 content=`润色-进阶`

### 2.3 seed 加载变更

现有 `seeds.rs:load_prompt_seeds` 读 seed md 文件内容写入 DB content 字段。改为：
- DB content 字段存文件名引用（`润色-默认` / `润色-进阶`）
- 不再把 md 内容读进 DB
- 拷贝 md 文件到 `~/.octopus/.sync/prompts/polish/`（幂等，已存在跳过）

## 3. 运行时读取

### 3.1 启动加载 active prompt

```rust
// main.rs 启动：读 active id → 从 DB 拿 content（=文件名）→ 读文件 → set_system_prompt
let active_id = load_active_prompt_id().unwrap_or(1);
let record = load_prompt(active_id)?;
let prompt_text = read_prompt_file(&record.content)?; // 读 ~/.octopus/.sync/prompts/polish/<content>.md
octopus_llm::set_system_prompt(&prompt_text);
```

### 3.2 set_active_prompt

激活时同样读文件内容 → `set_system_prompt`。

### 3.3 文件读取函数

```rust
/// 读润色 prompt 文件内容。content 是文件名（不含扩展名）。
/// 路径：~/.octopus/.sync/prompts/polish/<content>.md
/// 失败时返回空串（降级——不让润色功能完全卡死）。
fn read_prompt_file(content: &str) -> String {
    let path = octopus_config_home().join(".sync").join("prompts").join("polish").join(format!("{}.md", content));
    std::fs::read_to_string(&path).unwrap_or_default()
}
```

### 3.4 `open_file_in_editor` 泛化（加 category 参数）

现有 `open_file_in_editor(name)` 硬编码 `~/.octopus/.sync/prompts/`。改为 `open_file_in_editor(name, category)`：
- `category="polish"` → `~/.octopus/.sync/prompts/polish/<name>.md`
- `category="command"` → `~/.octopus/.sync/prompts/command/<name>.md`
- md5 hash 路径也包含 category 子目录（去重 key 更精确）

同步改 `list_prompt_files` 加 category 参数，`resolve_prompt_reference` 改读 `command/` 子目录。

### 3.5 新建空白 prompt 文件

新增 `create_prompt_file(category, name) -> ()` 命令：
- 在 `~/.octopus/.sync/prompts/<category>/` 创建空 `<name>.md`（已存在则报错）
- 创建后前端直接调 `open_file_in_editor(name, category)` 打开 CompactEditor

## 4. 前端 PromptsPanel 改造

### 4.1 编辑/查看合并为一个按钮

- 去掉「查看」和「编辑」两个按钮，合并为单个「编辑」按钮
- 点击 → 调 `open_file_in_editor(content, "polish")`（复用 CompactEditor file source 机制）打开对应 md 文件
- 不再在 PromptsPanel 内部做 textarea 编辑——编辑全部在 CompactEditor 里完成

### 4.2 列表展示

- 每个 prompt card：title + description + 内置/用户 badge + 使用中 badge
- 按钮：启用（非激活时）+ 编辑（CompactEditor 打开文件）

### 4.3 新建 prompt（含创建空白文件）

新建时：
- title input + description input + **文件名 input**（用户指定 md 文件名）
- 保存：`create_prompt_file("polish", name)` 创建空 md → DB `insert_prompt(title, name, description)` → 自动打开 CompactEditor 编辑

### 4.4 命令菜单 PromptEditor 同步改

`PromptEditor.tsx`（action_bar agent/ai 引用文件模式）同步改：
- `list_prompt_files` 调用加 `category="command"`
- `open_file_in_editor` 调用加 `category="command"`
- 引用模式下加「新建文件」入口（弹简单 input 填文件名 → `create_prompt_file("command", name)` → 刷新下拉）

### 4.5 hover 浮层预览路径更新

PromptEditor 的 hover 浮层 + 空目录提示路径从 `~/.octopus/.sync/prompts/` 改为 `~/.octopus/.sync/prompts/command/`

## 5. 不变量

| # | 不变量 | 保证方式 |
|---|---|---|
| INV-PP1 | prompts 表 content 字段存文件名引用，不存完整 md | seed + CRUD 写文件名 |
| INV-PP2 | 启动拷贝内置模板幂等（已存在不覆盖） | `!path.exists()` 守卫 |
| INV-PP3 | INCREMENTAL_RULE 拼接逻辑不变 | `build_system_prompt` 不改，只在读文件后拼 |
| INV-PP4 | 文件读取失败降级空串不卡死 | `unwrap_or_default()` |
| INV-PP5 | active_polish_prompt 仍按 id 存 app_config | 不改 active id 存储方式 |

## 6. 已知限制

- 用户自建 prompt 需手动指定文件名（不像命令行 prompt 那样自动扫描目录）
- 删除 prompt 不删 md 文件（只删 DB 行，文件留着以防误删）
- 不支持文件监听自动刷新 DB 列表（prompts 表是手动管理的）
