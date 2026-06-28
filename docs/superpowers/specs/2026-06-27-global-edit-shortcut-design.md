# 全局编辑快捷键（edit_global_shortcut）设计

> 日期：2026-06-27
> 状态：已实施（代码 + 编译验证通过；e2e 待用户桌面环境验证）
> 关联：clipboard-history-design（窗口管理）、coordinator 编辑态、asr-streaming-token-diagnostic（无关，仅同日）

## 1. 背景 / 动机

用户使用反馈：识别结果落在 `result_window`（always-on-top 浮窗），但用户常在**别的应用**里工作。要编辑识别文本必须先把 `result_window` 切到前台聚焦，再按窗口内 `edit_shortcut`（默认 Cmd+Enter）。这个「先聚焦窗口」的步骤在跨应用场景下很割裂——用户在浏览器/编辑器里，想改刚识别的一句话，得先点出 `result_window`。

现有 `edit_shortcut` 是**窗口内**快捷键：前端 `Result/index.tsx` 的 keydown 监听器匹配 `parseShortcut(edit_shortcut)` → `toggleEdit()`，仅当 `result_window` 聚焦时生效，**无法跨应用**触发。

用户要求：新增一个**全局**快捷键，任意应用聚焦时按一下就能编辑识别区；同时**保留**现有窗口内 Cmd+Enter 不动（用户明确约束：「保留焦点状态下的 CMD + Enter 进入编辑/保存不动」）。

## 2. 设计目标

- **跨应用**：任意应用聚焦时按全局键 → 唤起 `result_window` 到前台 + 进入编辑态。
- **与窗口内 Cmd+Enter 并存**（不替换、不冲突）：Cmd+Enter 继续管「结果窗已聚焦时的编辑 toggle」，全局键管「跨应用唤起 + toggle」。
- **toggle 语义一致**：全局键也是「进入/保存同键」（复用 `toggleEdit`）。
- **可配置 + 热重载 + 冲突检测**，复用现有 `asr_shortcut` 的设置基础设施。
- **空文本保护**：无识别结果时全局键只唤起窗口、不进空编辑。

## 3. 设计

### 3.1 新配置字段 `edit_global_shortcut`

- `AppConfig.edit_global_shortcut: String`（`crates/infra/src/config.rs`），`#[serde(default = "default_edit_global_shortcut")]`，默认 `"CmdOrCtrl+Shift+E"`。
- 默认值选择：与 `asr_shortcut`（`CmdOrCtrl+Shift+Z`）同系列（`CmdOrCtrl+Shift+<字母>`），`E` = Edit 易记；不与 `clipboard_shortcut`（`Alt+V`）/ `edit_shortcut`（`Cmd+Enter`）冲突。
- `db.sql` `app_config` seed 加一行（新安装用户）；老 DB 缺该行时 serde default 兜底（`load_config` 反序列化容错）。
- `impl Default for AppConfig` 同步加字段初始化。

### 3.2 后端：全局快捷键 handler

新增两个函数（`crates/desktop/src/result_window.rs`）：

- `trigger_global_edit(app)`：`show` + `set_focus` `result_window` + `emit("global-edit-toggle", ())`。
- `register_edit_global_shortcut(app, shortcut_str)`：解析 Accelerator → `on_shortcut` 注册，handler（Pressed 时）调 `trigger_global_edit`。与 `shortcut::register_shortcut`（handler 调 `coordinator.toggle()`）的区别仅在此 handler。

注册时机：`main.rs` setup 阶段，紧跟 `asr_shortcut` 注册之后（读 `config.edit_global_shortcut`）。

### 3.3 前端：事件 → toggleEdit

`Result/index.tsx` 在 `toggleEdit` 声明**之后**加独立 `useEffect`，`listen("global-edit-toggle", () => toggleEdit())`。

- **为什么独立 useEffect 而非并入主事件数组**：`toggleEdit` 是 `const`（TDZ），主事件 useEffect（L129）在 `toggleEdit` 声明（L247）之前，前置引用触发 TS2448。独立 useEffect 放声明之后规避。
- 复用 `toggleEdit` = `editingRef.current ? commitEdit() : enterEdit()`：未编辑→进入，已编辑→保存，与窗口内 Cmd+Enter 同语义。
- `enterEdit` 已自带 `if (!displayedRef.current.trim()) return`：无识别结果时全局键只唤起窗口、不进空编辑。

### 3.4 热重载 + 冲突检测（复用 asr_shortcut 模式）

- `settings_commands.rs::set_config`：`edit_global_shortcut` 变更时 `unregister` 旧的 + `register_edit_global_shortcut` 新的，注册成功才持久化，失败恢复旧值并返回 Err（同 `asr_shortcut` 2026-06-21 审查修复）。
- `apply_config_value` 加 `edit_global_shortcut` 分支（字符串校验）。
- `check_shortcut` 通用冲突检测（注册 → 立即注销），设置 UI 键盘捕获时自动复用。

### 3.5 设置 UI

`GeneralPanel.tsx`「快捷键」卡片加「全局编辑」行（`ShortcutButton` 组件），复用 `startShortcutCapture("edit_global_shortcut")` → `check_shortcut` + `setVal`。窗口内「编辑模式」（`edit_shortcut`）配置行已**移除**——Cmd+Enter 固定默认值，不再在设置页管理（功能靠字段 default + 前端 keydown 保留）。

## 4. 边界与权衡

| 场景 | 行为 |
|------|------|
| 无识别结果（Idle / 空文本） | 全局键 show+focus 窗口；`enterEdit` 空文本 return，不进编辑 |
| 录音中（Streaming stage） | 全局键进编辑 → `handle_enter_edit_mode` 硬暂停 ASR（与窗口内 Cmd+Enter 录音中按下同行为） |
| 编辑中再按全局键 | `toggleEdit` → `commitEdit` 保存（toggle 语义） |
| 失焦后按全局键 | show+set_focus 重新激活；若在编辑态则 commit（toggle）——用户失焦后想继续编辑可点回编辑区 |
| 与 asr_shortcut 撞键 | Tauri `on_shortcut` 后注册覆盖/报错；设置 UI 改键时 `check_shortcut` + 注册失败恢复旧值兜底 |
| 编辑态按 ESC（窗口内） | `cancelEdit`：退出编辑 + 还原原文快照 + 不写 DB（放弃编辑）；非编辑态 ESC 仍放弃录音——编辑态需按 2 次 ESC 才放弃录音。保存走 Cmd+Enter / 工具栏「保存编辑」按钮 |

**为什么全局键也 toggle（而非只进入）**：与窗口内 Cmd+Enter 语义一致（用户心智模型统一「编辑键 = 进入/保存」），且复用 `toggleEdit` 零额外代码。

## 5. 不改动 / 持久化

- 窗口内 `edit_shortcut`（Cmd+Enter）**功能**完全保留（前端 keydown + 字段 default），但**设置页配置行已移除**——固定 Cmd+Enter，不再可改（用户要求）。
- `enter_edit_mode` / `commit_edit` 后端命令、`handle_enter_edit_mode` / `commit_edit_apply` 逻辑不变——全局键复用现有编辑态链路。

### 5.1 DB 持久化（`crates/infra/src/db.rs`）

`load_app_config_at` / `save_app_config_at` 是**显式字段列表**（非 serde 全量），每加一个 `AppConfig` 字段必须同步在这两处补行，否则：`set_config` 改了不写 DB（热重载内存生效但重启回退）+ `get_config` 从 DB load 不到该字段 → 回退 serde default（设置页显示默认值，正是本次报告的 bug）。

`edit_global_shortcut` 必须在两处补：
- `load_app_config_at`：字符串字段区 `"edit_global_shortcut" => cfg.edit_global_shortcut = value`
- `save_app_config_at`：`fields` 数组 `("edit_global_shortcut", cfg.edit_global_shortcut.clone())` + 数组长度 `25 → 26`

**隐藏前提（2026-06-28 踩坑）**：`load_app_config_at` 用 `WHERE category='setting'` 过滤，而该行的 `category` 必须真是 `'setting'`。老库 schema 的 `category` 列 DEFAULT 曾为 `'default'`（`db.sql` 后改 `'setting'`，但 `CREATE TABLE IF NOT EXISTS` 不更新老表列 DEFAULT），导致新字段首次 `set_config` 写入时拿到 `'default'` → load 漏读 → 设置页回退 serde 默认值（现象：「改键生效但显示错」）。修复 = 确保 DB 列 DEFAULT=`'setting'` + 既有 `default` 行改回 `setting`。代码层无需改动。
