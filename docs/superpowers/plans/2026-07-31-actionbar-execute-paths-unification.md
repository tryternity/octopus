# 统一 ActionBar 执行路径 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 斜杠命令路径统一走后端 `execute_action_bar`，消除「直接点击 vs 斜杠」两套实现的分裂。

**Architecture:** 方案 A——前端 slash 分流只负责「解析 itemId + 构造 text + 选命令」，所有 DB action_type 动作处理统一到后端 `execute_action_bar_inner`。ai 超时从前端移到后端（`tokio::time::timeout`）。删除前端 `openUrlTemplate` + `executeAiItem` 的重复逻辑。

**Tech Stack:** Rust（tauri + tokio）+ TypeScript（React），零新依赖。

**Spec:** `docs/superpowers/specs/2026-07-31-actionbar-execute-paths-unification-design.md`

## Global Constraints

- **后端单一真相源**：DB action_type 动作只在 `execute_action_bar_inner` 处理，前端不自己实现动作逻辑
- **needVoice 始终走 voice**：needVoice agent 无论直接点击/斜杠都走 `trigger_agent_voice`（忽略 slash params）
- **text 来源在调用点统一**：直接点击 `ctx?.text || ""`；斜杠 `params || ctx?.text || ""`
- **auto_translate 不超时**：后端 ai 分支的 auto_translate 路径不加 timeout
- **ai 超时常量**：`AI_TIMEOUT_SECS = 10`（复用前端原值）
- **url 编码以后端为准**：`url_encode_param`（Rust `percent-encoding`，全编码 `!*'()`），删前端 `encodeURIComponent`
- **范围**：仅 DB action_type（url/agent/ai/script/copy_path）；launch_app/open_file/copy_and_reveal/copy 是搜索 Provider 独占，不纳入

## File Structure

| 文件 | 责任 | 动作 |
|---|---|---|
| `crates/desktop/src/action_bar/action_bar_commands/script.rs` | ai 分支加超时（`AI_TIMEOUT_SECS` + `tokio::time::timeout`） | 修改 |
| `crates/desktop/frontend/src/pages/ActionBar/index.tsx` | slash 分流简化（删 url 特殊分支 + 改 agent needVoice 条件）+ 删 `executeAiItem`。`openUrlTemplate` **保留**（搜索结果 case "url" 仍用） | 修改 |

**Decomposition 理由**：Task 1（后端 ai 超时）独立可测，是前端删除的前提（删前端超时前必须后端先有）；Task 2（前端重构）依赖 Task 1。每个 Task 产出可独立验证。

---

### Task 1: 后端 ai 分支加超时

**Files:**
- Modify: `crates/desktop/src/action_bar/action_bar_commands/script.rs:425-438`（非 auto_translate 的 ai 分支）

**Interfaces:**
- Produces: `AI_TIMEOUT_SECS: u64 = 10` 常量 + ai 分支超时包裹。Task 2 依赖此（前端删超时的前提）。

**Context:**
- 现状（`script.rs:425-438`）：非 auto_translate 的 ai 操作用 `tokio::task::spawn_blocking` 裸调 `octopus_llm::chat_text_with_prompt`，无超时。
- 先例：`script.rs:251` 的 `wait_with_timeout_secs` 是后端超时模式（script 分支 60s）。
- `octopus_llm::chat_text_with_prompt` 是同步 `pub fn`（`crates/llm/src/client.rs:181`），在 spawn_blocking 里调。
- `tokio::time::timeout` 包 spawn_blocking 的 JoinHandle：超时后 await 返回 `Err(Elapsed)`，spawn_blocking 线程继续跑到结束才回收（已知限制，spec 风险 #3 已记录）。

- [ ] **Step 1: 加 AI_TIMEOUT_SECS 常量**

在 `script.rs` 顶部（其他常量附近，或 `execute_action_bar_inner` 前）加：

```rust
/// AI（LLM 润色/摘要/解释）操作超时秒数。auto_translate 不受此限（长文本本地翻译）。
/// spec 2026-07-31-actionbar-execute-paths-unification：超时从前端移到后端，
/// 避免 LLM 线程泄漏（前端超时只丢 UI 结果，后端线程仍跑）。
const AI_TIMEOUT_SECS: u64 = 10;
```

- [ ] **Step 2: ai 分支加 tokio::time::timeout 包裹**

修改 `script.rs:431-437`（非 auto_translate 的 ai 分支的 spawn_blocking 块），从：

```rust
            // LLM 调用是同步阻塞 HTTP——必须 spawn_blocking 防卡 tokio runtime
            let result = tokio::task::spawn_blocking(move || {
                octopus_llm::chat_text_with_prompt(&prompt, &enriched_text, &config_clone, None)
            }).await
                .map_err(|e| e2s_ctx("LLM 线程异常: {}", e))?
                .map_err(e2s)?;
            action_bar_show_result(result, String::new(), item.title, app.clone(), true);
            Ok(true)
```

改为：

```rust
            // LLM 调用是同步阻塞 HTTP——必须 spawn_blocking 防卡 tokio runtime。
            // tokio::time::timeout 包裹：超时返回 Err（auto_translate 路径上方已 return，不进这里）。
            // 注意：超时后 spawn_blocking 线程无法立即中断（同步 reqwest），会继续跑到结束才回收——
            // 但前端立即收到 Err 释放 UI，比原「前端超时 + 后端永远跑」改进。spec 风险 #3。
            let llm_future = tokio::task::spawn_blocking(move || {
                octopus_llm::chat_text_with_prompt(&prompt, &enriched_text, &config_clone, None)
            });
            match tokio::time::timeout(std::time::Duration::from_secs(AI_TIMEOUT_SECS), llm_future).await {
                Ok(Ok(res)) => {
                    let result = res.map_err(e2s)?;
                    action_bar_show_result(result, String::new(), item.title, app.clone(), true);
                    Ok(true)
                }
                Ok(Err(e)) => Err(e2s_ctx("LLM 线程异常: {}", e)),
                Err(_elapsed) => Err(format!("AI 操作超时（{}秒）", AI_TIMEOUT_SECS)),
            }
```

- [ ] **Step 3: 编译 + 测试**

Run:
```bash
cargo build --release -p octopus-desktop 2>&1 | tail -5
cargo test -p octopus-desktop 2>&1 | tail -5
```
Expected: build 0 error 0 warning；rust test 488 passed（不回归）。

- [ ] **Step 4: Commit**

```bash
git add crates/desktop/src/action_bar/action_bar_commands/script.rs
git commit -m "feat(action-bar): 后端 ai 分支加超时（tokio::time::timeout，10s）

auto_translate 例外（上方已 return）。超时从前端移到后端——避免 LLM 线程泄漏，
且为斜杠路径统一走 execute_action_bar 做准备（前端不再需要自己的超时机制）。"
```

---

### Task 2: 前端 slash 分流统一 + 删重复逻辑

**Files:**
- Modify: `crates/desktop/frontend/src/pages/ActionBar/index.tsx`
  - slash 分流（~599-684）
  - `executeAiItem`（~469-501）+ 其在 `executeItem` 的调用（~526-529）
  - `openUrlTemplate`（~569-580）

**Interfaces:**
- Consumes: Task 1 的后端 ai 超时（前端不再需要自己的超时）。
- Produces: 斜杠路径统一走 execute_action_bar。

**Context:**
- 现状 slash 分流（`index.tsx:599-684`）：url 完全前端处理（`openUrlTemplate`，~634-657）；agent needVoice 条件含 `!params`（~659，导致 needVoice+参数 走后端但 voice 填不上）；其他 fallback execute_action_bar（~670-682）。
- 现状 `executeAiItem`（~469-501）：10s 前端超时（`timedOutRef` + `setTimeout`）+ auto_translate 判断。Task 1 后端有超时后，此函数失去存在意义。
- 现状 `executeItem`（~503-560）：ai 分支调 `executeAiItem`（~526-529）。

- [ ] **Step 1: slash 分流简化——删 url 特殊分支 + 改 agent needVoice 条件**

修改 `index.tsx:629-683`。删掉 url 特殊分支（634-657 整段）+ 改 agent needVoice 条件（去掉 `&& !params`，让 needVoice 始终走 voice）。简化后的 slash 分流：

```typescript
      // 斜杠路径统一走后端（spec 2026-07-31-actionbar-execute-paths-unification）：
      // DB action_type 动作全走 execute_action_bar，后端是唯一动作处理点。
      // 前端只负责：解析 itemId + 构造 text（params 优先于选中文本）+ 选命令。
      // needVoice agent 始终走 trigger_agent_voice（忽略 slash params，与直接点击一致）。
      const actionType = (data.action_type as string) || result.actionType;
      const item = menuItemsRef.current.find((i) => i.id === itemId);
      if (!item) {
        console.warn("[slash] 菜单项未找到:", itemId);
        return;
      }
      const ctx = contextRef.current;
      const text = params || ctx?.text || "";

      // agent needVoice → 联动语音录音（与 executeItem 一致，无视 params）
      if (actionType === "agent" && item.needVoice) {
        setView("loading");
        try {
          await invoke("trigger_agent_voice", { itemId });
        } catch (e) {
          showQuickError(String(e).slice(0, 40));
          setView("main");
        }
        return;
      }

      // 其他（url/ai/script/copy_path/非voice agent）→ execute_action_bar
      setView("loading");
      try {
        await invoke("execute_action_bar", { itemId, text });
        // ai/script 异步结果由后端收口（action_bar_show_result 隐藏浮窗）；
        // url/copy_path 后端 Ok(false) 外层收口。同步 dismiss 兜底。
        invoke("action_bar_dismiss", { reason: "slash-exec" });
      } catch (e) {
        showQuickError(String(e).replace(/^脚本执行失败:\s*/, "").slice(0, 40));
        setView("main");
      }
      return;
```

注意：删掉了原 url 分支的 `fallbackText` / `rawUrl` / action_data 空处理（后端 `script.rs:441-449` 已有）。

- [ ] **Step 2: 确认 openUrlTemplate 的去留（搜索结果仍用）**

grep 确认 `openUrlTemplate` 引用：

```bash
rg -n "openUrlTemplate" crates/desktop/frontend/src/pages/ActionBar/index.tsx
```

**预期结果**：line 723（搜索结果 switch 的 `case "url"`，quicklink/关键词触发）仍引用 `openUrlTemplate`。

**决策**：**保留 `openUrlTemplate` 函数**——搜索结果的 url（搜索 Provider 运行时类型）按范围决策不纳入本次统一，它仍需前端 `openUrlTemplate` 处理。Task 2 Step 1 删 slash 分流时，line 655 的 `openUrlTemplate(rawUrl, ...)` 调用随之删除（slash url 改走后端），但函数本身保留给 line 723 用。

此步无代码改动，只是确认「不删函数」的判断。若 grep 显示 line 723 已不存在或 case "url" 已重构，则重新评估。

- [ ] **Step 3: 删 executeAiItem + 改 executeItem ai 分支**

删 `executeAiItem` 函数（`index.tsx:469-501`）。修改 `executeItem` 的 ai 分支（~526-529），从：

```typescript
    if (item.actionType === "ai") {
      executeAiItem(item);
      return;
    }
```

改为（ai 不再需要前端特殊分流，并入通用 fallback）：

```typescript
    // ai 不再单独分流——后端 execute_action_bar_inner 的 ai 分支自带超时
    // （spec 2026-07-31：超时从前端移到后端，避免 LLM 线程泄漏）
```

即删掉这个 if 块，让 ai 类型落到 `executeItem` 末尾的通用 `execute_action_bar` 调用（~555-559）。

同时删 `timedOutRef`（若仅 executeAiItem 用）+ `AI_TIMEOUT_MS` 常量（若仅 executeAiItem 用）。grep 确认：

```bash
rg "timedOutRef|AI_TIMEOUT_MS" crates/desktop/frontend/src/pages/ActionBar/index.tsx
```
Expected: 若无其他引用则删除其声明。

- [ ] **Step 4: tsc + vitest**

Run:
```bash
cd crates/desktop/frontend
npx tsc --noEmit
npx vitest run
```
Expected: tsc 0 error；vitest 全过（原 428，可能有 keyNavigation.test.ts 引用 executeAiItem 的测试需更新——若报错按测试实际调整）。

- [ ] **Step 5: Commit**

```bash
git add crates/desktop/frontend/src/pages/ActionBar/index.tsx
git commit -m "refactor(action-bar): 斜杠路径统一走 execute_action_bar——删前端重复逻辑

- slash 分流简化：url/ai/script/copy_path 全走 execute_action_bar
- agent needVoice 始终走 trigger_agent_voice（去掉 !params 条件，与直接点击一致）
- 删 openUrlTemplate 的 slash 调用（函数保留给搜索结果 case "url" 用，搜索运行时类型不纳入本次统一）
- 删 executeAiItem（ai 超时移到后端，前端不再需要 timedOutRef + setTimeout）

后端成为 DB action_type 动作处理的唯一真相源。"
```

---

### Task 3: e2e 验证 + 文档同步

**Files:**
- 无代码改动，验证 + 文档

- [ ] **Step 1: 构建 + 手动 e2e**

Run:
```bash
./scripts/build-macos-dmg.sh --no-lto --open
```

手动测试清单（对照 spec 差异表逐项验证等价性）：

**核心回归：直接点击 == 斜杠命令**
1. ✅ url（有模板）：选中「hello world」→ 直接点击 Google vs `/google hello world` → 打开相同 URL
2. ✅ url（空 action_data，「网页」菜单）：选中 `example.com` → 直接点击 vs `/www` → 都打开 https://example.com
3. ✅ ai（润色）：选中文本 → 直接点击润色 vs `/润色` → 都产生润色结果
4. ✅ ai 超时：配无效 LLM endpoint → 触发 ai → 10s 后报「AI 操作超时」（直接点击 + 斜杠都验）
5. ✅ script：选中文件路径 → 直接点击脚本 vs `/脚本名` → 都执行
6. ✅ copy_path：选中文件 → 直接点击 vs `/copy_path 命令名` → 都复制路径
7. ✅ agent（非 voice）：直接点击 vs `/agent` → 都在终端启动 agent 命令
8. ✅ agent（needVoice）：直接点击 vs `/voice_agent` → 都触发语音录音（斜杠 params 被忽略）

**url 编码变化验证**
9. ✅ 选中含 `!*'()` 的文本（如 `test!`）→ `/google` → URL 中 `!` 应被编码为 `%21`（后端全编码）

- [ ] **Step 2: 更新 slash-command spec**

Modify `docs/superpowers/specs/2026-07-30-actionbar-slash-command-design.md`：斜杠分流改为统一走 execute_action_bar（替换之前补的 url action_data 空处理描述——现在由后端处理，前端不分支）。

- [ ] **Step 3: 更新 architecture.md**

Modify `docs/architecture.md`：ActionBar 执行路径描述更新——斜杠命令也走 execute_action_bar，后端单一真相源。

- [ ] **Step 4: 移除 TODO 锚点 + 更新 memory**

- 删 `index.tsx` 的 TODO 注释（~630-633，重构已完成）
- 更新 memory `project_unify-actionbar-execute-paths` 标记为 ✅ 已完成

- [ ] **Step 5: Commit**

```bash
git add docs/ crates/desktop/frontend/src/pages/ActionBar/index.tsx
git commit -m "docs(sync): ActionBar 路径统一完成——更新 spec/architecture/memory"
```

- [ ] **Step 6: Review plan（强制——回看偏差）**

实现完成后回到本 plan，把实际偏差（如 Step 2 的 openUrlTemplate 是否被 case "url" 保留、ai 超时的 spawn_blocking 取消语义实测）回写到对应 Task。

---

## Self-Review 记录

**Spec 覆盖检查**：
- ✅ 方案 A（斜杠走后端）→ Task 2 Step 1
- ✅ ai 超时移后端 → Task 1
- ✅ 删 openUrlTemplate 的 slash 调用（函数保留给搜索结果 case "url" 用）→ Task 2 Step 1/2
- ✅ 删 executeAiItem → Task 2 Step 3
- ✅ needVoice 始终走 voice（去 !params）→ Task 2 Step 1
- ✅ url 编码后端为准 → Task 2 Step 1（删前端 url 分支，编码自然落到后端）
- ✅ 范围限定 DB action_type → Task 2 未动搜索运行时类型
- ✅ auto_translate 不超时 → Task 1（auto_translate 上方已 return，不进超时块）

**类型一致性**：
- `AI_TIMEOUT_SECS: u64 = 10` —— Task 1 定义、使用一致
- `execute_action_bar(itemId, text)` —— Task 2 slash 分流调用签名与后端一致
- `trigger_agent_voice(itemId)` —— Task 2 调用与后端命令一致

**已知实现注意**（非占位符）：
- Task 2 Step 2 的 `openUrlTemplate`：需 grep 确认 case "url"（搜索结果 switch，~696-704）是否仍引用——若是则保留函数（只删 slash 分流的调用）。这是实现时要核实的点，plan 已标注。
- Task 2 Step 3 的 `timedOutRef` / `AI_TIMEOUT_MS`：需 grep 确认无其他引用再删声明。
- Task 3 Step 1 第 9 项的 url 编码验证：`!` 编码为 `%21` 是后端 `url_encode_param` 行为，实测确认。
