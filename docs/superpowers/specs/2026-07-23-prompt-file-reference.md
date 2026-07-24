# Prompt 外部文件引用设计（@文件名 语法）

> **日期**：2026-07-23
> **状态**：✅ 已实现（cargo test + tsc + vite build 通过；e2e 待用户验证）

---

## 0. 目标

让 action_bar 菜单项的 `action_data`（prompt 内容）支持引用外部 `.md` 文件，而不是把完整 prompt 内联在数据库里。

用户场景：Tolaria 命令的 prompt 有几百字，写在 DB 里难维护。改为 `@tolaria` → 运行时读 `~/.octopus/.sync/prompts/tolaria.md` 的内容作为 prompt。

## 1. 关键决策（已确认）

| 决策 | 选择 | 理由 |
|---|---|---|
| 引用语法 | `@文件名`（如 `@tolaria`） | 简洁，@ 开头直觉是「引用」 |
| 适用范围 | agent + ai 类型 | 这两类的 action_data 是 prompt 模板，最长最需要外置 |
| 文件不存在 | 当普通文本处理（降级） | 安全——`@tolaria` 原样当 prompt，不报错 |
| 文件目录 | `~/.octopus/.sync/prompts/` | 集中管理，与现有 VOICE_POLISH.md 同目录区 |

## 2. 语法规则

### 2.1 引用格式

```
@文件名
```

- `@` 后是文件名（不含路径、不含扩展名）
- 自动在 `~/.octopus/.sync/prompts/<文件名>.md` 查找
- **整个 action_data 必须只有 `@文件名`**（首尾 trim 后）——不支持文本中间混用引用

### 2.2 识别规则

```rust
fn resolve_prompt_reference(action_data: &str) -> String {
    let trimmed = action_data.trim();
    if let Some(name) = trimmed.strip_prefix('@') {
        let name = name.trim();
        let path = octopus_config_home().join("prompts").join(format!("{}.md", name));
        if let Ok(content) = std::fs::read_to_string(&path) {
            return content;  // 文件内容作为完整 prompt
        }
    }
    action_data.to_string()  // 不是引用 / 文件不存在 → 原文返回（降级）
}
```

- `@tolaria` → 读 `~/.octopus/.sync/prompts/tolaria.md`
- `@tolaria ` (尾部空格) → trim 后同上
- `@tolaria.md` → strip_prefix 后是 `tolaria.md`，查找 `tolaria.md.md`（大概率不存在→降级）——不特殊处理，用户应写 `@tolaria` 不带扩展名
- `请处理：@tolaria` → 不是纯引用（strip_prefix 后含 `：@`）→ 当普通文本
- `@subdir/file` → 查找 `subdir/file.md`（路径拼接，支持子目录）

### 2.3 不处理的边界

- 不支持多个 `@` 引用拼接（`@a @b` 当普通文本）
- 不支持引用内再引用（文件内容里的 `@x` 不展开）
- 不支持非 `.md` 扩展名（固定 `.md`）

## 3. 注入点

### 3.1 agent 类型

`execute_action_bar_inner` 的 agent 分支（行 1765）：
```rust
// 改前
let prompt = render_agent_prompt(&item.action_data, "", &text, &app_state_files);
// 改后
let resolved = resolve_prompt_reference(&item.action_data);
let prompt = render_agent_prompt(&resolved, "", &text, &app_state_files);
```

agent context JSON（行 1891 `trigger_agent_voice_core`）也要用 resolved：
```rust
"prompt_template": resolve_prompt_reference(&item.action_data),
```

### 3.2 ai 类型

`execute_action_bar_inner` 的 ai 非 auto_translate 分支（行 1663）：
```rust
// 改前
let prompt = item.action_data.clone();
// 改后
let prompt = resolve_prompt_reference(&item.action_data);
```

### 3.3 不注入的类型

- `url`：action_data 是 URL 模板，不是 prompt
- `script`：action_data 是脚本内容/路径（已有自己的文件读取逻辑行 1712-1717）
- `copy_path`：action_data 是路径格式
- `auto_translate`：特殊值，不是 prompt

### 3.4 新增 Tauri 命令

| 命令 | 签名 | 用途 |
|---|---|---|
| `list_prompt_files` | `() -> Vec<PromptFileInfo>` | 扫描 `~/.octopus/.sync/prompts/*.md`，返回文件列表供设置页下拉选择 |
| `open_file_in_editor` | `(name: String, app) -> ()` | 读文件全文 → CompactEditor 打开（source="file"，按路径 md5 去重） |

```rust
pub struct PromptFileInfo {
    pub name: String,      // 文件名（不含扩展名），如 "tolaria"——对应 @tolaria 引用
    pub file_name: String, // 完整文件名，如 "tolaria.md"
    pub preview: String,   // 前 500 字符预览，供 hover 浮层展示
}
```

`open_file_in_editor` 用 CompactEditor 的 `source="file"` tab——item_id = 文件完整路径 md5 前 8 字节转 i64，前端按 `file:<itemId>` 去重（同一文件只开一个 tab，已存在则激活不覆盖）。

目录不存在时返回空 Vec（首次使用时目录还没建，不报错）。

### 3.5 UI 设计（PromptEditor 组件）

**使用 frontend-design skill 指导**

设置页 agent/ai 菜单项的内容区用 `PromptEditor` 组件替换原 textarea：
- **Segmented 切换**：「内联」/「引用文件」两种模式，独立 mode state（切换不碰 value）
- **内联模式**：textarea 直接写 prompt（原有行为）
- **引用模式**：文件下拉（`list_prompt_files`）+ 路径展示 + **hover 浮层预览**（1s 延迟消失，前 500 字符，向上弹出）+ 「查看更多 / 编辑内容」按钮（调 `open_file_in_editor` 用 CompactEditor 打开全文）
- **空目录状态**：Inbox 图标 + 路径指引（`~/.octopus/.sync/prompts/*.md`）
- 父组件用 `key={form.id}` 让切换不同菜单项时 PromptEditor 重新 mount

## 4. 不变量

| # | 不变量 | 保证方式 |
|---|---|---|
| INV-P1 | 仅当 action_data trim 后以 `@` 开头才尝试文件引用 | strip_prefix 守卫 |
| INV-P2 | 文件不存在时降级为原文，不报错 | read_to_string 失败 → 返回原文 |
| INV-P3 | 引用展开发生在执行时（非保存时） | resolve 在 execute_action_bar_inner 内调，DB 存原始 `@文件名` |
| INV-P4 | DB 存原始引用字符串（`@tolaria`），不存展开后内容 | resolve 不写回 DB |
| INV-P5 | 同一文件在 CompactEditor 只开一个 tab（file source 去重） | item_id = 路径 md5，前端 `file:<id>` 去重，已存在激活不覆盖 |

## 5. 文件目录

`~/.octopus/.sync/prompts/`——与 `VOICE_POLISH.md`（自定义润色 prompt）同在 `~/.octopus/` 下。不新建 `prompts/` 子目录的话文件会和配置混在一起，所以用 `prompts/` 子目录隔离。

用户把 prompt 文件放 `~/.octopus/.sync/prompts/tolaria.md`，在设置页 action_data 填 `@tolaria`。

## 6. 已知限制

- 不支持热重载通知（用户改了 prompt 文件内容，下次执行时自然读到新内容——无需通知机制，因为是执行时读文件）
- 不支持文件监听（YAGNI）
- 不支持引用非 `.md` 文件
