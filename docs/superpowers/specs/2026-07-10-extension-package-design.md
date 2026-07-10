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

### 4.1 导入 = 创建 DB 记录

Package 导入时在 `action_bar_items` 表创建一条记录：

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

扩展子页展示的 `version`/`description`/`skill` 信息**不存 DB**，从 `~/.octopus/extensions/*/config.yaml` 实时读取：

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
- **设置页扩展子页**「刷新扩展」按钮手动触发重新扫描
- **浮窗唤出**时只读 DB，不触发文件 IO

---

## 6. ZIP 导入

### 6.1 导入方式

1. **系统打开 `.octopusext.zip`**——Tauri 文件关联，双击 zip 唤起 octopus 自动导入
2. **设置页拖拽 zip**——扩展子页是 drop zone，拖入 `.octopusext.zip` 自动导入

### 6.2 导入流程

```text
用户拖入/双击 my-translator.octopusext.zip
  → 解压到临时目录
  → 读取 config.yaml
  → 校验：
    ✗ config.yaml 缺失 / 必填字段为空 / 脚本文件不存在
      → 红色气泡「导入失败：<原因>」
    ✓ 通过 ↓
  → 移动到 ~/.octopus/extensions/<dir_name>/
  → 弹出父菜单选择器（现有父菜单项列表 + 「顶层」选项）
  → 用户选择父菜单
  → INSERT action_bar_items（parent_id = 用户选择, action_data = 脚本绝对路径）
  → toast「已导入：翻译助手」
  → 刷新扩展列表
```

### 6.3 ZIP 命名约定

- 扩展名 `.octopusext.zip`（不注册自定义 UTI，避免 macOS 复杂度）
- 解压后顶层必须是一个文件夹（含 `config.yaml`），不允许散文件

### 6.4 同名包升级

`~/.octopus/extensions/<dir_name>/` 已存在 → 覆盖（同名包升级）。DB 记录的 `action_data` 路径不变（文件夹名不变则路径不变）。

### 6.5 命令

```rust
#[tauri::command]
fn import_extension(zip_path: String, parent_id: Option<i64>) -> Result<String, String>;
// 解压 + 校验 + 安装 + 创建 DB 记录，返回 Package name

#[tauri::command]
fn list_extensions() -> Result<Vec<ExtensionInfo>, String>;

#[tauri::command]
fn refresh_extensions() -> Result<(), String>;
// 重新扫描（当前 list_extensions 已实时扫描，refresh 供 UI 按钮调用）

#[tauri::command]
fn delete_extension(dir_name: String) -> Result<(), String>;
// 删 DB 记录（action_data 匹配）+ 删 extensions 文件夹
```

---

## 7. 设置页 UI

### 7.1 布局

命令面板 tab header 按钮区新增「扩展」入口：

```
[执行记录] [扩展] [全部展开] [新增主菜单项]
```

点击「扩展」切换到扩展管理子页。

### 7.2 扩展子页

```
┌─────────────────────────────────────────┐
│  扩展包                       [刷新扩展] │
│  ~/.octopus/extensions/                 │
│                                         │
│  拖拽 .octopusext.zip 到此处导入        │
│  ═════════════════════════════════════  │
│                                         │
│  ● 翻译助手          v1.0.0   SKILL     │
│    选中文本翻译为中英互译                │
│    ─ main.py · #python · 同步           │
│    挂载于：工具                          │
│                                         │
│  ● JSON 格式化       v1.2.0             │
│    格式化剪贴板 JSON 并粘贴              │
│    ─ fmt.sh · #shell · 异步             │
│    挂载于：顶层                          │
│                                         │
│  ● （空）                                │
│    还没有扩展包，拖入 zip 或手动放入     │
│    ~/.octopus/extensions/                │
└─────────────────────────────────────────┘
```

### 7.3 每个包卡片

- 名称 + 版本号 + SKILL 徽章（`skill` 块存在时显示）
- 描述
- 脚本文件名 · magic comment 类型 · 异步/同步标签
- 挂载位置（父菜单名 或 「顶层」）
- 点击卡片：展开详情（config.yaml 原文只读 + skill ref 路径 + 资源文件列表）
- 删除按钮（删 DB 记录 + extensions 文件夹，二次确认）

### 7.4 交互

- **拖拽导入**——扩展子页是 drop zone
- **刷新**——重新扫描文件夹
- **删除**——二次确认（删 DB + 文件夹）
- **只读**——不能在此编辑 Package 内容，只能改文件

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
3. **DB `action_data` 存绝对路径**——Package 导入后路径固定
4. **同名文件夹覆盖**——升级 Package 不创建新 DB 记录
5. **`OCTOPUS_PACKAGE_DIR` 仅 Package 脚本设置**——内联脚本无此环境变量
6. **Packages 导入 DB 后与普通菜单项无运行时差异**——浮窗不区分来源
7. **扩展子页元信息从 config.yaml 实时读取**——DB 不存 version/description/skill
8. **skill 块纯声明性**——一期不做 agent 调度，仅前端展示 + config 预留
