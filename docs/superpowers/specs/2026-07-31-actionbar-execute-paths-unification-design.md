# 统一 ActionBar「直接点击」与「斜杠命令」执行路径

> **状态：⏳ 待启动（初稿，启动重构时需 brainstorm 细化）**
> **日期**：2026-07-31
> **关联**：[斜杠命令设计](2026-07-30-actionbar-slash-command-design.md)、memory `project_unify-actionbar-execute-paths`

## 背景：为什么要统一

ActionBar 菜单项的执行当前有**两套独立实现**，按 action_type 分流的逻辑分别存在前后端，容易分裂：

| 路径 | 入口 | 实现位置 |
|---|---|---|
| **直接点击** | `executeItem(item)` → `invoke("execute_action_bar", {itemId, text})` | 后端 `script.rs::execute_action_bar_inner`（单一 switch，按 action_type 分流） |
| **斜杠命令** | `executeSearchResult(result)` → 前端 slash 分流 | 前端 `index.tsx`（部分动作自己处理 + 部分 fallback 到 execute_action_bar） |

### 已踩坑案例（2026-07-31）

**url 类「选中文本即 URL」项**（系统「网页」菜单，`action_data=''`）：
- 后端处理了空 action_data（用选中文本当 URL，缺 scheme 补 https://）→ 直接点击 ✅
- 前端 slash 路径漏了这个分支 → `openUrlTemplate` 的 `if (!url) return` 静默无反应 ❌
- 临时修复（commit `34eb80e4`）：前端补 action_data 空分支对齐后端

**根因**：同一动作语义有两套实现，改一边容易漏另一边。本次只暴露了 url，agent/ai/script/copy_path 等类型的「直接点击 vs 斜杠」一致性**未全面审计**。

## 目标

消除「直接点击」与「斜杠命令」的动作处理分裂，让 action_type 的分流逻辑只有**单一真相源**。

## 范围

### 包含
- 审计所有 action_type（url / agent / ai / script / copy_path / launch_app / open_file / menu）在两条路径下的行为差异
- 选定统一方案并实施
- 补回归测试覆盖「直接点击 == 斜杠命令」的等价性

### 不包含
- ActionBar 搜索/补全逻辑（不变）
- 子菜单展开逻辑（executeItem 的 submenu 分支是 UI 行为，不涉及动作执行分裂）

## 方案（待 brainstorm 定稿，初稿倾向）

### 方案 A（倾向）：斜杠路径统一走后端 execute_action_bar

斜杠命令也构造 `{itemId, text}` 调 `execute_action_bar`，后端 `execute_action_bar_inner` 作为唯一动作处理点。前端删除 `openUrlTemplate` 等重复逻辑，slash 分流只负责「解析 itemId + text 参数」。

**优点**：单一真相源，后端改了前端自动跟随；前端瘦身。
**风险**：某些动作可能依赖前端上下文（如 result 携带的已替换 URL），需审计后端能否拿到等价信息。

### 方案 B：前端补齐所有后端分支

维持双实现但保证逐分支对齐。

**优点**：改动局部，不破坏现有 invoke 契约。
**缺点**：双实现长期维护成本高，易再次分裂（本次 url 就是这么漏的）。**不推荐**。

## 待定问题（启动时 brainstorm）

1. 斜杠路径目前有些动作**故意**在前端处理（如 url 的 `{query}` 替换、agent needVoice 联动语音）——统一到后端后这些怎么承接？
2. `executeSearchResult` 的 `result.actionData`（搜索结果携带的已处理数据，如 quicklink 替换后的 URL）与 DB 原始 `item.action_data` 的差异如何在后端统一表达？
3. 是否所有动作都适合走 `execute_action_bar`，还是有少数必须前端处理（如纯前端 clipboard copy）？

## 测试策略（待细化）

核心回归测试：**同一菜单项 + 同一输入文本，直接点击与斜杠命令产生等价的副作用**（打开相同 URL / 执行相同 agent 命令 / 相同 LLM prompt）。可考虑参数化测试覆盖所有 action_type。

## 触发时机

用户明确：「等我测完 Agent 命令，就需要对齐这个」。即以下修复 e2e 验证通过后启动：
- agent new-tab listener 稳定化（`9f4f5182`）
- 首次 agent 命令复用占位首 tab（`ea27ba4c`）
- url slash action_data 空修复（`34eb80e4`）

## 关键文件

- 前端：`crates/desktop/frontend/src/pages/ActionBar/index.tsx`
  - `executeItem`（~503）—— 直接点击入口
  - `executeSearchResult`（~564）—— 搜索/斜杠执行入口
  - slash 分流（~599-662）—— 前端动作处理（待统一/删除）
  - `openUrlTemplate`（~569）—— 前端重复逻辑（方案 A 下删除）
- 后端：`crates/desktop/src/action_bar/action_bar_commands/script.rs::execute_action_bar_inner`（~342）—— 动作处理单一真相源（方案 A 下保留并扩展）
