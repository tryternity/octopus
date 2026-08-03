# 应用感知润色 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans.

**Goal:** 润色时按前台 app 自动选模板（app_bundle_ids 关联）+ 注入 app 上下文到 user prompt。

**Architecture:** prompts 表加 app_bundle_ids + inject_context 两列。focus_tracker 缓存 app name。润色前查路由缓存（bundle_id → prompt_id）选模板。inject_context=1 时 user prompt 头部加「当前应用：名称（类别）」。AppPicker 组件复用 actionbar 现成实现。

**Tech Stack:** Rust + rusqlite + Tauri 2 + React

## Global Constraints

- faithful/user-intent 行为不变（inject_context=0, app_bundle_ids=''）
- 无 app 信息时用默认模板 + 不注入上下文
- active_polish_prompt 仍有效（无 app 匹配时的 fallback）
- {} edited 标记机制不受影响
- 模板路由用缓存（RwLock<HashMap>），CRUD 时 invalidate
- schema version bump（prompts 加 2 列）
- AppPicker 复用 actionbar 现成组件

**Spec:** `docs/superpowers/specs/2026-08-01-app-aware-polish-design.md`

---

## Task 1: DB schema + seed + focus_tracker 缓存 app name

**Files:**
- Modify: `crates/infra/src/db.sql`（prompts 加列 + schema bump）
- Modify: `crates/infra/src/seeds.rs`（seed inject_context 值）
- Modify: `crates/desktop/src/platform/focus_tracker.rs`（缓存 name）

- [x] **Step 1: db.sql prompts 加 2 列 + schema bump**

`crates/infra/src/db.sql` prompts 表（约 line 37）加：
```sql
    app_bundle_ids TEXT NOT NULL DEFAULT '',   -- JSON 数组 ["com.tencent.xinWeChat"]，空=全局
    inject_context INTEGER NOT NULL DEFAULT 0,  -- 0=不注入 app 上下文，1=注入
```
schema version +1。

- [x] **Step 2: seeds.rs inject_context 值**

`crates/infra/src/seeds.rs` load_prompt_seeds 的 INSERT 语句加 inject_context 列：
- faithful (id=1): inject_context=0
- user-intent (id=2): inject_context=0
- app-casual (id=3): inject_context=1

- [x] **Step 3: focus_tracker 缓存 app name**

`crates/desktop/src/platform/focus_tracker.rs`：
- `CACHED_PREV` 从 `Mutex<Option<(i32, String)>>` 改为 `Mutex<Option<(i32, String, String)>>`（pid, bundle_id, name）
- `save_frontmost_pid`：缓存时存 name（frontmost_app 第三项）
- `cached_bundle_id()` 和 `cached_pid()` 适配新 tuple
- 新增 `pub fn cached_app_name() -> Option<String>`

- [x] **Step 4: build + test**

Run: `cargo build -p octopus-infra -p octopus-desktop --features embedded 2>&1 | grep -E "^error|^warning"`
Expected: 可能有 error（seeds.rs INSERT 列数不匹配等）——修到 0 error 0 warning

- [x] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(db): prompts 加 app_bundle_ids + inject_context 列 + focus_tracker 缓存 name"
```

---

## Task 2: prompt.rs + client.rs 加 AppContext 注入

**Files:**
- Modify: `crates/llm/src/prompt.rs`
- Modify: `crates/llm/src/client.rs`

- [x] **Step 1: prompt.rs 加 AppContext struct + 类别映射**

```rust
/// app 上下文（注入 user prompt 头部，仅 inject_context=1 的模板用）。
pub struct AppContext {
    pub name: String,
    pub category: String,  // 空串=无类别
}

/// bundle_id → 类别映射（精简，覆盖典型场景，其余靠 LLM 推断）。
pub fn classify_app_context(bundle_id: &str) -> &'static str {
    match bundle_id {
        b if b.starts_with("com.tencent.xinWeChat") || b.starts_with("com.tencent.qq") => "即时通讯",
        b if b.starts_with("com.microsoft.word") || b.starts_with("com.apple.TextEdit") || b.starts_with("com.apple.Pages") => "文档写作",
        _ => "",
    }
}
```

- [x] **Step 2: regions_prompt 加 app_context 参数**

```rust
pub(crate) fn regions_prompt(regions: &[crate::PolishRegion], app_context: Option<&AppContext>) -> String {
    let mut body = String::new();
    for r in regions {
        if r.preserve { body.push_str(&format!("{{{}}}", r.text)); } else { body.push_str(&r.text); }
    }
    let prefix = match app_context {
        Some(ctx) if !ctx.name.is_empty() => {
            if ctx.category.is_empty() {
                format!("当前应用：{}\n", ctx.name)
            } else {
                format!("当前应用：{}（{}）\n", ctx.name, ctx.category)
            }
        }
        _ => String::new(),
    };
    format!("{}请润色以下语音识别文本：\n{}", prefix, body)
}
```

- [x] **Step 3: user_prompt 加 app_context 参数（对称改）**

同 regions_prompt，加 `app_context: Option<&AppContext>` 参数，头部注入。

- [x] **Step 4: client.rs polish_regions 加 app_context 参数**

```rust
pub fn polish_regions(
    regions: &[PolishRegion],
    config: &CompatibleLlmConfig,
    app_context: Option<&prompt::AppContext>,
) -> Result<String> {
    // ...
    let result = chat_text(
        &prompt::system_prompt(),
        &prompt::regions_prompt(regions, app_context),
        // ...
    )?;
    Ok(strip_edited_markers(&result))
}
```

- [x] **Step 5: 更新测试**

prompt.rs 测试适配新签名（regions_prompt/user_prompt 加 `None` 参数 = 无 app 上下文，等价旧行为）。加 app_context 注入测试。

- [x] **Step 6: build + test**

Run: `cargo build -p octopus-llm && cargo test -p octopus-llm`
Expected: 0 error 0 warning，全过

- [x] **Step 7: Commit**

```bash
git add -A
git commit -m "feat(prompt): AppContext 注入 user prompt 头部 + regions_prompt/user_prompt 加参数"
```

---

## Task 3: 模板路由（缓存 + resolve_polish_prompt）+ coordinator 调用

**Files:**
- Modify: `crates/desktop/src/engine/coordinator/polish.rs`
- Modify: `crates/infra/src/db.rs` 或 `crates/desktop/src/commands/` — 新增路由查询函数

- [x] **Step 1: 新增 resolve_polish_prompt + 路由缓存**

在 coordinator/polish.rs 或新模块：
```rust
use std::collections::HashMap;
use parking_lot::RwLock;

/// 路由缓存：bundle_id → prompt_id。模板 CRUD 时 invalidate。
static ROUTE_CACHE: Lazy<RwLock<HashMap<String, i64>>> = Lazy::new(|| RwLock::new(HashMap::new()));

/// 模板 CRUD 后调，清空缓存。
pub(crate) fn invalidate_route_cache() {
    ROUTE_CACHE.write().clear();
}

/// 按前台 app bundle_id 解析润色模板。有 app 关联→取最新；无→默认。
/// 返回 (prompt_content, inject_context)。
pub(crate) fn resolve_polish_prompt(bundle_id: Option<&str>) -> (String, bool) {
    // 1. 缓存命中
    if let Some(bid) = bundle_id {
        if let Some(&pid) = ROUTE_CACHE.read().get(bid) {
            if let Ok((content, inject)) = fetch_prompt(pid) { return (content, inject); }
        }
    }
    // 2. 查 DB
    let (prompt_id, content, inject) = if let Some(bid) = bundle_id {
        match octopus_infra::db::with_db(|conn| {
            // 查 app_bundle_ids LIKE '%bid%' ORDER BY updated_at DESC LIMIT 1
            conn.query_row(
                "SELECT id, content, inject_context FROM prompts \
                 WHERE category='voice_text_polish' AND app_bundle_ids LIKE ?1 \
                 ORDER BY updated_at DESC LIMIT 1",
                rusqlite::params![format!("%{}%", bid)],
                |row| Ok((row.get(0)?, row.get::<_,String>(1)?, row.get::<_,bool>(2)?)),
            ).ok()
        }).flatten() {
            Some(x) => x,
            None => fetch_default_prompt()?,
        }
    } else {
        fetch_default_prompt()?
    };
    // 3. 写缓存
    if let Some(bid) = bundle_id {
        ROUTE_CACHE.write().insert(bid.to_string(), prompt_id);
    }
    (content, inject)
}
```

- [x] **Step 2: spawn_polish_thread + 最终润色传 app_context**

`spawn_polish_thread` 调用前：
```rust
let bundle_id = crate::platform::focus_tracker::cached_bundle_id();
let app_name = crate::platform::focus_tracker::cached_app_name();
let (content, inject) = resolve_polish_prompt(bundle_id.as_deref());
// 临时 set system prompt（如果模板变了）
crate::llm::set_system_prompt(&content);
let app_context = if inject {
    app_name.map(|name| crate::llm::AppContext {
        name,
        category: crate::llm::classify_app_context(bundle_id.as_deref().unwrap_or("")).to_string(),
    })
} else {
    None
};
// 传给 polish_regions
octopus_llm::polish_regions(&regions, &llm_config, app_context.as_ref())
```

注意：最终润色内联路径（polish.rs:88-92）同样改。

- [x] **Step 3: settings_commands create/update/delete_prompt 调 invalidate_route_cache**

模板 CRUD 后清缓存。

- [x] **Step 4: build + test**

Run: `cargo build -p octopus-desktop --features embedded && cargo test -p octopus-desktop --features embedded`
Expected: 0 error 0 warning，全过

- [x] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(polish): 模板路由（缓存 + resolve_polish_prompt）+ coordinator 传 app_context"
```

---

## Task 4: 前端模板编辑 UI（AppPicker + inject_context 开关）

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Settings/` — 找到润色模板编辑 UI

- [x] **Step 1: 模板编辑表单加 AppPicker + inject_context 开关**

在润色模板编辑（创建/编辑 prompt）表单加：
- `<AppPicker value={app_bundle_ids} onChange={...} />`（复用 `pages/Settings/ActionBar/AppPicker.tsx`）
- inject_context 开关（Toggle/Checkbox）

- [x] **Step 2: create/update prompt Tauri 命令传 app_bundle_ids + inject_context**

前端 invoke create_prompt/update_prompt 时多传这两个字段。

- [x] **Step 3: 后端 create/update_prompt 命令加参数**

`settings_commands.rs` 的 create_prompt/update_prompt 加 app_bundle_ids + inject_context 参数。

- [x] **Step 4: tsc + vite build**

Run: `cd crates/desktop/frontend && npx tsc --noEmit && npm run build`
Expected: 0 error

- [x] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(settings): 模板编辑加 AppPicker + inject_context 开关"
```

---

## Task 5: 全量验证 + 文档同步

- [x] **Step 1: 全量验证**

```bash
cargo build -p octopus-desktop --features embedded 2>&1 | grep -E "^error|^warning|Finished"
cargo test -p octopus-desktop --features embedded 2>&1 | tail -3
cd crates/desktop/frontend && npx tsc --noEmit && npm run build 2>&1 | tail -3
```

- [x] **Step 2: architecture.md 更新**

- [x] **Step 3: spec 加实现状态**

- [x] **Step 4: Commit**

---

## Task 6: e2e 反馈后的 3 项改进（系统模板锁定 + 激活态对齐 + app-casual 改名）

e2e 验证后用户反馈驱动的 3 项改进，均在 Task 1-5 完成后追加。

- [x] **Step 1: 系统内置模板锁定全局——不可配置路由**

3 个 `is_system=1` 模板保持「全局 fallback」角色。前端「路由配置」按钮灰禁（title 提示）+ 卡片不展示 app 关联/注入指示；后端 `update_prompt` 对 `is_system=true` 回写 DB 现有值（防御）。回归测试 `system_prompts_locked_global_routing`。
commit `e73d3ca2`

- [x] **Step 2: 激活态对齐模型管理（CurrentBanner + 绿色按钮）**

去掉红色「激活中」badge；列表顶部加 CurrentBanner（绿色 success 横幅显示当前激活模板名）；卡片「激活」按钮始终显示（当前→绿色「已激活」灰禁，其余→绿色「激活」可点），复用 `ModelRow.tsx` 的 `variant="success"` 风格。
commit `0fa20998`

- [x] **Step 3: app-casual 模板改「场景自适应」——去口语化暗示**

md 提示词 Role + Rules §2/§3 重写（去「保留自然语气」「不过度书面化」等口语化暗示，改按 app 针对性：微信/QQ→口语、Word→书面办公、IDE/agent→条理技术化）。seeds.rs title `口语化整理`→`场景自适应`，dest_name `润色-口语化`→`润色-场景自适应`（文件名同步改名）。
commit `143898a0` + `b7bc26d3`

---

## Self-Review

**Spec coverage:**
- ✅ DB schema（Task 1）
- ✅ focus_tracker 缓存 name（Task 1）
- ✅ AppContext + 注入（Task 2）
- ✅ 模板路由 + 缓存（Task 3）
- ✅ AppPicker UI（Task 4）
- ✅ 文档（Task 5）
- ✅ 系统模板锁定全局（Task 6 Step 1）
- ✅ 激活态对齐模型管理（Task 6 Step 2）
- ✅ app-casual 改「场景自适应」（Task 6 Step 3）

**Type consistency:** AppContext struct 在 prompt.rs 定义、coordinator 构造、polish_regions 消费——签名一致。resolve_polish_prompt 返回 (content, inject)——coordinator 消费一致。

## 实施记录

全部 task 完成，commit 序列（branch daily_bugfix_0730）：
- `0372a571` Task 1：DB schema（prompts 加 app_bundle_ids + inject_context，v54→v55）+ seeds + focus_tracker 缓存 name
- `89b3717b` Task 2：prompt.rs AppContext + classify_app_context + regions_prompt 加 app_context；client.rs polish_regions 接收显式 prompt_content + app_context
- `64e74219` Task 3：coordinator/prompt_route.rs 路由缓存 + resolve_polish_prompt；polish.rs resolve_app_aware_prompt；settings_commands CRUD 扩参 + invalidate
- `1f461c7e` Task 4：Prompts/RouteConfigDialog + PromptsPanel 卡片路由配置按钮 + 新建表单 inject_context Toggle + i18n
- `36178015` Task 5：architecture + spec 实现状态 + plan 实施记录
- `e73d3ca2` Task 6.1：系统内置模板锁定全局——不可配置路由（前端灰禁 + 后端防御 + 回归测试）
- `0fa20998` Task 6.2：激活态对齐模型管理——CurrentBanner + 绿色按钮 + 去 activeBadge
- `143898a0` Task 6.3：app-casual 改「场景自适应」——md 提示词去口语化暗示 + seeds title/description
- `b7bc26d3` Task 6.3：文件名同步改名 润色-口语化→润色-场景自适应

与原 plan 的偏差详见 spec「## 实现状态」。关键：polish_regions 改显式 prompt_content（非 set_system_prompt）、classify_app_context case-insensitive、前端用独立弹窗、提供 dev DB 迁移脚本、系统模板锁定 + 激活态对齐模型管理（Task 6 e2e 反馈驱动）。
