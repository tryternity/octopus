# 润色路由命中可视化 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 润色开始时浮窗「润色中」文案携带命中的模板名 + 前台 app 名，让用户感知应用感知路由生效。

**Architecture:** `prompt_route::resolve_polish_prompt` 返回值加 `template_title` + `route_hit` 元数据；`polish.rs::resolve_app_aware_prompt` 改返回结构体（含模板名/app名/route_hit）；最终润色路径 `show_result` 文案用 `polish_status_text()` helper 拼接。时序调整：解析提前到 show 之前。零前端改动。

**Tech Stack:** Rust（coordinator/prompt_route + polish 模块）

## Global Constraints

- 文案只在常规模式 `show_result` 显示；instant 模式（`show_instant`）不变
- 中间润色（`spawn_polish_thread`，mode=2）不显示路由提示（现状不变，本就不弹浮窗文案）
- `polish_regions` 的 content/app_context 传递逻辑不变（只多了展示用元数据）
- perf 打点不受影响（仍记录 source/bundle/prompt_id/title）
- 解析失败（空 prompt 降级）显示 `⏳ 润色中`（不带模板名）

**Spec:** `docs/superpowers/specs/2026-08-01-polish-route-visibility-design.md`

---

## Task 1: `ResolvedPrompt` 加 template_title + route_hit 字段

**Files:**
- Modify: `crates/desktop/src/engine/coordinator/prompt_route.rs`

**Interfaces:**
- Produces: `ResolvedPrompt { content: String, inject_context: bool, template_title: String, route_hit: bool }`——Task 2 的 `resolve_app_aware_prompt` 消费

- [x] **Step 1: 扩展 `ResolvedPrompt` 结构体**

`crates/desktop/src/engine/coordinator/prompt_route.rs` 当前 `ResolvedPrompt` 定义（约 line 25-30）：
```rust
pub(crate) struct ResolvedPrompt {
    /// 模板规则文本（已从文件读出）
    pub content: String,
    pub inject_context: bool,
}
```
改为：
```rust
pub(crate) struct ResolvedPrompt {
    /// 模板规则文本（已从文件读出）
    pub content: String,
    pub inject_context: bool,
    /// 模板显示名（用于浮窗「润色中」文案；降级时为空串）
    pub template_title: String,
    /// 是否命中 app 关联模板（true=显示 app 名；false=默认模板，只显示模板名）
    pub route_hit: bool,
}
```

- [x] **Step 2: `resolve_record` 填充新字段**

`resolve_record`（约 line 64-69）当前：
```rust
fn resolve_record(rec: &octopus_infra::db::PromptRecord) -> ResolvedPrompt {
    ResolvedPrompt {
        content: read_prompt_file(&rec.content),
        inject_context: rec.inject_context,
    }
}
```
改为（`template_title` 取 `rec.title`；`route_hit` 作参数传入）：
```rust
fn resolve_record(rec: &octopus_infra::db::PromptRecord, route_hit: bool) -> ResolvedPrompt {
    ResolvedPrompt {
        content: read_prompt_file(&rec.content),
        inject_context: rec.inject_context,
        template_title: rec.title.clone(),
        route_hit,
    }
}
```

- [x] **Step 3: `resolve_polish_prompt` 三个调用点传 route_hit**

`resolve_polish_prompt`（约 line 36-58）有三处调 `resolve_record` + 一处降级 `ResolvedPrompt`。改为：

cache-hit 分支（约 line 40-42）：
```rust
        if let Some(&cached_id) = ROUTE_CACHE.read().unwrap().get(bid) {
            if let Some(rec) = load_record(cached_id) {
                perf_log_route("cache-hit", Some(bid), rec.id, &rec.title);
                return resolve_record(&rec, true);
            }
        }
```

db-hit 分支（约 line 45-49）：
```rust
        if let Ok(Some(rec)) = octopus_infra::db::find_prompt_by_bundle_id(bid) {
            ROUTE_CACHE.write().unwrap().insert(bid.to_string(), rec.id);
            perf_log_route("db-hit", Some(bid), rec.id, &rec.title);
            return resolve_record(&rec, true);
        }
```

default 分支（约 line 53-57）：
```rust
    let default_id = octopus_infra::db::load_active_prompt_id().unwrap_or(1);
    match load_record(default_id) {
        Some(rec) => {
            perf_log_route("default", bundle_id, rec.id, &rec.title);
            resolve_record(&rec, false)
        }
        None => {
            crate::core::perf_log::log(&format!(
                "[POLISH] route source=default bundle={:?} (load_record 失败，空 prompt 降级)", bundle_id
            ));
            ResolvedPrompt {
                content: String::new(),
                inject_context: false,
                template_title: String::new(),
                route_hit: false,
            }
        }
    }
```

- [x] **Step 4: build + test**

Run: `cargo build -p octopus-desktop --features embedded 2>&1 | grep -E "^error|^warning|Finished"`
Expected: 0 error 0 warning（`resolve_record` 签名改了但所有调用点都在同文件内，Step 3 已全改）。若有 `prompt_route` 之外的 `resolve_record` 调用（grep 确认无），一并改。

Run: `cargo test -p octopus-desktop --features embedded 2>&1 | grep -E "test result|FAILED" | tail -3`
Expected: 全过（本 task 无新测试，字段扩展不破坏现有行为）。

- [x] **Step 5: Commit**

```bash
git add crates/desktop/src/engine/coordinator/prompt_route.rs
git commit -m "feat(prompt-route): ResolvedPrompt 加 template_title + route_hit 字段"
```

---

## Task 2: `resolve_app_aware_prompt` 返回结构体 + `polish_status_text` helper

**Files:**
- Modify: `crates/desktop/src/engine/coordinator/polish.rs`

**Interfaces:**
- Consumes: Task 1 的 `ResolvedPrompt { content, inject_context, template_title, route_hit }`
- Produces: `ResolvedAppPrompt` 结构体 + `polish_status_text()` helper——Task 3 的最终润色路径消费

- [x] **Step 1: 定义 `ResolvedAppPrompt` 结构体**

在 `polish.rs` 靠近 `resolve_app_aware_prompt`（约 line 255）之前定义：
```rust
/// resolve_app_aware_prompt 的解析结果。content + app_context 移入 spawn 线程供 polish_regions 用；
/// template_title / app_name / route_hit 是展示用元数据（浮窗「润色中」文案）。
struct ResolvedAppPrompt {
    content: String,
    app_context: Option<octopus_llm::AppContext>,
    /// 模板显示名（降级时空串）
    template_title: String,
    /// 前台 app 名（route_hit=true 时用于文案）
    app_name: Option<String>,
    /// 是否命中 app 关联模板
    route_hit: bool,
}
```

- [x] **Step 2: 重写 `resolve_app_aware_prompt` 返回结构体**

当前（约 line 261-274）返回 `(String, Option<octopus_llm::AppContext>)`。改为返回 `ResolvedAppPrompt`：
```rust
fn resolve_app_aware_prompt() -> ResolvedAppPrompt {
    let bundle_id = crate::platform::focus_tracker::cached_bundle_id();
    let resolved = super::prompt_route::resolve_polish_prompt(bundle_id.as_deref());
    let app_name = crate::platform::focus_tracker::cached_app_name();
    let app_context = if resolved.inject_context {
        app_name.as_ref().map(|name| octopus_llm::AppContext {
            name: name.clone(),
            category: octopus_llm::classify_app_context(bundle_id.as_deref().unwrap_or(""))
                .to_string(),
        })
    } else {
        None
    };
    ResolvedAppPrompt {
        content: resolved.content,
        app_context,
        template_title: resolved.template_title,
        app_name,
        route_hit: resolved.route_hit,
    }
}
```
注意：`app_name` 现在只读一次（之前在闭包里读两次），存到结构体里复用。

- [x] **Step 3: 加 `polish_status_text` helper**

紧跟 `resolve_app_aware_prompt` 之后加：
```rust
/// 拼接浮窗「润色中」文案：命中 app 关联→「⏳ 润色中 · 模板名（app名）」；
/// 默认→「⏳ 润色中 · 模板名」；降级（空模板名）→「⏳ 润色中」。
fn polish_status_text(r: &ResolvedAppPrompt) -> String {
    if r.route_hit {
        if let Some(ref app) = r.app_name {
            return format!("⏳ 润色中 · {}（{}）", r.template_title, app);
        }
    }
    if r.template_title.is_empty() {
        "⏳ 润色中".to_string()
    } else {
        format!("⏳ 润色中 · {}", r.template_title)
    }
}
```

- [x] **Step 4: 更新两处 `resolve_app_aware_prompt` 调用点的解构**

两处调用点（最终润色 line 88 + 中间润色 line 226）当前是：
```rust
let (prompt_content, app_context) = resolve_app_aware_prompt();
```
最终润色路径（line 88）改为：
```rust
let resolved_prompt = resolve_app_aware_prompt();
```
中间润色路径（line 226）同样改为：
```rust
let resolved_prompt = resolve_app_aware_prompt();
```

- [x] **Step 5: 修正两处的 `polish_regions` 调用（用结构体字段）**

最终润色路径 spawn 内联（约 line 92）当前：
```rust
                let inner = || match octopus_llm::polish_regions(&regions, &llm_config, &prompt_content, app_context.as_ref()) {
```
改为：
```rust
                let inner = || match octopus_llm::polish_regions(&regions, &llm_config, &resolved_prompt.content, resolved_prompt.app_context.as_ref()) {
```

中间润色路径 spawn 内联（约 line 233）当前：
```rust
        let result = match octopus_llm::polish_regions(&regions, &llm_config, &prompt_content, app_context.as_ref()) {
```
改为：
```rust
        let result = match octopus_llm::polish_regions(&regions, &llm_config, &resolved_prompt.content, resolved_prompt.app_context.as_ref()) {
```

- [x] **Step 6: build**

Run: `cargo build -p octopus-desktop --features embedded 2>&1 | grep -E "^error|^warning|Finished" | tail -5`
Expected: 0 error 0 warning。`resolved_prompt` 在 spawn 闭包内 move（结构体整体 move 进闭包，字段 `.content`/`.app_context` 借用 OK）。若有 borrow 报错（闭包借 `resolved_prompt.content` 又 move 整个结构体），改为在 spawn 前先 `let content = resolved_prompt.content.clone();` 拆出——但通常 `&resolved_prompt.content` 在 `move` 闭包里因 `resolved_prompt` 也被 move 而合法。

- [x] **Step 7: 加 `polish_status_text` 单测**

在 `polish.rs` 的 `#[cfg(test)] mod tests`（若不存在则在文件末尾加）加测试：
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polish_status_text_route_hit_with_app() {
        let r = ResolvedAppPrompt {
            content: String::new(), app_context: None,
            template_title: "场景自适应".into(), app_name: Some("微信".into()), route_hit: true,
        };
        assert_eq!(polish_status_text(&r), "⏳ 润色中 · 场景自适应（微信）");
    }

    #[test]
    fn polish_status_text_route_hit_no_app_name() {
        // route_hit=true 但 app_name=None（focus_tracker 未缓存到 name）→ 退化为只显示模板名
        let r = ResolvedAppPrompt {
            content: String::new(), app_context: None,
            template_title: "场景自适应".into(), app_name: None, route_hit: true,
        };
        assert_eq!(polish_status_text(&r), "⏳ 润色中 · 场景自适应");
    }

    #[test]
    fn polish_status_text_default_template() {
        let r = ResolvedAppPrompt {
            content: String::new(), app_context: None,
            template_title: "忠实校对".into(), app_name: Some("微信".into()), route_hit: false,
        };
        assert_eq!(polish_status_text(&r), "⏳ 润色中 · 忠实校对");
    }

    #[test]
    fn polish_status_text_empty_title_degraded() {
        let r = ResolvedAppPrompt {
            content: String::new(), app_context: None,
            template_title: String::new(), app_name: None, route_hit: false,
        };
        assert_eq!(polish_status_text(&r), "⏳ 润色中");
    }
}
```

注意：先 grep `polish.rs` 是否已有 `#[cfg(test)] mod tests`——若有，把测试加进去（`use super::*;` 可能已存在）；若无，新建。`ResolvedAppPrompt` 字段需在测试可见（已是模块内私有 fn 用的 struct，同模块测试可访问）。

- [x] **Step 8: test + Commit**

Run: `cargo test -p octopus-desktop --features embedded polish_status 2>&1 | tail -8`
Expected: 4 个 polish_status_text 测试全过。

```bash
git add crates/desktop/src/engine/coordinator/polish.rs
git commit -m "feat(polish): resolve_app_aware_prompt 返回结构体 + polish_status_text 文案 helper"
```

---

## Task 3: 最终润色路径时序调整 + show 文案携带模板信息

**Files:**
- Modify: `crates/desktop/src/engine/coordinator/polish.rs`（`start_final_polish_or_paste`，约 line 57-90）

**Interfaces:**
- Consumes: Task 2 的 `resolve_app_aware_prompt() -> ResolvedAppPrompt` + `polish_status_text()`

- [x] **Step 1: 把 `resolve_app_aware_prompt()` 提前到 `show_result` 之前**

`start_final_polish_or_paste` 当前结构（约 line 57-88）：
```rust
        Some(llm_config) => {
            // 进入异步润色状态
            crate::ui::tray::update_tray_label(app_handle, crate::ui::tray::TrayState::Processing);
            if super::INSTANT_MODE.load(std::sync::atomic::Ordering::Relaxed) {
                crate::ui::result_window::show_instant(app_handle, "polishing", "");
            } else {
                crate::ui::result_window::show_result(app_handle, "⏳ 最终润色中...");
            }
            // ... (取 transcript 字段、建 Stage::Polishing) ...
            let session_id = id;
            // 应用感知：在 coordinator 线程解析模板
            let resolved_prompt = resolve_app_aware_prompt();
            std::thread::spawn(move || { ... });
```

调整为：`resolve_app_aware_prompt()` 移到 `show_result` **之前**，`show_result` 文案改用 `polish_status_text`。instant 模式不变：
```rust
        Some(llm_config) => {
            // 进入异步润色状态
            crate::ui::tray::update_tray_label(app_handle, crate::ui::tray::TrayState::Processing);
            // 应用感知：在 coordinator 线程解析模板（focus_tracker 缓存反映 op-start app）。
            // 提前到 show 之前——让「润色中」文案能携带命中的模板名。
            let resolved_prompt = resolve_app_aware_prompt();
            if super::INSTANT_MODE.load(std::sync::atomic::Ordering::Relaxed) {
                crate::ui::result_window::show_instant(app_handle, "polishing", "");
            } else {
                crate::ui::result_window::show_result(app_handle, &polish_status_text(&resolved_prompt));
            }
            // ... (取 transcript 字段、建 Stage::Polishing) ...
            let session_id = id;
            std::thread::spawn(move || { ... });
```

注意：删掉原 line 88 的 `let resolved_prompt = resolve_app_aware_prompt();`（已提前）。保留 `std::thread::spawn` 闭包内对 `resolved_prompt.content` / `resolved_prompt.app_context` 的引用（Task 2 Step 5 已改）。

- [x] **Step 2: build + test**

Run: `cargo build -p octopus-desktop --features embedded 2>&1 | grep -E "^error|^warning|Finished" | tail -5`
Expected: 0 error 0 warning。注意 `show_result` 第二参数是 `&str`，`polish_status_text` 返回 `String`——传 `&polish_status_text(&resolved_prompt)` 即可（临时值借引用在 show_result 调用期间有效）。

Run: `cargo test -p octopus-desktop --features embedded 2>&1 | grep -E "test result|FAILED" | tail -3`
Expected: 全过。

- [x] **Step 3: Commit**

```bash
git add crates/desktop/src/engine/coordinator/polish.rs
git commit -m "feat(polish): 最终润色 show_result 文案携带命中的模板名 + app 名"
```

---

## Task 4: 全量验证 + 文档同步

- [x] **Step 1: 全量验证**

```bash
cargo build -p octopus-desktop --features embedded 2>&1 | grep -E "^error|^warning|Finished"
cargo test -p octopus-desktop --features embedded 2>&1 | grep -E "test result|FAILED" | tail -3
```
Expected: 0 error 0 warning，全测试过。

- [x] **Step 2: spec 加实现状态**

`docs/superpowers/specs/2026-08-01-polish-route-visibility-design.md` 末尾加：
```markdown
## 实现状态（2026-08-01）

已实现方案 A（文案携带）。commit 序列见 plan「## 实施记录」。
```

- [x] **Step 3: plan 加实施记录 + checkbox 全勾**

- [x] **Step 4: architecture.md 同步**

`docs/architecture.md` 应用感知润色子节补一句：浮窗「润色中」文案携带命中的模板名 + app 名（route_hit=true 时）。

- [x] **Step 5: Commit**

```bash
git add -A
git commit -m "docs(sync): 路由命中可视化 architecture + spec + plan 同步"
```

---

## Self-Review

**Spec coverage:**
- ✅ `ResolvedPrompt` 加字段（Task 1）
- ✅ `resolve_app_aware_prompt` 返回结构体（Task 2）
- ✅ `polish_status_text` helper（Task 2，4 单测覆盖 4 个文案分支）
- ✅ 最终润色 show 文案（Task 3）
- ✅ 时序调整 resolve 提前（Task 3 Step 1）
- ✅ instant 模式不变（Task 3 注释明确）
- ✅ 中间润色不改（Task 2 Step 4 仅改解构 + Step 5 改字段引用，不调 show_result）
- ✅ 降级显示「⏳ 润色中」（Task 2 Step 3 helper + Step 7 测试）
- ✅ 文档同步（Task 4）

**Type consistency:**
- `ResolvedPrompt` Task 1 定义 4 字段（content/inject_context/template_title/route_hit）→ Task 2 `resolve_app_aware_prompt` 读取这 4 字段 ✓
- `ResolvedAppPrompt` Task 2 定义 5 字段 → Task 3 `polish_status_text(&resolved_prompt)` 消费 template_title/app_name/route_hit ✓
- `resolve_record(rec, route_hit)` Task 1 Step 2 签名 → Task 1 Step 3 三处调用传 true/true/false ✓
- `polish_regions(&regions, &config, &resolved_prompt.content, resolved_prompt.app_context.as_ref())` Task 2 Step 5 两处一致 ✓

**Placeholder scan:** 无 TBD/TODO，每步含完整代码 + 命令 + 预期输出 ✓

## 实施记录

全部 4 task 完成，commit 序列（branch daily_bugfix_0730）：
- `0441bfb1` Task 1：ResolvedPrompt 加 template_title + route_hit 字段
- `bddbeebd` Task 2：resolve_app_aware_prompt 返回 ResolvedAppPrompt 结构体 + polish_status_text helper（4 单测）+ 两处 polish_regions 调用适配
- `7927a91b` Task 3：最终润色路径 resolve_app_aware_prompt 提前到 show_result 前 + show_result 文案改用 polish_status_text；移除 #[allow(dead_code)]
- （Task 4：本文档同步）

验证：build 0 error 0 warning，desktop 495 passed（含 4 个 polish_status_text 新测试）。无偏差（实现与 spec 方案 A 完全一致）。
