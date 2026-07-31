# 统一 ActionBar「直接点击」与「斜杠命令」执行路径

> **状态：✅ 设计定稿（待写 plan + 实现）**
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

**根因**：斜杠命令的问题主要是**没有复用后端、自己实现了一遍**，但又问题很多。同一动作语义有两套实现，改一边容易漏另一边。

### Explore 发现的完整差异（5 个实质分裂点）

| # | action_type | 差异 | 风险 |
|---|---|---|---|
| 1 | **url** | 直接点击全走后端（Rust `percent-encoding`，编码 `!*'()`）；斜杠全走前端（JS `encodeURIComponent`，不编码 `!*'()`） | 编码字符集分裂（罕见字符场景 URL 不同） |
| 2 | **agent needVoice + 有参数** | 直接点击无视参数走 voice；斜杠有参数时不走 voice，落 execute_action_bar（后端 voice 参数恒空，`{{voice}}` 填不上） | 隐蔽语义裂缝 |
| 3 | **ai** | 直接点击有 10s 超时（`executeAiItem`）；斜杠无超时 | 长 LLM 调用卡 loading 无救济 |
| 4 | **text 来源** | 直接点击恒用 `ctx.text`；斜杠用 `params \|\| ctx.text` | 系统性差异 |
| 5 | **前端超时无法取消后端线程** | 前端超时只丢 UI 结果，后端 LLM 线程仍跑 | 资源泄漏 |

## 目标

消除「直接点击」与「斜杠命令」的动作处理分裂，后端 `execute_action_bar_inner` 成为**唯一动作处理点**。前端只负责「解析 itemId + 构造 text + 选命令」。

## 范围

### 包含（DB action_type）
- url / agent / ai / script / copy_path 五类

### 不包含
- **launch_app / open_file / copy_and_reveal / copy**：搜索 Provider 独占的运行时类型（非 DB action_type，executeItem 和后端都不处理），保持现状
- **submenu**：菜单层独占（slash provider 已过滤）
- **agent needVoice + slash 参数**：本次忽略参数（当无参数处理，走 voice）；params 作为 voice text 初值的场景留待后续

## 架构：方案 A——斜杠路径统一走后端 execute_action_bar

斜杠路径的 DB action_type 动作**全部走 `execute_action_bar(itemId, text)`**，后端成为唯一动作处理点。前端只负责两件事：
1. **解析 itemId + params**（从 slash 结果的 `slashLockedItemIdRef` 或 `data.id`）
2. **构造 text**：`text = params || ctx?.text || ""`（slash 参数优先于选中文本）

### 按 action_type 的分流

| action_type | 斜杠路径调什么 | 处理 |
|---|---|---|
| **url** | `execute_action_bar(itemId, text)` | 删前端 `openUrlTemplate`，编码统一到后端 `url_encode_param`（全编码 `!*'()`）。**行为变化**：斜杠 url 编码从 JS → Rust 全编码 |
| **script** | `execute_action_bar(itemId, text)` | 无差异（本就 fallback 后端） |
| **copy_path** | `execute_action_bar(itemId, text)` | 同上 |
| **ai** | `execute_action_bar(itemId, text)` | 后端加超时保护（见下） |
| **agent** + needVoice | `trigger_agent_voice(itemId)`（**忽略 slash params**） | 与直接点击一致（都无视参数走 voice） |
| **agent** + 非 needVoice | `execute_action_bar(itemId, text)` | text = `params \|\| ctx.text` |

### ai 超时移到后端（核心决策）

**现状问题**：前端 `executeAiItem` 的 10s 超时只是 UI 救济——丢迟到结果，但后端 LLM 线程仍在跑（资源泄漏）。斜杠路径根本没超时。

**方案**：后端 `execute_action_bar_inner` 的 ai 分支加 `tokio::time::timeout`：

```rust
// 后端 ai 分支（非 auto_translate）
const AI_TIMEOUT_SECS: u64 = 10;
let result = tokio::time::timeout(
    std::time::Duration::from_secs(AI_TIMEOUT_SECS),
    tokio::task::spawn_blocking(move || {
        octopus_llm::chat_text_with_prompt(&prompt, &enriched_text, &config_clone, None)
    }),
).await;
match result {
    Ok(Ok(res)) => { action_bar_show_result(res, String::new(), item.title, app.clone(), true); Ok(true) }
    Ok(Err(e)) => Err(e2s(e)),                          // LLM 返回错误
    Err(_elapsed) => Err("AI 操作超时（10秒）".into()),  // 超时
}
```

**前端删除**：`executeAiItem` 的 `timedOutRef` + `setTimeout` 机制。超时由后端返回 Err 触发，前端的 catch 统一处理错误展示。auto_translate 的「不超时」判断（`executeAiItem:477-478` 的 `isTranslate`）随之删除——后端 ai 分支已处理 auto_translate 不超时。简化后 `executeAiItem` 失去存在意义（只是普通 invoke 包装），**并入 `executeItem` 的通用 fallback 路径**（ai 类型不再需要前端特殊分流）。

**auto_translate 例外**：保持不超时（本地翻译长文本分段）。后端 ai 分支的 auto_translate 路径不加 timeout。

**收益**：
- LLM 超时后线程真正取消（不泄漏）
- 斜杠 ai 自动获得超时保护
- 前端 `executeAiItem` 可大幅简化

### text 来源统一

两条路径的 text 来源系统性差异，在**调用点**统一：
- 直接点击：`text = ctx?.text || ""`（选中文本）
- 斜杠：`text = params || ctx?.text || ""`（参数优先）

后端 `execute_action_bar_inner` 不动——它只接收 text 参数，不关心来源。前端各自在调用前构造好 text。

### 删除项
- 前端 slash 分流里 url 特殊分支（`index.tsx:634-657` 的 `openUrlTemplate` 调用 + action_data 空处理）——后端 `script.rs:441-449` 已有
- 前端 slash 分流里 agent needVoice 的 `&& !params` 条件（改为始终走 voice）
- 前端 `executeAiItem`（含 `timedOutRef` + `setTimeout` 机制）——ai 超时移后端

**注意**：`openUrlTemplate` helper **函数本身保留**——搜索结果 switch 的 `case "url"`（quicklink/关键词触发，`index.tsx:723`）仍用它。搜索运行时类型按范围决策不纳入本次统一。仅删 slash 分流对它的调用。

## 不变量

1. **后端单一真相源**：DB action_type 的动作处理只在 `execute_action_bar_inner`，前端不自己实现动作逻辑
2. **needVoice 始终走 voice**：needVoice 的 agent 无论直接点击还是斜杠，都走 `trigger_agent_voice`（忽略 slash params）
3. **text 来源在调用点统一**：前端构造好 text 传后端，后端不关心来源
4. **auto_translate 不超时**：保持现状（长文本本地翻译）
5. **搜索运行时类型不受影响**：launch_app/open_file/copy_and_reveal/copy 仍是前端 Provider 独占

## 测试策略

| 层 | 覆盖 | 方式 |
|---|---|---|
| **核心回归**：直接点击 == 斜杠命令等价 | url/ai/script/copy_path/agent 各 action_type | 手动 e2e（对照差异表逐项验证） |
| **url 编码变化** | 含 `!*'()` 的 query | 手动：选中含这些字符的文本，`/google` 触发，看 URL 是否全编码 |
| **ai 超时** | 后端返回 Err | 手动：配慢/无效 LLM endpoint，触发 ai，看 10s 后是否报超时 |
| **单元测试** | 后端 ai 超时常量 + 分支 | 视 octopus-llm 可 mock 性——若不可 mock，靠 e2e |

## 改动文件

| 文件 | 改动 |
|---|---|
| `crates/desktop/src/action_bar/action_bar_commands/script.rs` | ai 分支加 `tokio::time::timeout`（非 auto_translate），超时返回 Err |
| `crates/desktop/frontend/src/pages/ActionBar/index.tsx` | slash 分流简化——删 openUrlTemplate、删 url 空 action_data 处理、删 ai 前端超时；统一走 execute_action_bar / trigger_agent_voice |
| `crates/desktop/frontend/src/pages/ActionBar/index.tsx` | `executeAiItem` 删除（含 timedOutRef 机制），ai 类型并入 executeItem 通用 fallback；auto_translate 不超时判断随之移除（后端 ai 分支已处理） |

## 风险

1. **url 编码行为变化**：斜杠 url 从 JS `encodeURIComponent` → 后端全编码。含 `!*'()` 的 query 会变（更安全，但极少数依赖这些字符不编码的 URL 可能受影响）——已确认接受
2. **auto_translate 不超时**：保持现状，长文本本地翻译仍可能很久——可接受
3. **spawn_blocking 超时取消语义**：`tokio::time::timeout` 对 `spawn_blocking` 的取消——超时后 JoinHandle drop，但线程内的同步 reqwest HTTP 调用无法立即中断（线程继续跑到结束才回收）。**这是已知限制**——超时后前端立即收到 Err，但后端线程可能多跑一会。比现状（前端超时 + 后端永远跑）仍是一大改进。实现时验证 reqwest 是否可用 async client 替代同步调用以支持真正取消。

## 关键文件

- 前端：`crates/desktop/frontend/src/pages/ActionBar/index.tsx`
  - `executeItem`（~503）—— 直接点击入口
  - `executeAiItem`（~469）—— ai 超时机制（待删/简化）
  - `executeSearchResult`（~564）—— 搜索/斜杠执行入口
  - slash 分流（~599-684）—— 前端动作处理（待统一/删除）
  - `openUrlTemplate`（~569）—— 前端重复逻辑（待删）
- 后端：`crates/desktop/src/action_bar/action_bar_commands/script.rs::execute_action_bar_inner`（~342）—— 动作处理单一真相源
  - ai 分支（~425-438）—— 待加超时
  - url 分支（~440-458）—— 不动（已是正确实现）
