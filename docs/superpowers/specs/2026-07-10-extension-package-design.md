# Extension Package 格式——可分享动作包 + Skill 预留

> **日期**：2026-07-10
> **状态**：设计完成，待实施
> **关联**：[action-bar-menu-db spec](2026-07-09-action-bar-menu-db-design.md)、[脚本增强 spec](2026-07-10-action-bar-script-enhancement-design.md)、[调研报告 §11](2026-07-09-action-bar-related-tools-survey.md) PopClip Package 格式

---

## 1. 背景与目标

当前 action bar 的脚本动作基于 `action_bar_items.action_data` 单字段（内联 magic comment + 代码）。无法表达：
- 带资源文件的复杂脚本（词表、模板、配置）
- 可分享/可安装的动作包
- 未来 AI agent 的能力声明（skill）

本设计引入 **Extension Package**（`.octopusext` 文件夹），解决上述三个问题。

### 定位

octopus 是**通用办公 AI 入口系统**（语音 + 截屏 + OCR + AI + 未来 agent）。Package 格式是一期落地（单脚本 + 元数据 + 资源），二期扩展（线性管道），全程预留 skill 接口（agent 可读 SKILL.md + `action_type: skill` 调度入口）。

### 不在本次范围

- **二期 B：线性多步骤管道**（`action.type: pipeline`）——前一步 stdout 传给下一步
- **通用启动器功能**——后续独立 spec
- **正则/per-app 触发规则**（config.yaml `rules` 预留字段，二期实现）
- **action_type: skill** 的执行调度——仅预留 config.yaml `skill` 块 + 前端展示

---

## 2. Package 目录结构

```
~/.octopus/extensions/
├── translator/
│   ├── config.yaml          # 元数据 + 规则 + 执行体 + skill 声明
│   ├── main.py              # 执行脚本（相对路径引用）
│   ├── glossary.json        # 资源文件（脚本通过 $OCTOPUS_PACKAGE_DIR 读取）
│   └── SKILL.md             # 可选：Package 内自带 skill 声明
├── json-formatter/
│   ├── config.yaml
│   └── fmt.sh
└── ...
```

**Package 识别规则**：`~/.octopus/extensions/` 下每个含 `config.yaml` 的子文件夹 = 一个 Package。

**资源访问**：脚本通过环境变量 `OCTOPUS_PACKAGE_DIR`（绝对路径）读取同目录资源文件。

---

## 3. config.yaml 字段定义

```yaml
# 必填
name: "翻译助手"              # 菜单显示名称（权重上限 12）
description: "选中文本翻译为中英互译"
version: "1.0.0"
author: "user"

# 执行体
action:
  type: script                # 一期仅 script；二期加 pipeline
  script: main.py             # 相对 Package 根目录的文件路径（首行 magic comment）
  is_async: false             # 异步 fire-and-forget（默认 true）
  write_output_to_clipboard: true  # 仅同步模式可勾选

# 触发规则（一期全部可选，默认始终显示）
rules: {}
  # match: "^https?://"       # 正则匹配选中文本才显示（二期）
  # apps:                     # 仅在这些 app bundle id 中显示（二期）
  #   - com.apple.Safari

# Skill 能力声明（预留 agent 接口）
skill: {}
  # ref: ~/.agents/skills/my-skill    # 关联已有 agent skill 目录
  # file: SKILL.md                     # Package 内自带 SKILL.md
  # description: "将选中文本翻译为目标语言"  # agent 可读的能力描述（可选覆盖）
```

### 字段约束

- `name`、`description`、`version`、`author` 必填
- `action.type` 一期仅 `script`，二期加 `pipeline`
- `action.script` 必须是相对路径，指向 Package 内文件
- `action.is_async` 默认 `true`（与 DB 默认值一致）
- `skill` 整块可选——不填则 Package 是纯脚本动作，不声明 agent 能力

### skill 关联方式

Package 的 skill 能力声明**桥接已有的 agent skill 体系**：
- `skill.ref`——指向 `~/.agents/skills/`、`~/.claude/skills/` 等现成 skill 目录（绝对或 `~` 路径）
- `skill.file`——Package 文件夹内自带的 `SKILL.md`
- `skill.description`——可选，覆盖 skill 的能力描述供 agent 读取

Package = agent skill 的**可执行入口**（给 skill 绑定了触发规则 + 脚本执行体）。

---

## 4. DB 集成

### 4.1 导入 = 校验 + 保存时安装

Package 导入分两步：
1. **校验**（`import_extension`）：解压 zip 到临时目录（或直接用文件夹）→ 校验 config.yaml + 脚本文件 → 检测重复 dir_name → 返回 `ImportResult`（**不复制到 extensions**）
2. **安装**（`install_extension`）：保存时调用 → 复制到 `~/.octopus/extensions/<dir_name>/` → 创建 DB `action_bar_items` 记录

前端 `actionData` 格式：`"sourcePath|dirName"`（校验阶段），保存后 DB `action_data` 变为脚本绝对路径。

```sql
INSERT INTO action_bar_items (parent_id, title, icon, action_type, action_data,
                              sort_order, is_system, is_enabled,
                              is_async, write_output_to_clipboard)
VALUES (?parent_id, '翻译助手', '', 'script',
        '/Users/.../.octopus/extensions/translator/main.py',
        ?sort_order, 0, 1, 0, 1);
```

- `parent_id`——导入时用户选择挂到哪个父菜单（如「工具」分组），或不选（顶层）
- `action_data`——存脚本文件**绝对路径**（区别于内联脚本的 magic comment + 代码）
- `is_system = 0`——用户可删除

Packages 不再是运行时扫描的独立体系，而是**导入 DB 的普通菜单项**。浮窗只用 `list_action_bar_items`，不需扫描 extensions 目录。

### 4.2 内联 vs Package 的区分

`execute_action_bar_inner` 的 script 分支通过 `action_data` 格式区分：

```rust
fn load_script_source(action_data: &str) -> String {
    // 绝对路径 → Package 脚本文件，读文件内容
    if action_data.starts_with('/') {
        std::fs::read_to_string(action_data).unwrap_or_default()
    } else {
        // 内联脚本（DB 项）——action_data 本身就是 magic comment + 代码
        action_data.to_string()
    }
}
```

Package 脚本执行时额外设置环境变量：
- `OCTOPUS_PACKAGE_DIR`——Package 文件夹绝对路径（脚本可读取同目录资源）

### 4.3 扩展元信息查询

扩展包信息（`version`/`description`/`skill`）从 `~/.octopus/extensions/*/config.yaml` 读取（导入时校验，不存 DB）：

```rust
#[tauri::command]
fn list_extensions() -> Result<Vec<ExtensionInfo>, String>;

struct ExtensionInfo {
    dir_name: String,         // 文件夹名
    name: String,             // config.yaml name
    description: String,
    version: String,
    author: String,
    has_skill: bool,
    skill_ref: Option<String>,
    // 关联的 DB 记录
    db_item_id: Option<i64>,  // 已导入则为 DB id
    parent_id: Option<i64>,
}
```

---

## 5. 加载时机

- **App 启动时**扫描 `~/.octopus/extensions/`（仅验证文件夹存在，不加载到菜单——菜单走 DB）
- **设置页菜单编辑**保存时触发安装（`install_extension` 复制到 extensions）
- **浮窗唤出**时只读 DB，不触发文件 IO

---

## 6. ZIP 导入

### 6.1 导入方式（集成进菜单编辑）

扩展包不再有独立子页，而是集成进菜单编辑流程：

1. 设置页 → 命令面板 → 新增子项 → 类型选「扩展包」
2. EditForm 出现拖拽区 + 「选择 zip 文件」/ 「选择文件夹」按钮
3. 拖入 zip/文件夹 或点击选择 → `import_extension` 校验 + 重复检测
4. 校验通过 → 自动填充 title/actionData/isAsync
5. 点击保存 → `install_extension` 复制到 extensions + 创建 DB 记录
6. 取消或 X 清除 → 仅清表单（无脏数据）

### 6.2 校验流程

```text
用户拖入 zip/文件夹/选择文件
  → import_extension(source_path)
  → zip 解压到临时目录（文件夹直接用）
  → 校验 config.yaml + 必填字段 + 脚本文件存在
  → 检测 extensions/<dir_name>/ 是否已存在（重复报错）
  → 返回 ImportResult { name, sourcePath, dirName, ... }
  → 前端填充表单 actionData="sourcePath|dirName"
```

### 6.3 安装流程（保存时）

```text
用户点保存
  → install_extension(sourcePath, dirName, name, ...)
  → 复制到 ~/.octopus/extensions/<dir_name>/
  → 清理临时目录（zip 解压的）
  → INSERT action_bar_items（action_data = 脚本绝对路径）
```

---

## 7. 设置页 UI（集成进菜单编辑）

扩展包不再有独立子页。菜单编辑表单 `actionType=extension` 时显示拖拽区（`ExtensionDropZone` 组件）：

- **拖拽**——zip 文件或含 config.yaml 的文件夹（Tauri `onDragDropEvent`）
- **选择文件**——系统文件选择器（zip）
- **选择文件夹**——系统目录选择器（`directory: true`）
- **X 清除**——仅清表单（校验阶段不复制文件，无脏数据）
- **保存**——调 `install_extension` 复制到 extensions + 创建 DB

EditForm 改为单页导航（非弹窗）：左上角 ArrowLeft 返回、无边框、垂直排列。
菜单项单击 = 展开/收起 submenu；编辑用独立 Pencil 按钮；删除二次确认。

---

## 8. spawn_script 适配

`spawn_script` 需适配 Package 脚本（文件路径 vs 内联）：

```rust
// execute_action_bar_inner script 分支调用前，先加载脚本源码
let source = if item.action_data.starts_with('/') {
    // Package 脚本——读文件
    std::fs::read_to_string(&item.action_data).unwrap_or_default()
} else {
    // 内联脚本——action_data 本身就是源码
    item.action_data.clone()
};

// spawn_script 时额外设置 OCTOPUS_PACKAGE_DIR
if item.action_data.starts_with('/') {
    let pkg_dir = std::path::Path::new(&item.action_data)
        .parent()
        .map(|p| p.to_string_lossy().to_string());
    if let Some(dir) = pkg_dir {
        cmd.env("OCTOPUS_PACKAGE_DIR", dir);
    }
}
```

---

## 9. 不变量

1. **Package 必须含 `config.yaml`**——无 config 的文件夹被忽略
2. **`action.script` 是相对路径**——指向 Package 内文件，不允许绝对路径
3. **DB `action_data` 存绝对路径**——Package 安装后路径固定
4. **校验阶段不复制文件**——导入仅校验 + 重复检测，保存时才复制到 extensions
5. **重复检测**——`extensions/<dir_name>/` 已存在时报错
6. **`OCTOPUS_PACKAGE_DIR` 仅 Package 脚本设置**——内联脚本无此环境变量
7. **Packages 安装后与普通菜单项无运行时差异**——浮窗不区分来源
8. **skill 块纯声明性**——一期不做 agent 调度，仅 config 预留
