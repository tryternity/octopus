# Action Bar App-aware 菜单绑定设计（per-app 绑定）

> **日期**：2026-07-23
> **状态**：✅ 已实现（cargo test 全过 + tsc + vite build 通过；e2e 待用户验证）
> **关联**：DB schema v48→v49；架构文档见 `docs/architecture.md` §「Action Bar」+ §「octopus-search」
> **灵感来源**：[Sidey](https://github.com/chentao1006/Sidey) 的 app-aware assistants（prompt 自带 `apps: [bundleID]` 反向绑定）

---

## 0. 目标与范围

### 0.1 核心目标

给 action_bar 菜单项增加「app 绑定」能力：每个菜单项可绑定 0~N 个 app（bundle_id），唤起时按当前前台 app 过滤——**全局项（未绑定）永远显示 + 当前 app 专属项追加显示**。与现有 `accepts`（text/file/any）维度独立，两者 AND。

用户场景：在 Xcode 里写代码看到「Code Review」菜单项，在 Safari 里看文章看到「Summarize」，在 Warp 终端里看到「解释命令」——不同 app 出现不同命令。

### 0.2 范围

| 包含 | 不包含 |
|---|---|
| `action_bar_items` 加 `app_bundle_ids` 列（JSON 数组） | 类别维度（AppKind 绑定，如「所有终端」） |
| 应用索引（launcher_index）加 `bundle_id` 列 | Quick Execute 全局热键的 app 过滤 |
| 设置页 app 多选器 UI（复用应用索引） | 搜索引擎 MenuProvider 的 app 过滤 |
| 浮窗菜单条 + 搜索结果的 app 过滤 | 自动识别新 app 类别 |

---

## 1. 关键决策（已确认）

| 决策 | 选择 | 理由 |
|---|---|---|
| bundle_id 来源 | 扩展应用索引加 bundle_id 列 | `extract_app_icon` 已在读 Info.plist，加一行 `defaults read CFBundleIdentifier`；多选器需要稳定 key |
| 类别维度 | 本期不做，纯 bundle_id 精确匹配 | 新终端（Warp/Wave）在多选器里搜选中即可，避免重构 classify_app |
| 与 accepts 关系 | 独立 AND | `isItemVisible` 里 accepts 过滤 + app 过滤都通过才显示 |
| 空菜单降级 | 全局 + 当前 app 专属合并 | 全局项永远显示，专属项追加（Sidey 模式） |

---

## 2. 数据模型变更

### 2.1 action_bar_items 加 app_bundle_ids（schema v48 → v49）

```sql
-- crates/infra/src/db.sql action_bar_items 表
app_bundle_ids TEXT NOT NULL DEFAULT ''   -- JSON 数组字符串
```

- 空串 `""` = 全局项，所有 app 都显示（默认值，向后兼容）
- `["com.apple.Safari","com.google.Chrome"]` = 仅在这两个 app 前台时显示
- 存储格式：JSON.stringify 产出的字符串，前端 JSON.parse 容错消费

### 2.2 launcher_index 加 bundle_id（schema v49 同批迁移）

```sql
-- crates/infra/src/db.sql launcher_index 表
bundle_id TEXT NOT NULL DEFAULT ''   -- app 的 CFBundleIdentifier，command 无
```

- 应用索引扫描时 `defaults read <Info.plist> CFBundleIdentifier` 读取
- `AppEntry` struct 加 `bundle_id: String` 字段
- 旧 DB 缓存无 bundle_id → 检测全空时强制 rescan（类似现有 icon 全空检测）

### 2.3 迁移（migrate_v48_to_v49 helper）

v49 迁移同时处理两列（幂等 `PRAGMA table_info` 检查 + `ALTER TABLE ADD COLUMN`）：
- `action_bar_items.app_bundle_ids`：菜单项 app 绑定
- `launcher_index.bundle_id`：应用索引的 CFBundleIdentifier

helper 在 `init_schema` 三个迁移路径调用（v==48 库 / v47→v48 后 / v46→v47→v48 后），确保所有老库都能到 v49。

---

## 3. 过滤逻辑

### 3.1 过滤维度矩阵

菜单项可见性 = `isEnabled` AND `accepts 过滤` AND `app 过滤`：

| 维度 | 数据源 | 逻辑 |
|---|---|---|
| `isEnabled` | DB `is_enabled` 列 | false → 隐藏 |
| `accepts` | DB `accepts` 列（text/file/any） | 与选中类型（context.kind）匹配 |
| `app_bundle_ids` | DB `app_bundle_ids` 列（JSON） | 空=全局显示；非空=前台 bundle_id ∈ 列表 |

**关键**：`accepts=any` 的项以前无条件显示，现在也要叠加 app 过滤（用户可能给 any 项也绑 app）。

### 3.2 前端过滤函数

```ts
function isItemVisibleForApp(item: ActionBarItem, bundleId?: string): boolean {
  const ids = parseAppBundleIds(item.appBundleIds);
  if (ids.length === 0) return true;      // 全局项，永远显示
  if (!bundleId) return false;            // 有绑定但拿不到前台 app → 隐藏专属项（保守）
  return ids.includes(bundleId);
}
```

落点：`ActionBar/index.tsx` 的 `isItemVisible`（现有 accepts 过滤）末尾加 app 过滤 + `contextFilteredResults`（搜索结果二次过滤）同步加。

### 3.3 不受 app 过滤的场景

| 场景 | 原因 |
|---|---|
| Quick Execute（全局热键） | 用户主动配置的「随时可执行」入口，不经过前端 `isItemVisible` |
| 搜索引擎 MenuProvider | 搜索是用户主动行为，搜到了就该能执行 |
| 设置页列表 | 管理员要看到全部菜单项（含绑定的），否则无法编辑 |

---

## 4. 不变量

| # | 不变量 | 落地点 |
|---|---|---|
| INV-A1 | `app_bundle_ids` 为空串 = 全局项，所有 app 显示 | `isItemVisibleForApp` 空数组返回 true |
| INV-A2 | `app_bundle_ids` 非空 = 仅绑定的 app 显示 | `isItemVisibleForApp` 检查 includes |
| INV-A3 | app 过滤与 accepts 过滤独立 AND | `isItemVisible` 两层检查都通过才 true |
| INV-A4 | `app_bundle_ids` 存储为 JSON 数组字符串 | DB 列 TEXT，前端 JSON.stringify/parse |
| INV-A5 | 前台 app bundle_id 拿不到时，专属项隐藏（保守） | `isItemVisibleForApp` bundleId=undefined 返回 false |
| INV-A6 | 全新库存量菜单项 app_bundle_ids 默认空串（全局） | db.sql DEFAULT '' |
| INV-A7 | 应用索引 bundle_id 全空时强制重扫 | `AppIndex::scan` 检测 has_bundle_ids |

---

## 5. UI 设计（设置页 app 多选器）

**使用 frontend-design skill 指导**

编辑表单里新增「绑定应用」区域：
- **已选 chips**：app icon（base64 data URI）+ 名字，chip 上有 × 移除
- **展开浮层**：可搜索的 app 列表（调 `list_all_apps` Tauri 命令拿全量），已选的高亮
- **空状态提示**：「不绑定 = 在所有应用中显示（全局命令）」

---

## 6. 接口

### 6.1 新增 Tauri 命令

| 命令 | 签名 | 用途 |
|---|---|---|
| `list_all_apps` | `() -> Vec<AppBrief>` | 返回全部已索引应用（name + bundle_id + icon），供多选器 |

### 6.2 新增 Rust 类型

```rust
// crates/search/src/engine.rs
pub struct AppBrief {
    pub name: String,
    pub bundle_id: String,
    pub icon: String,  // base64 data URI
}
```

### 6.3 既有命令签名变更

`create_action_bar_item` / `update_action_bar_item` 加 `app_bundle_ids: String` 参数。

---

## 7. 已知限制（不在本期范围）

- **无类别维度**：不支持「绑定终端类」。新终端需用户在多选器手动选中。数据模型预留（app_bundle_ids 是 JSON 数组，未来可加 app_kinds 字段）
- **Quick Execute 不受 app 过滤**：全局热键跳过浮窗直接执行
- **搜索引擎不过滤**：MenuProvider 搜索菜单项不过滤 app
