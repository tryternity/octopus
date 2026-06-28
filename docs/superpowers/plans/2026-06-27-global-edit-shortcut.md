# 全局编辑快捷键（edit_global_shortcut）实施计划

> 日期：2026-06-27
> 状态：已实施（代码 + 编译验证通过；e2e 待用户桌面环境验证）
> 关联 spec：[2026-06-27-global-edit-shortcut-design.md](../specs/2026-06-27-global-edit-shortcut-design.md)

## 目标

新增全局快捷键 `edit_global_shortcut`（默认 CmdOrCtrl+Shift+E），任意应用聚焦时唤起 `result_window` 并 toggle 编辑（进入/保存），与窗口内 Cmd+Enter 并存。**用户约束：保留 Cmd+Enter 不动。**

## 任务分解（均已实施）

### Task 1：配置层 `edit_global_shortcut` 字段
- [x] `crates/infra/src/config.rs`：`AppConfig` 加 `edit_global_shortcut: String` + `#[serde(default = "default_edit_global_shortcut")]` + `default_edit_global_shortcut()` 返回 `"CmdOrCtrl+Shift+E"` + `impl Default` 同步 + 单测断言。
- [x] `crates/infra/src/db.sql`：`app_config` seed 加 `('edit_global_shortcut', 'CmdOrCtrl+Shift+E', ...)`。
- [x] `crates/infra/src/db.rs`：`load_app_config_at` + `save_app_config_at` 补 `edit_global_shortcut`（显式字段列表，漏则不存 DB / 设置页回退默认；save 长度 25→26）。

### Task 2：后端 handler + 注册
- [x] `crates/desktop/src/result_window.rs`：加 `trigger_global_edit(app)`（show + set_focus + emit `"global-edit-toggle"`）+ `register_edit_global_shortcut(app, str)`（`on_shortcut` → `trigger_global_edit`）。
- [x] `crates/desktop/src/main.rs`：setup 阶段 `asr_shortcut` 注册后加 `register_edit_global_shortcut(app.handle(), &config.edit_global_shortcut)`。

### Task 3：热重载 + 校验
- [x] `crates/desktop/src/settings_commands.rs`：
  - `apply_config_value` 加 `edit_global_shortcut` 分支（字符串校验）。
  - `set_config` 加 `edit_global_shortcut` 热重载（unregister old + register new + 失败恢复 + 持久化）；`old_shortcut` 拆为 `old_asr` + `old_edit_global`。

### Task 4：前端
- [x] `Result/index.tsx`：`toggleEdit` 声明后加独立 `useEffect` listen `"global-edit-toggle"` → `toggleEdit()`（独立 useEffect 规避 TDZ / TS2448）。
- [x] `Settings/GeneralPanel.tsx`：「快捷键」卡片加「全局编辑」行（`ShortcutButton` 组件）；**移除**「编辑模式」（`edit_shortcut`）配置行——Cmd+Enter 固定默认，不再在设置页管理。

### Task 5：构建 + 文档
- [x] `cargo check -p octopus-desktop -p octopus-infra` 通过（仅 1 个 pre-existing dead_code warning，与本次无关）。
- [x] 前端 `npm run build`（tsc + vite）通过，dist 换 bundle（`index-DyUJGfnE.js`）。
- [x] 同步文档：本计划 + spec + `architecture.md`（`result_window` 编辑入口 / `settings_commands` 26 字段 + 热重载 / 快捷键卡片移除编辑模式）。

## 验证清单（e2e，待用户在桌面环境跑）

1. 按默认 `CmdOrCtrl+Shift+E`：`result_window` 唤起到前台 + 进入编辑态（有识别结果时）。
2. 编辑态再按 `CmdOrCtrl+Shift+E`：保存（commit）。
3. 无识别结果时按：只唤起窗口，不进空编辑。
4. 窗口内 `Cmd+Enter` 仍正常（进入/保存 toggle，未受影响）。
5. 设置 → 快捷键 → 全局编辑：键盘捕获改键，热重载即时生效；**改后设置页显示新值**（DB 持久化）；冲突键报错恢复。
6. 重启应用：配置持久化，全局键仍生效（验证 DB 存取修复）。
7. 编辑态按 ESC：放弃编辑（`cancelEdit`，还原原文、不保存）；非编辑态 ESC 放弃录音（原行为）——编辑态需按 2 次 ESC 才放弃录音。保存走 Cmd+Enter 或工具栏「保存编辑」。

## e2e 调试记录：DB category 漏读 bug（2026-06-28）

**现象**：设置页「全局编辑」始终显示默认 `CmdOrCtrl+Shift+E`，但 DB 存的是改后的值（`CmdOrCtrl+Shift+Z`）、改键热重载也生效正确——「更新对、显示错」。

**根因**（纯 DB 数据层，与代码逻辑无关）：
- `app_config.category` 列在**老库**的 DEFAULT 是 `'default'`（`db.sql` 后改为 `'setting'`，但 `CREATE TABLE IF NOT EXISTS` 不更新已存在表的列 DEFAULT；`PRAGMA user_version=7` 的 migration v5→v6 只一次性改了当时的数据行，没改列 DEFAULT）。
- `load_app_config_at` 用 `WHERE category='setting'` 过滤；`save_app_config_at` 的 INSERT 不指定 category（吃列 DEFAULT）+ `ON CONFLICT(config_key) DO UPDATE SET config_value` 只改值不改 category。
- `edit_global_shortcut` 是新加字段，老库无 seed 行 → 首次 `set_config` 以列 DEFAULT=`'default'` 写入 → 被 load 的 `'setting'` 过滤漏读 → 回退 serde 默认 `CmdOrCtrl+Shift+E`。
- 写路径（save 按 `config_key` 匹配，无视图分类）+ 热重载注册（用前端传入的内存值，不经 load）都不受影响 → 所以「更新对、显示错」。

**修复**：手动修开发库——`category` 列 DEFAULT 改 `'setting'` + 既有 `default` 行改回 `setting`。**代码层零改动**（load 严格 `'setting'` 过滤 + `db.sql` DEFAULT `'setting'` 对新库本就正确）。

**教训（未来给 `AppConfig` 加字段必读）**：老库里该字段的 `app_config` 行 `category` 必须是 `'setting'`，否则被 load 漏读 → 设置页回退默认（且「改键生效但显示错」极具迷惑性）。最稳妥：确保 DB schema 列 DEFAULT=`'setting'`，或 seed/migration 显式写 `category='setting'`。

## 不改动

- 窗口内 `edit_shortcut`（Cmd+Enter）**功能**保留（前端 keydown + 字段 default），但**设置页配置行已移除**（固定 Cmd+Enter 不可改）。
- 后端编辑态命令（`enter_edit_mode` / `commit_edit`）+ `handle_enter_edit_mode` / `commit_edit_apply` 逻辑不变。
