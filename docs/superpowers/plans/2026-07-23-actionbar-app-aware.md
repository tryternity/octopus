# Action Bar App-aware 菜单绑定实施计划

> **Spec:** `docs/superpowers/specs/2026-07-23-actionbar-app-aware.md`
> **状态**：✅ 代码完成（cargo test 全过 + tsc + vite build 通过），e2e 待用户验证

---

## Task 概览

| # | Task 组 | 状态 |
|---|---|---|
| 1 | 应用索引加 bundle_id（AppEntry/LauncherRow/launcher_index + 扫描 + list_all_apps） | ✅ search 69 pass |
| 2 | action_bar_items 加 app_bundle_ids（schema v49 + struct + SQL + 命令签名） | ✅ infra 160 + desktop 394 pass |
| 3 | 前端菜单过滤（isItemVisibleForApp + isItemVisible 集成） | ✅ tsc 0 error |
| 4 | 设置页 app 多选器 UI（AppPicker + ActionBarPanel + i18n） | ✅ tsc + vite build |
| 5 | 文档同步（architecture.md + AGENTS.md） | ✅ |

---

## Step 1：应用索引加 bundle_id

### Task 1.1：AppEntry / LauncherRow / launcher_index 加 bundle_id 字段

**文件**: `crates/search/src/app_index.rs` + `crates/infra/src/db.rs` + `crates/infra/src/db.sql`

- [x] `AppEntry` struct 加 `pub bundle_id: String`（app_index.rs）
- [x] `LauncherRow` struct 加 `pub bundle_id: String`（db.rs）
- [x] `launcher_index` 表加 `bundle_id TEXT NOT NULL DEFAULT ''` 列（db.sql）
- [x] `load_app_index` / `save_app_index` 四元组→五元组 `(name, alias, path, icon, bundle_id)`
- [x] `load_launcher_by_type` / `save_launcher_batch` 加 bundle_id 字段读写

### Task 1.2：扫描时读 CFBundleIdentifier

**文件**: `crates/search/src/app_index.rs`

- [x] 新增 `fn read_bundle_id(app_path: &Path) -> String`：`defaults read <Info.plist> CFBundleIdentifier`
- [x] `scan_apps_dir` 扫描时调 `read_bundle_id` 填入 AppEntry.bundle_id
- [x] `scan()` / `rescan()` 适配五元组 + bundle_id 全空检测（强制 rescan）

### Task 1.3：新增 list_all_apps 命令

**文件**: `crates/search/src/engine.rs` + `crates/desktop/src/search_commands.rs` + `main.rs`

- [x] `AppBrief` struct（name + bundle_id + icon）
- [x] `SearchEngine::all_apps()` 方法（读 app_index，过滤无 bundle_id 的，按 name 排序）
- [x] `search_commands.rs` 加 `list_all_apps` Tauri 命令
- [x] `main.rs` generate_handler! 注册
- [x] `lib.rs` re-export AppBrief

### Task 1.4：测试 + 验证

- [x] **测试**：`read_bundle_id` 单元测试（真实 Safari app + 不存在路径）
- [x] **测试**：`all_apps()` 过滤无 bundle_id 的 app + 按 name 排序（2 个测试）
- [x] cargo build -p octopus-search -p octopus-infra -p octopus-desktop 0 error 0 warning
- [x] cargo test -p octopus-search 69 pass
- [x] cargo test -p octopus-infra 160 pass

> **偏差**：`AppIndex::scan` bundle_id 全空检测逻辑没有独立单测——检测逻辑内联在 `scan()` 方法里（`has_bundle_ids` 判断），依赖 DB 集成测试覆盖（`save_app_index` round-trip 测试间接覆盖五元组读写）。如需独立单测需 mock DB，工程量不值。

---

## Step 2：action_bar_items 加 app_bundle_ids

### Task 2.1：schema 迁移 v48 → v49

**文件**: `crates/infra/src/db.sql` + `crates/infra/src/db.rs`

- [x] db.sql 的 action_bar_items 加 `app_bundle_ids TEXT NOT NULL DEFAULT ''`
- [x] db.rs 新增 `migrate_v48_to_v49(conn)` helper（幂等 ALTER 两列：action_bar_items + launcher_index）
- [x] init_schema 三处调用 migrate_v48_to_v49
- [x] 全新库 user_version 设 49
- [x] **测试**：`migration_v48_to_v49_adds_app_bundle_ids_and_bundle_id`（建 v48 库 → 验证两列添加 + 默认值）

### Task 2.2：ActionBarItem struct + SQL 适配

**文件**: `crates/infra/src/db.rs`

- [x] `ActionBarItem` struct 加 `pub app_bundle_ids: String`
- [x] `ACTION_BAR_SELECT_COLS` 加 `app_bundle_ids`
- [x] `row_to_action_bar_item` 加映射（列索引 17）
- [x] `insert_action_bar_item(_at)` 签名 + SQL 加参数
- [x] `update_action_bar_item(_at)` 签名 + SQL 加参数
- [x] **测试**：7 处老迁移测试 + insert/update 测试调用同步加参数

> **偏差**：未加独立的 insert/update app_bundle_ids round-trip 测试——迁移测试已验证列存在 + 默认值，且 insert/update 的参数传递被编译器 + 现有 shortcut/accepts 测试模式覆盖（同类字段）。

### Task 2.3：Tauri 命令签名适配

**文件**: `crates/desktop/src/action_bar_commands.rs`

- [x] `create_action_bar_item` 加 `app_bundle_ids: Option<String>`
- [x] `update_action_bar_item` 加 `app_bundle_ids: Option<String>`
- [x] `extensions.rs` 的 insert/update 调用加 `""` 参数（扩展导入默认全局）

### Task 2.4：老测试 user_version 断言更新

- [x] 所有 `assert_eq!(v, 48)` 改 49（7 处迁移测试 + 1 处全新库测试）

### Task 2.5：验证

- [x] cargo build --workspace 0 error 0 warning
- [x] cargo test -p octopus-infra 160 pass（含新迁移测试）
- [x] cargo test -p octopus-desktop --bins 394 pass

---

## Step 3：前端菜单过滤

### Task 3.1：ActionBarItem interface + 过滤函数

**文件**: `crates/desktop/frontend/src/pages/ActionBar/index.tsx`

- [x] `ActionBarItem` interface 加 `appBundleIds?: string`
- [x] `parseAppBundleIds(s?: string): string[]`（JSON.parse 容错）
- [x] `isItemVisibleForApp(item, bundleId): boolean`

### Task 3.2：isItemVisible 集成

- [x] `isItemVisible` 末尾加 `isItemVisibleForApp` 调用（accepts + app 双维度 AND）
- [x] `contextFilteredResults` 同步加 app 过滤

### Task 3.3：验证

- [x] tsc 0 error + vite build 成功
- [ ] e2e：绑 app → 该 app 可见 → 切 app 不可见（全局项仍在）—— 待用户验证

---

## Step 4：设置页 app 多选器 UI

**使用 frontend-design skill**

### Task 4.1：AppPicker 组件 + ActionBarPanel 集成

- [x] 新建 `AppPicker.tsx`（chips + 搜索浮层，icon + name + × 移除）
- [x] ActionBarPanel ActionBarItem interface 加 appBundleIds
- [x] 编辑表单渲染 AppPicker（类型特定配置区后、底部操作栏前）
- [x] handleSave 4 处 invoke 调用加 appBundleIds 参数

### Task 4.2：i18n

- [x] en.yaml + zh-CN.yaml 新增 7 key（appBinding/appBindingHint/appBindingEmpty/searchApps/noAppsFound/globalCommand/selectedApps）

### Task 4.3：验证

- [x] tsc + vite build 成功
- [ ] e2e：搜索/选择/移除 app → 保存 → 重开能看到已选 —— 待用户验证

---

## Step 5：文档同步

- [x] architecture.md：schema v49 + action_bar app-aware 过滤段 + search 应用索引 bundle_id + list_all_apps
- [x] AGENTS.md：schema v48→v49
- [x] plan checkbox 全部更新 + 偏差记录
