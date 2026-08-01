# 应用感知润色——app 关联模板 + 上下文注入 — 设计规格

- **日期**：2026-08-01
- **类型**：新功能（prompt 路由 + 上下文注入 + DB schema + UI）
- **范围**：润色时按前台 app 自动选模板（app_bundle_ids 关联）+ 注入 app 上下文到 user prompt
- **动机**：app-casual 模板写了「根据场景适配」但 LLM 只能猜；用户希望不同 app 用不同润色风格（微信→口语，Word→正式）。参考 actionbar 的 app_bundle_ids 关联机制 + SayIt 的 AppPromptRule

## 核心设计

### 模板路由（按 app 自动选模板）

润色时：
1. 取前台 app bundle_id（`cached_bundle_id()`）
2. 查 prompts 表：`app_bundle_ids` 包含此 bundle_id 的模板
3. **有匹配** → 取 `updated_at` 最新的（解决一个 app 关联多模板）
4. **无匹配** → 取 `active_polish_prompt`（用户激活的默认模板）

### app 上下文注入

当选中的模板 `inject_context=1` 时，user prompt 头部加 app 信息：

```
当前应用：微信（即时通讯）
请润色以下语音识别文本：
...
```

无类别时仅 app 名称（`当前应用：Code`）。无 app 信息时不注入。

### 类别映射（代码层，精简）

```rust
fn classify_app_context(bundle_id: &str) -> &'static str {
    match bundle_id {
        b if b.starts_with("com.tencent.xinWeChat") || b.starts_with("com.tencent.qq") => "即时通讯",
        b if b.starts_with("com.microsoft.word") || b.starts_with("com.apple.TextEdit") || b.starts_with("com.apple.Pages") => "文档写作",
        _ => "",
    }
}
```

只覆盖微信/QQ/Word/TextEdit/Pages——其余靠 LLM 从 app 名称推断。

## 数据层

### prompts 表加 2 字段

```sql
ALTER TABLE prompts ADD COLUMN app_bundle_ids TEXT NOT NULL DEFAULT '';
ALTER TABLE prompts ADD COLUMN inject_context INTEGER NOT NULL DEFAULT 0;
```

- `app_bundle_ids`：JSON 数组 `["com.tencent.xinWeChat"]`，空=全局（不关联特定 app）
- `inject_context`：0=不注入 app 上下文，1=注入

### seed 值

| id | 模板 | app_bundle_ids | inject_context |
|---|---|---|---|
| 1 | faithful | `''`（全局） | 0 |
| 2 | user-intent | `''`（全局） | 0 |
| 3 | app-casual | `''`（全局） | 1 |

用户自建模板默认 `app_bundle_ids=''`（全局）、`inject_context=1`。

### schema version

bump +1（加 2 列）。无旧用户兼容包袱，开发者手动清 DB。

## 后端改动

### focus_tracker.rs

- `save_frontmost_pid`：缓存 app name（`frontmost_app()` 返回的第三项）。`CACHED_PREV` 从 `(pid, bundle_id)` 改为 `(pid, bundle_id, name)`
- 新增 `cached_app_name() -> Option<String>`

### 润色模板路由

新增 `resolve_polish_prompt(app_bundle_id: Option<&str>) -> (prompt_id, prompt_content, inject_context)`：
- **缓存**：`static ROUTE_CACHE: RwLock<HashMap<String, i64>>`（bundle_id → prompt_id）。命中直接取 prompt_id，查 DB 取 content
- **未命中**：查 DB `SELECT * FROM prompts WHERE app_bundle_ids LIKE '%bundle_id%' ORDER BY updated_at DESC LIMIT 1`
- 无匹配 → 读 `active_polish_prompt` 配置值取默认模板
- 返回模板 id + content + inject_context 标志
- **缓存失效**：`invalidate_route_cache()`——模板 CRUD（create/update/delete）后调，清空整个缓存

调用点：`spawn_polish_thread` + 最终润色内联（润色前解析模板，不再用全局 `system_prompt()`）

### prompt.rs

- `regions_prompt` 加 `app_context: Option<AppContext>` 参数
- 有 app_context 时头部加 `当前应用：{name}（{category}）\n`
- 无 app_context 时行为不变

```rust
pub(crate) struct AppContext {
    pub name: String,
    pub category: String,  // 空串=无类别
}
```

### client.rs

- `polish_regions` 加 `app_context: Option<AppContext>` 参数
- 传给 `regions_prompt`

### coordinator/polish.rs

- `spawn_polish_thread` + 最终润色内联：润色前调 `resolve_polish_prompt` 解析模板
- 读 `cached_bundle_id` + `cached_app_name` 构造 `AppContext`
- 传给 `polish_regions`

### settings_commands.rs

- `create_prompt` / `update_prompt`：加 `app_bundle_ids` + `inject_context` 参数
- `apply_config_value`：无需改（这两个是 prompts 表字段，不走 app_config）

## 前端改动

### 设置页模板编辑

润色模板编辑区加：
- **关联应用**：复用 actionbar 的 `AppPicker` 组件（`pages/Settings/ActionBar/AppPicker.tsx`）——已实现 app 搜索 + 多选（icon + name + bundle_id），调 `list_all_apps` Tauri 命令。空=全局模板（不关联特定 app）
- **注入应用上下文**：开关（inject_context）

### 模板列表展示

模板卡片显示关联的 app（如有），方便用户识别。

## 不变量

1. faithful/user-intent 行为完全不变（inject_context=0，app_bundle_ids=''）
2. 无 app 信息时（前台是 octopus 自身 / 检测失败）→ 用默认模板 + 不注入上下文
3. `active_polish_prompt` 仍有效（无 app 匹配时的 fallback）
4. `{}` edited 标记机制不受影响
5. 防御 strip 不受影响
6. 用户不在 prompts 表配置 app 关联时，行为等价现状（全局默认模板）

## 风险

- **app 名称泄漏到输出**：LLM 把「当前应用：微信」写进润色结果。概率低（指令明确说输出纯文本）。可在 strip 层加防御
- **模板路由缓存**：`RwLock<HashMap<bundle_id, prompt_id>>` 缓存路由结果。首次查 DB 后缓存，后续直接命中。模板 CRUD（create/update/delete）时清缓存（`invalidate_route_cache()`）
- **冷门 app bundle_id**：复用 AppPicker 组件（调 `list_all_apps` 从已安装 app 列选取），用户不需知道 bundle_id
- **schema 变更**：prompts 表加 2 列，bump schema version

## 文件改动

| 文件 | 操作 |
|---|---|
| `crates/infra/src/db.sql` | prompts 表加 app_bundle_ids + inject_context 列；schema version bump |
| `crates/infra/src/seeds.rs` | seed 加 inject_context 值（faithful=0, user-intent=0, app-casual=1） |
| `crates/desktop/src/platform/focus_tracker.rs` | 缓存 app name；新增 cached_app_name() |
| `crates/llm/src/prompt.rs` | AppContext struct；regions_prompt 加 app_context 参数 |
| `crates/llm/src/client.rs` | polish_regions 加 app_context 参数 |
| `crates/desktop/src/engine/coordinator/polish.rs` | 模板路由 resolve_polish_prompt；spawn_polish_thread 传 app_context |
| `crates/desktop/src/commands/settings_commands.rs` | create/update_prompt 加 app_bundle_ids + inject_context |
| `crates/desktop/frontend/src/pages/Settings/` | 模板编辑加关联应用 + 注入开关 |
| `docs/architecture.md` | 更新 |

## 验证

- cargo build + cargo test（schema 变更 + prompt 构造 + 路由逻辑）
- tsc + vite build（前端模板编辑 UI）
- e2e：① 无 app 关联→用默认模板 ② app 关联→自动切模板 ③ inject_context=1→user prompt 含 app 信息 ④ inject_context=0→不注入 ⑤ faithful/user-intent 不受影响 ⑥ 一个 app 多模板→取最新

## 实现状态（2026-08-01）

全部 5 task 已实现并 commit。与原设计的偏差：

1. **`polish_regions` 接收显式 `prompt_content` 而非 `set_system_prompt`**：原设计 Step 3 草案用 `set_system_prompt(&content)` 临时改全局态。实际实现改为 `polish_regions(regions, config, prompt_content: &str, app_context)` 接收显式 prompt_content，内部 `build_system_prompt(prompt_content)` 构造 system prompt。理由：全局 `SYSTEM_PROMPT: RwLock<String>` 在中间润色（spawn_polish_thread）与最终润色（start_final_polish_or_paste 内联线程）并发时会竞态——app A 的路由 set 后，app B 的路由可能覆盖。显式传参彻底隔离，spec §「不再用全局 system_prompt()」的方向一致。

2. **`classify_app_context` case-insensitive**：原设计 `b.starts_with("com.microsoft.word")`（小写）匹配不到真实 bundle_id `com.microsoft.Word`（大写 W）。实现改 `to_ascii_lowercase` 后比小写前缀，覆盖 `com.apple.TextEdit` / `com.apple.Pages` 等真实大小写。

3. **前端用独立「路由配置」弹窗（RouteConfigDialog）而非内联编辑表单**：原设计提「模板编辑 UI」未定形态。实现选独立弹窗（卡片加「路由配置」按钮 → 弹窗内 AppPicker + inject_context Toggle），因 prompt 内容编辑走外部 .md 编辑器，app 关联/inject_context 是 DB 字段需独立 UI 入口。复用 actionbar AppPicker 组件。新建表单也带 inject_context Toggle（默认 true）。

4. **dev DB 迁移提供脚本**：原设计说「无旧用户兼容包袱，开发者手动清 DB」。实现额外提供 `scripts/migrate-db-54-to-55.sh`（ALTER TABLE 加列 + 填系统 seed inject_context + 升 user_version，幂等，保留其他表数据）——开发库已有其他数据（模型/剪贴板/vault），清库代价高，迁移脚本保留数据。init_schema 的 bail 分支不变。

5. **`resolve_polish_prompt` 无关联时不写缓存**：原设计未明。实现中 bundle_id 有但查无关联模板时**不写缓存**（bundle_id 可能是任何未关联 app，写缓存后用户加关联也无法生效——CRUD 时虽全清但中间窗口不准）。仅命中关联模板时写缓存。

测试：infra 158 PASS（含 find_prompt_by_bundle_id 新测试）+ llm 12 PASS（含 AppContext 注入 6 新测试）+ desktop 493 PASS + sync 110 PASS（:memory: fresh DB）；前端 tsc + vite build 0 error。CI 等效（:memory:）全绿。
