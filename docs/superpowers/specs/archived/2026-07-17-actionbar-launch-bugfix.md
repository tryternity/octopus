# Actionbar Launch 集中 bug 审查修复

> 2026-07-17 · 基于 commit `95097685`（翻译分支入口修复）+ actionbar launch 整体链路的两路交叉审查
>
> **状态**：已实现（8 条全部修复）

## 1. 审查范围

针对 actionbar launch 翻译链路 + CompactEditor 翻译对照模式 + activation 焦点协调做集中审查。基线 `95097685` 本身的修复（把 `action_bar_visible` 从「gate 整个翻译分支入口」降级为「只 gate Local 流式路径里的 hide/depth 三行」）正确，让 Quick Execute 翻译也能进入流式路径——但这同时扩大了一个既有竞态的触发面。

审查方式：两路交叉 agent + 逐条磁盘复核。共发现 **1 个 Critical + 5 个 Important + 2 个 Moderate**，全部属实。

## 2. 修复列表

| # | 严重度 | 问题 | 根因 | 修法 |
|---|---|---|---|---|
| 1 | Critical | 流式翻译与开 tab 竞态 → 译文区永久卡 loading | 后端 `run_on_main_thread(open)` + 立即 `spawn(do_translate_streaming)`，前端靠单值 ref 路由，spawn emit 早于 open-tab emit → ref 仍 null → 丢弃事件 | sessionId 路由 + 后端 payload 带 sessionId + 前端 ref 改 Map |
| 2 | Important | quick_execute 读上次 trigger 残留的 PENDING_CONTEXT | PENDING_CONTEXT 仅在 trigger_action_bar 写，quick_execute 不写，AI 动作读陈旧 source/surrounding | quick_execute Text 分支调 gather_context 写新 ctx（`set_pending_context`） |
| 3 | Important | ActionBar 可见时 quick_execute hide 后 CompactEditor 被压后台 | 标准 `hide_action_bar_window`（was_inactive=true 时 `activateWithOptions(prev_app)` 把源 app 拉前台），后台 app 的 set_focus 不激活 | 改 `win.hide()` + `after_floating_window_hide_keep_active` + `finalize_action_bar_pub` 三步，对齐 `action_bar_show_result_internal` |
| 4 | Important | contrast 左栏编辑丢失 | `onOriginalChange` 错调 `updateActiveTextAt` 写幽灵字段 `text`（contrast 模式渲染/保存都不读 text） | 新增 `updateActiveOriginalAt` 写 `originalText` |
| 5 | Important | contrast tab 重译把占位符当源文本 + 覆盖 originalText | `handleTranslateForTab` 的 `sourceText = tab.text`，contrast tab 的 text 是后端脚手架 | sourceText 按 mode 区分——contrast 读 `originalText`，plain 读 `text` |
| 6 | Important | 翻译事件跨窗口泄漏（Result 收到 CompactEditor 事件） | `do_translate_streaming` 用 `app.emit("translate-progress|done")` 全局广播，Result 无过滤订阅 | 拆分事件名：CompactEditor 走 `compact-editor://translate-progress|done`，Result 保留 `translate-progress|done` |
| 7 | Moderate | activation flag 用 macOS 14+ deprecated 的 `1<<1` | Apple 头文件明确 `ActivateIgnoringOtherApps` deprecated 且"will have no effect" | 项目内 3 处 `1<<1` 改 `1<<0`（`after_floating_window_hide` + screenshot 两处） |
| 8 | Moderate | 单窗口并发翻译共享单 `translatingTabKeyRef`，事件错路由 | ref 单值，并发开两个 contrast loading tab 后开者覆盖前者 | ref 改 Map<sessionId, tabKey>，按 sessionId 路由 |

## 3. 核心架构决策（发现 1+6+8 统一方案）

三处 bug 同源——**事件路由不可靠**。统一用 sessionId + 拆分事件名解决：

### 3.1 后端 TranslateEmitTarget

`do_translate_streaming(text, app, target)` 按 target 分发：

```rust
enum TranslateEmitTarget {
    Result,                                  // 旧事件名 translate-progress|done，payload 裸 String
    CompactEditor { session_id: String },    // 新事件名 compact-editor://translate-progress|done，payload { sessionId, text }
}
```

- **事件名彻底隔离**：Result 窗口 ASR 翻译与 CompactEditor 翻译互不干扰，根治跨窗口泄漏
- **sessionId 路由**：CompactEditor 多 tab 并发翻译不冲突，根治错路由
- **payload 带 sessionId**：前端不再依赖 ref 时序，根治竞态

### 3.2 translate_text 命令

`translate_text(text, target_type, app) -> Result<String, String>`：
- `target_type: "compact_editor"` → 生成并返回 sessionId（前端据此建立映射）
- `target_type: "result"` → 走旧事件名，返回空串

### 3.3 TempTabPayload.translate_session_id

后端 execute_action_bar_inner Local 翻译分支生成 sessionId，同时写入 `TempTabPayload.translate_session_id` + 传给 `do_translate_streaming` 的 CompactEditor target。前端 open-tab 事件据此把 `sessionId → tabKey` 写入 `translatingSessionsRef`，无需等翻译事件来路由。

### 3.4 前端 translatingSessionsRef（Map）

```ts
const translatingSessionsRef = useRef<Map<string, string>>(new Map());
```

三个写入点：open-tab handler / pending-take / handleTranslateForTab。
两个读出点：translate-progress / translate-done handler（按 `payload.sessionId` 查 Map）。
done 时 `delete(sessionId)` 而非清空整个 Map（支持并发）。
`translating` 全局状态从 `Map.size === 0` 派生。

## 4. 不变量

1. **事件名隔离**：CompactEditor 翻译只 emit/listen `compact-editor://translate-*`；Result ASR 翻译只 emit/listen `translate-*`。两套互不重叠。
2. **sessionId 全链路一致**：后端生成 → 写 TempTabPayload → 前端 open-tab 写 Map → translate-* 事件 payload 带 sessionId → 前端按 sessionId 路由。任一环节丢失即路由失败（日志可见）。
3. **Map 并发安全**：`translatingSessionsRef` 支持多个 session 并发，互不覆盖。
4. **PENDING_CONTEXT 同步性**：quick_execute 进入前必刷新（gather_context 或仅 text），不允许读到上次 trigger 的残留。
5. **ActionBar 可见时 hide 路径**：quick_execute 必须用 keep_active variant，避免 CompactEditor 被压后台。finalize_action_bar_pub 必须配套调用防 TRIGGER_IN_PROGRESS 残留。
6. **activation flag 全项目统一**：`NSApplicationActivationOptions(1 << 0)`（ActivateAllWindows），不再用 deprecated 的 `1 << 1`。

## 5. 降级路径

- `gather_context` 失败（osascript/lsof 超时等） → PENDING_CONTEXT 仅写 text，source/surrounding=None，`build_enriched_text` 跳过拼接（AI 仍能执行，无上下文标签）。
- sessionId 路由失败（payload.sessionId 不在 Map 中） → 翻译事件丢弃（log），用户重试。
- CompactEditor target emit 失败 → 用户看到译文 loading 永不更新，需关 tab 重触发（同原 bug 现象，但概率极低）。

## 6. 实施记录

详见 git log，分支 `fix/daily-bug-fix-actionbar-launch`，6 个独立 commit：

1. `fix(activation): NSApplicationActivationOptions 统一为 1<<0`（发现 7）
2. `fix(action-hotkey): quick_execute 刷新 PENDING_CONTEXT + keep_active hide`（发现 2+3）
3. `fix(compact-editor): contrast 模式原文编辑 + 重译源绑定 originalText`（发现 4+5）
4. `fix(translate): sessionId 路由 + 拆分事件名根治竞态/泄漏/并发错路由`（发现 1+6+8）

合并了 `fix/action-hotkey-residual-and-fallback`（上次的快捷键残留 + 静默失败修复），让本分支包含完整的 action_hotkey 修复链。

## 7. 验证

- `cargo build --release -p octopus-desktop`：0 error 0 warning
- `cargo test -p octopus-desktop --bin octopus-desktop`：311 passed
- `npx tsc --noEmit`：0 error
- `npm run build`：0 error
- 影响面追踪：`rg "translate-progress|translate-done"` 后端只 2 处 emit（Result 分支），前端 4 处 listen（Result 2 + CompactEditor 2 用新事件名）；`rg "translatingTabKeyRef"` 0 处残留；`rg "1 << 1"` 0 处残留

## 8. 不在本次范围

- 不重构 `do_translate_streaming` 整体结构（只加 target 参数）
- 不改 Result 窗口 ASR 翻译链路（仅确认它仍监听老事件名）
- 不补集成测试（mock AppHandle 成本高，文档钉死不变量即可）
- translating 状态保持全局（任意 session 在跑就 true），不做 per-tab 维度
