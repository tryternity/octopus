# 润色路由命中可视化 — 设计规格

- **日期**：2026-08-01
- **类型**：UX 增强（应用感知润色的可观测性补强）
- **范围**：润色开始时浮窗「润色中」文案携带命中的模板名 + 前台 app 名，让用户感知路由生效
- **动机**：应用感知润色已实现路由（按 app 自动选模板），但用户无法直观看到「这次润色用了哪个模板、是否命中了我绑的 app 关联」。当前只有 `perf_log`（开发者可见），用户不可见。参考用户原诉求：「让用户感知路由生效」。

## 核心设计

润色开始时（`show_result` / `show_instant`），文案从固定的 `"⏳ 最终润色中..."` 改为携带路由信息：

| 路由来源 | 文案形态 | 例子 |
|---|---|---|
| 命中 app 关联模板（db-hit / cache-hit） | `⏳ 润色中 · {模板名}（{app名}）` | `⏳ 润色中 · 场景自适应（微信）` |
| 默认模板（default，无 app 关联） | `⏳ 润色中 · {模板名}` | `⏳ 润色中 · 忠实校对` |
| 默认模板 + inject_context 时 | 同上（模板名后不加 app，因为默认模板可能全局生效） | `⏳ 润色中 · 场景自适应` |
| 解析失败（降级空 prompt） | `⏳ 润色中` | `⏳ 润色中` |

instant 模式（PTT/hands-free）：`show_instant("polishing", "")` 的 text 字段暂不改（instant 是底部小指示卡，空间极小，文案塞不下模板信息）。instant 用户按住键说话，对「用了哪个模板」的感知需求弱。仅常规模式（`show_result`）显示模板信息。

### 为什么不做独立提示条

考虑过「浮窗顶部独立提示条 + 2-3s 淡出」，否决理由：
- 浮窗空间紧张（多行滚动文本区），加提示条挤压主内容
- 新事件 + 前端组件 + 淡出动画，改动大，YAGNI
- 文案携带方案已满足核心诉求（用户润色时看到模板名），且零前端改动

## 数据流

```
coordinator 线程（spawn 前）:
  resolve_app_aware_prompt() → ResolvedAppPrompt {
      content: String,           // 模板规则文本（move 进 spawn 线程）
      app_context: Option<AppContext>,
      template_title: String,    // 新增：模板显示名
      app_name: Option<String>,  // 新增：前台 app 名（命中路由时才有意义）
      route_hit: bool,           // 新增：是否命中 app 关联（true=显示 app 名）
  }
  ↓ 拼接 show 文案
  show_result("⏳ 润色中 · 场景自适应（微信）")
  ↓ move content/app_context 进 spawn 线程
  polish_regions(&regions, &config, &content, app_ctx.as_ref())
```

时序调整：`resolve_app_aware_prompt()` 当前在 `show_result` **之后**调用（最终润色路径）。改为**之前**调用，让 show 文案能用解析结果。解析是廉价操作（DB 读 + 一次文件读），提前无性能影响。

## 后端改动

### `coordinator/polish.rs`

1. **`resolve_app_aware_prompt()` 返回值扩展**：从 `(String, Option<AppContext>)` 改为结构体：
   ```rust
   struct ResolvedAppPrompt {
       content: String,
       app_context: Option<octopus_llm::AppContext>,
       template_title: String,      // 模板显示名（用于 show 文案）
       app_name: Option<String>,    // 前台 app 名（route_hit=true 时用于文案）
       route_hit: bool,             // 是否命中 app 关联模板
   }
   ```

2. **`prompt_route::resolve_polish_prompt` 返回值扩展**：`ResolvedPrompt` 加 `template_title: String` + `route_hit: bool` 字段（app_name 在 polish.rs 从 focus_tracker 取）。

3. **show 文案拼接 helper**：
   ```rust
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

4. **最终润色路径**（`start_final_polish_or_paste`）：`resolve_app_aware_prompt()` 提前到 `show_result` 之前，文案改用 `polish_status_text(&resolved)`。

5. **中间润色路径**（`spawn_polish_thread`）：不调 `show_result`（现状），仅解析后传 spawn 线程。`template_title`/`app_name`/`route_hit` 不显示（中间润色本就不弹浮窗文案）。

### `coordinator/prompt_route.rs`

`resolve_polish_prompt` 内部已打 perf_log（含 source/title）。扩展 `ResolvedPrompt` 结构体加两字段，从 `load_record` 的 `PromptRecord.title` 填 `template_title`，从路由分支（cache-hit/db-hit = true，default = false）填 `route_hit`。

## 不变量

1. 文案只在常规模式 `show_result` 显示；instant 模式不变
2. 中间润色（mode=2）不显示路由提示（现状不变）
3. 解析失败（空 prompt 降级）显示 `⏳ 润色中`（不带模板名）
4. `polish_regions` 的 content/app_context 传递逻辑不变（只多了展示用的元数据）
5. perf 打点不受影响（仍记录 source/bundle/prompt_id/title）

## 风险

- **文案过长挤压浮窗**：`⏳ 润色中 · 场景自适应（微信）` ≈ 18 字符，浮窗默认宽度够。超长 app 名（如「Android Studio」）也仍在合理范围。可在前端 CSS 加 `truncate` 兜底（但本方案不动前端）。
- **模板名含特殊字符**：title 来自 DB（用户可编辑），理论上可控。不做额外转义（show_result 文本原样渲染）。

## 文件改动

| 文件 | 操作 |
|---|---|
| `crates/desktop/src/engine/coordinator/prompt_route.rs` | `ResolvedPrompt` 加 `template_title` + `route_hit` 字段；`resolve_record` / 降级分支填充 |
| `crates/desktop/src/engine/coordinator/polish.rs` | `resolve_app_aware_prompt` 返回结构体；加 `polish_status_text` helper；最终润色路径提前解析 + 文案拼接 |

## 验证

- cargo build + cargo test（结构体字段 + 文案拼接 helper 的单测）
- e2e：① 命中 app 关联→浮窗显示「模板名（app名）」② 默认模板→只显示模板名 ③ 解析失败→只显示「润色中」④ instant 模式不变
