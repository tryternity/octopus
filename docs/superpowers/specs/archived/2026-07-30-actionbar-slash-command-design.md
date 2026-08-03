# ActionBar `/` 斜杠命令 — 设计规格

- **日期**：2026-07-30
- **类型**：新功能（行为变更 + 新组件）
- **范围**：ActionBar 命令面板新增 `/` 斜杠命令（类似 Claude/VSCode slash command）
- **动机**：用户在 DB 配置的命令（如 `/tolaria` 触发 agent、`/google hello` 搜索）目前只能通过菜单点击或 Alt 快捷键触发，无法在搜索框用简洁的 `/cmd` 语法直达。引入斜杠命令提供「类终端」的快速命令入口。

## 目标与非目标

**目标**：
1. 输入 `/keyword [params]` 触发配了命令名的菜单项
2. 即时候选列表：输入 `/` 弹出所有可用命令（fuzzy 匹配），上下键选 + 回车执行
3. 复用现有 `trigger_keyword` 字段（语义改为 slash 命令名），不加新表/新列

**非目标**：
- 不做独立于 DB 菜单项的「内置命令系统」（所有命令都是 action_bar_items 表的菜单项）
- 不做 `{{voice}}` 文本替代（need_voice 项无参数时仍走录音路径，不变）
- 不改菜单点击/Alt 快捷键等现有触发方式（`/` 是新增的第三种入口）

## 核心决策（已与用户确认）

| 决策点 | 选择 | 理由 |
|---|---|---|
| 命令范围 | 复用 trigger_keyword，所有 action_type 都可配 | DB 字段已就绪，无需新表 |
| trigger 演进 | **语义改为 slash 命令名**，废弃旧「空格分隔裸关键词」模式 | 单一触发语法，不并存两套 |
| 触发方式 | 即时刻候选列表（输入 `/` 弹候选，fuzzy 匹配） | 类 Claude/VSCode 体验 |
| 参数传递 | 有参数填 `{query}`/`{text}`；无参数用选中文本 | 与现有菜单执行一致 |
| need_voice 项 | 无参数时强制录音（现有路径不变） | 不新增 voice 文本替代路径 |
| 候选展示 | 复用搜索结果区 + 新增 `slash` Tab | 输入 `/` 自动跳到 slash tab |
| 内置命令 | 给搜索引擎 seed 配 trigger_keyword（用户可改） | 不硬编码，seed 可覆盖 |

## 交互增强 v2（2026-07-30 补充，基于 v1 实现后反馈）

v1 已落地（见下文各章）。以下为交互增强，**覆盖** v1 对应决策：

### 增强 1：IME 兼容（`、` 顿号开头）

`/` 在中文拼音输入法下会变成 `、`（顿号）。为保持 `/` 全球惯例 + 解决 IME 干扰，query 开头检测同时认 `/` 和 `、`：

- `strip_prefix('/')` 扩展为「strip `/` 或 `、`」
- **只在开头兼容**——后续字符的 `、` 不特殊处理（正常参数）
- 自动跳 tab 检测：`query.startsWith("/") || query.startsWith("、")`

### 增强 2：候选池扩大（所有菜单项 + 标题匹配）

v1 候选池仅「配了 trigger_keyword 的项」。v2 扩大到**所有菜单项**，匹配维度增加标题：

- 候选池：`is_enabled && action_type != "submenu"` 的所有菜单项（不限 trigger_keyword）
- 匹配：对每项，对 `trigger_keyword`（若有）和 `title` 都做 fuzzy，取最高分
- 命令名精确匹配优先于标题 fuzzy（如 `/google` 优先命中 trigger_keyword=google 而非标题含 "google" 的其他项）
- 候选显示：**菜单项标题**（如「百度」「Google」），携带菜单项 id（不管匹配源是命令名还是标题）

### 增强 3：Tab 补全 + 选中锁定

fuzzy 选中候选后按 Tab：
- 补全：输入框文本变为 `/菜单标题 `（标题 + 空格），光标在空格后
- **锁定选中**：补全后高亮项锁定（selectedItemId 记录菜单项 id），用户输参数期间**不重新搜索候选**（query 变化但 slash 候选列表保持）
- 执行：回车时用**锁定的菜单 id** + 输入框空格后文本作为参数（不从文本解析命令）
- 解锁：用户删除补全的标题（query 不再匹配锁定项）→ 解锁，恢复 fuzzy 候选

### 增强 4：菜单标题字符约束

为支持 Tab 补全（标题作为补全文本，需无歧义），菜单标题加字符约束：

- 允许：中文字母数字、`-`、`_`
- 禁止：空格、其他特殊字符
- 校验：EditForm 保存时校验（同 trigger_keyword 的 UI 校验模式）
- 向后兼容：现有 seed 标题均符合（全为单词/中文词，无空格）

### 执行模型变化（v1 → v2）

| 维度 | v1 | v2 |
|---|---|---|
| 命令来源 | 从 action_data 解析（cmd/params） | 锁定的菜单 id + 输入框参数 |
| 候选显示 | `/trigger_keyword` | 菜单标题 |
| Tab | 无 | 补全标题 + 空格，锁定选中 |
| 标题匹配 | 无 | 所有菜单项标题参与 fuzzy |

> v2 的执行不再依赖从输入文本解析命令名——直接用选中候选携带的菜单 id。参数从输入框「补全标题后的空格」之后解析。这避免了标题含特殊字符的解析歧义，也让 Tab 补全成为可靠的参数输入前置步骤。

## 现状（改造前）

### trigger_keyword 机制（将被替换语义）

- **字段**：`action_bar_items.trigger_keyword`（`db.sql:135`，TEXT NOT NULL DEFAULT ''）
- **旧语义**：输入 `keyword rest`（空格分隔），`rest` 替换 URL 模板 `{query}`/`{text}`
- **旧限制**：只 `action_type="url"` 生效（`menu.rs:80`）
- **旧匹配**：`search_quicklink_keywords`（`menu.rs:68-101`），`splitn(2, whitespace)` 精确匹配首词
- **旧 UI**：`EditForm.tsx:235-252` 只 url 类型显示输入框，placeholder `"tr"`

### Tab 体系

- `TabId = "all" | "apps" | "files" | "bookmarks" | "actions" | "commands"`（`searchTypes.ts:32`）
- `TABS` 数组（`searchTypes.ts:46-53`）定义 6 个 tab + 循环顺序 + 快捷键字符
- `filterByTab`（`searchLogic.ts:68-80`）按 source 过滤：`apps→app` / `files→file` / `bookmarks→bookmark` / `actions→menu` / `commands→command`
- `MenuProvider.matches_tab`（`menu.rs:17-19`）匹配 `"quick" | "actions"`

### 执行入口

- url 类型：`executeSearchResult` case "url"（`index.tsx:564-583`）替换 `{query}`/`{text}` → `open_url`
- menu/agent/ai 类型：`executeSearchResult` case "menu"（`index.tsx:556-562`）→ `executeItem` → `execute_action_bar` / `trigger_agent_voice`
- `execute_action_bar_inner`（`script.rs:342-545`）按 action_type 分流

## 架构（改造后）

```
输入 /google hello
  ↓
前端 query effect 检测 startsWith("/") → setActiveTab("slash")
  ↓
后端 search_stream(tab="slash", query="/google hello")
  ↓ MenuProvider.matches_tab("slash") = true
  ↓ search_slash_commands(query, rows):
  ↓   query 以 / 开头 → 切 cmd="google" + params="hello"
  ↓   fuzzy 匹配 trigger_keyword 非空的菜单项
  ↓   产出 source="slash" 候选（含 params 到 action_data）
  ↓
前端 slash tab 显示候选列表
  ↓ 用户上下键选 + 回车
  ↓
executeSearchResult case "slash":
  ├─ agent needVoice → trigger_agent_voice（忽略 slash params，与直接点击一致）
  └─ 其他（url/ai/script/copy_path/非voice agent）→ execute_action_bar(text=params||选中文本)
       （url 的 action_data 空/模板替换、ai 超时、编码等全由后端 execute_action_bar_inner 处理）
```

> **路径统一（2026-07-31 重构）**：斜杠 DB action_type 动作全走 `execute_action_bar`，
> 后端成为唯一动作处理点。详见
> [ActionBar 路径统一 spec](2026-07-31-actionbar-execute-paths-unification-design.md)。
> 前端不再自己处理 url（删 openUrlTemplate 的 slash 调用 + action_data 空分支）、
> 不再有 ai 前端超时（移到后端 tokio::time::timeout）。
```

## 改动点

### 1. 数据层（无 schema 变更，语义迁移）

**`trigger_keyword` 字段语义改为「slash 命令名」**：
- 旧：裸关键词（`tr hello` 空格触发）
- 新：slash 命令名（`/google hello` 触发）
- 字段名保持 `trigger_keyword`（不改名，避免 DB migration；语义由代码 + 文档定义）

**DB seed 补充**（给搜索引擎配命令名）：
- `db.sql:483-486` 的 Google/百度/Bing/Github url 子项，补 `trigger_keyword`（`google`/`baidu`/`bing`/`github`）
- 用户可在设置 UI 改

### 2. 匹配层（`crates/search/src/providers/menu.rs`）

**删除 `search_quicklink_keywords`**（旧裸关键词逻辑），**新增 `search_slash_commands`**：

```rust
/// Slash 命令匹配：query 以 `/cmd [params]` 模式时，
/// fuzzy 匹配 trigger_keyword 非空的菜单项（所有 action_type），
/// 返回 source="slash" 候选，params 记入 action_data 供执行时用。
fn search_slash_commands(query: &str, rows: &[ActionBarItem]) -> Vec<SearchResult> {
    // query 必须以 / 开头
    let rest = match query.strip_prefix('/') {
        Some(r) if !r.is_empty() => r,
        _ => return vec![],  // 仅 "/" 不触发（或返回全部命令？见下方待定）
    };
    // 切 cmd（/ 后到空格前）+ params（空格后）
    let (cmd, params) = match rest.find(char::is_whitespace) {
        Some(i) => (&rest[..i], rest[i..].trim()),
        None => (rest, ""),  // 无参数
    };
    // fuzzy 匹配 trigger_keyword
    rows.iter()
        .filter(|r| r.is_enabled && !r.trigger_keyword.is_empty())
        .filter_map(|r| {
            let score = match_score(cmd, &r.trigger_keyword)?;
            // action_data 携带 params + 原始 action_type/action_data/id
            Some((score, SearchResult {
                source: "slash".into(),
                title: format!("/{}", r.trigger_keyword),
                subtitle: r.title.clone(),
                icon: None,
                action_type: r.action_type.clone(),  // 保留原类型供执行分流
                action_data: serde_json::json!({
                    "id": r.id,
                    "cmd": r.trigger_keyword,
                    "params": params,  // 可能为空
                    "action_type": r.action_type,
                    "action_data": r.action_data,
                }).to_string(),
                score,
            }))
        })
        .take(10)  // 候选上限
        .collect()
}
```

**`MenuProvider` 改动**：
- `matches_tab` 加 `"slash"`：`matches!(tab, "quick" | "actions" | "slash")`
- `search` 里 `search_quicklink_keywords` → 替换为 `search_slash_commands`
- `search_menus`（标题模糊匹配）不变

**仅 `/` 单字符时的行为**：
- **返回所有配了 trigger_keyword 的菜单项**（完整命令列表，score 一致，按 trigger_keyword 字母序）
- 理由：符合「即时候选」承诺——输入 `/` 即见全部可用命令，用户继续输入字符 fuzzy 缩小范围

**防抖**：slash tab 进即时搜索（不防抖）——命令匹配是纯内存 DB 读（同 menu/actions tab），`DEBOUNCED_TABS`（`index.tsx:40`）不含 slash。

### 3. Tab 体系（前端 `searchTypes.ts` + `searchLogic.ts`）

**新增 `slash` Tab**：
- `TabId` 联合加 `"slash"`
- `TABS` 数组加 `{ id: "slash", label: "斜杠", key: "s" }`（label 用「斜杠」与现有 `commands`「命令」区分；i18n key `slash` 对应「斜杠」/「Slash」）
- `filterByTab` 的 `sourceMap` 加 `slash: "slash"`

**输入 `/` 自动跳 slash tab**：
- `index.tsx` 的 query effect（`:341-360`）加分支：`if (query.startsWith("/")) setActiveTab("slash")`
- 反向：query 清空 `/` 前缀时回到 `all`（或不强制切回，由用户决定）

### 4. 执行层（`index.tsx` executeSearchResult）

**新增 case "slash"**（或在现有 case 基础上分流）：

```ts
case "slash": {
  // 路径统一（2026-07-31）：DB action_type 动作全走 execute_action_bar，
  // 后端是唯一动作处理点。前端只解析 itemId + 构造 text + 选命令。
  // 详见 2026-07-31-actionbar-execute-paths-unification-design.md
  const itemId = slashLockedItemIdRef.current ?? (data.id as number);
  const params = /* 从 query 解析 / data.params */;
  const item = menuItemsRef.current.find((i) => i.id === itemId);
  if (!item) break;
  const text = params || contextRef.current?.text || "";

  // agent needVoice → 联动语音录音（忽略 slash params，与直接点击一致）
  if (item.actionType === "agent" && item.needVoice) {
    await invoke("trigger_agent_voice", { itemId });
  } else {
    // 其他（url/ai/script/copy_path/非voice agent）→ execute_action_bar
    // url 的 action_data 空/模板替换、ai 超时、url 编码全由后端处理
    await invoke("execute_action_bar", { itemId, text });
    invoke("action_bar_dismiss", { reason: "slash-exec" });
  }
  break;
}
```
```

> 注：后端 `execute_action_bar_inner` 的 agent 分支（`script.rs:508-535`）当前 `voice=""`，传入 params 作为 text 即可（params 不直接填 voice，voice 仍走录音或留空）。

### 5. 设置 UI（`EditForm.tsx`）

**放开 trigger_keyword 类型限制**：
- 现 `:235` `type === "url"` 条件去掉，所有类型都能配
- placeholder/label 改为 slash 命名语义（如「`/` 命令名」）
- i18n（`zh-CN.yaml:717-718`）更新文案

### 6. 废弃清理

- `search_quicklink_keywords`（`menu.rs:65-101`）删除
- 相关单测（若有）更新或删除
- 文档（architecture.md / configuration.md）同步 trigger_keyword 新语义

## 不变量

1. **现有菜单点击 / Alt 快捷键 / 普通搜索不受影响**——`/` 是新增入口
2. **need_voice 项的录音路径不变**——无参数 `/cmd` 仍走 `trigger_agent_voice`
3. **url 模板替换机制不变**——`{query}`/`{text}` 占位符语义一致
4. **DB 无 schema 变更**——trigger_keyword 字段复用，仅语义迁移

## 测试策略

### 后端纯函数单测（`menu.rs` 内联 `#[cfg(test)]`）

`search_slash_commands` 覆盖矩阵：
- `/google hello` → 匹配 trigger_keyword=google 的项，params="hello"
- `/google`（无参数）→ 匹配，params=""
- `/goo`（fuzzy）→ 匹配 google（score < 精确匹配）
- `/xyz`（无匹配）→ 空结果
- `/`（仅斜杠）→ 返回全部命令（方案 A）或空（方案 B）
- 非 url 类型（agent/ai）配 trigger_keyword 也能匹配
- query 不以 `/` 开头 → 空结果（不影响普通搜索）

### 前端单测（`searchLogic.test.ts`）

- `filterByTab(results, "slash")` 只留 source="slash"
- Tab 循环含 slash（`getNextTab`）

### 回归

- 现有 `searchLogic.test.ts`（477 行）不破坏
- 现有 quicklink 测试（`action_bar.rs` 的 `ql` 测试）——语义变了，需更新
- tsc + vite build + 手动 e2e（输入 `/` 弹候选、`/google hello` 执行、`/tolaria` 录音）

## 风险与缓解

| 风险 | 缓解 |
|---|---|
| 旧 trigger_keyword 用户语义突变 | 现状无 seed 配 trigger_keyword（全空），影响面小；文档说明 |
| slash tab label 与 commands tab 冲突 | i18n 用区分性 label（「/命令」vs「命令助手」） |
| `/` 前缀与 URL 检测冲突 | url 检测在 url Provider，slash 在 menu Provider，tab 隔离不冲突 |
| fuzzy 匹配命令过多 | take(10) 限制 + score 排序 |

## 文件清单

| 文件 | 操作 |
|---|---|
| `crates/search/src/providers/menu.rs` | 删 `search_quicklink_keywords`，加 `search_slash_commands`；`matches_tab` 加 slash |
| `crates/desktop/frontend/src/pages/ActionBar/searchTypes.ts` | `TabId` 加 slash，`TABS` 加项 |
| `crates/desktop/frontend/src/pages/ActionBar/searchLogic.ts` | `filterByTab` sourceMap 加 slash |
| `crates/desktop/frontend/src/pages/ActionBar/index.tsx` | query effect 自动跳 slash tab；`executeSearchResult` 加 case "slash" |
| `crates/desktop/frontend/src/pages/Settings/ActionBar/EditForm.tsx` | 放开 trigger_keyword 类型限制 |
| `crates/infra/src/db.sql` | 搜索引擎 seed 补 trigger_keyword |
| `crates/desktop/frontend/src/locales/zh-CN.yaml` + `en.yaml` | i18n 文案 |
| `docs/architecture.md` | 同步 trigger_keyword 新语义 + slash 命令说明 |
