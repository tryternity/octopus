# 全局立即润色快捷键（polish_global_shortcut）设计

> 日期：2026-06-28
> 状态：设计中（待实施）
> 关联：[global-edit-shortcut-design](./2026-06-27-global-edit-shortcut-design.md)（同模式复刻）、coordinator `PolishNow`、result_window 窗口管理

## 1. 背景 / 动机

用户已有三个全局快捷键：`asr_shortcut`（语音识别）、`edit_global_shortcut`（语音编辑）、`clipboard_shortcut`（剪贴板浮窗）——任意应用聚焦时跨应用触发。

现在缺一个「立即润色」的全局入口。现有立即润色只能通过结果窗工具栏的「立即润色」按钮（前端 `invoke("polish_now")`）触发，**必须结果窗聚焦**。用户在浏览器/编辑器工作时，想对刚识别的文本立即润色（不等 `polish_mode` 自动润色、也不切到结果窗点按钮），得先把结果窗切到前台——跨应用场景下割裂。

用户要求：新增第四个全局快捷键，任意应用聚焦时按一下就对当前识别结果立即润色。**复刻 `edit_global_shortcut` 的模式**（config 字段 + result_window handler + 热重载 + 前端事件 + 设置 UI 行）。

## 2. 设计目标

- **跨应用**：任意应用聚焦时按全局键 → 对当前识别结果立即润色（等价工具栏「立即润色」/ `polish_now`）。
- **与工具栏按钮一致**：复用同一润色逻辑（loading 状态 + toast 反馈 + 幂等门控），不另起一套。
- **仅显示不聚焦**：唤起结果窗 `show` 让用户看到润色结果，但**不抢键盘焦点**（不 `set_focus`）——不打断当前应用输入。
- **空文本保护 + 幂等**：无识别结果时静默无操作；润色进行中再按幂等忽略。
- **可配置 + 热重载 + 冲突检测**，复用现有快捷键设置基础设施。

## 3. 设计

### 3.1 新配置字段 `polish_global_shortcut`

- `AppConfig.polish_global_shortcut: String`（`crates/infra/src/config.rs`），`#[serde(default = "default_polish_global_shortcut")]`，默认 `"CmdOrCtrl+Alt+S"`。
- 默认值选择：与同批调整的 `asr_shortcut`（`CmdOrCtrl+Alt+A`）/ `edit_global_shortcut`（`CmdOrCtrl+Alt+E`）/ `clipboard_shortcut`（`CmdOrCtrl+Alt+D`）统一为 `CmdOrCtrl+Alt+<字母>` 系列（S=润色、A=ASR、E=Edit、D=剪贴板），窗口内 `edit_shortcut`（`Cmd+Enter`）不冲突。
- `db.sql` `app_config` seed 加一行（新安装用户）；老 DB 缺该行时 serde default 兜底。
- `impl Default for AppConfig` 同步加字段初始化 + 单测断言默认值。

### 3.2 后端：全局快捷键 handler

新增两个函数（`crates/desktop/src/result_window.rs`），复刻 `trigger_global_edit` / `register_edit_global_shortcut`：

- `trigger_global_polish(app)`：`show` 结果窗（**不 `set_focus`**，区别于 edit_global）+ `emit("global-polish-trigger", ())`。
- `register_polish_global_shortcut(app, shortcut_str)`：解析 Accelerator → `on_shortcut` 注册，handler（Pressed 时）调 `trigger_global_polish`。

注册时机：`main.rs` setup 阶段，紧跟 `register_edit_global_shortcut` 之后（读 `config.polish_global_shortcut`）。

**为什么 show 不 set_focus**：润色是后端动作（`polish_now` 发 `Command::PolishNow`），不需要窗口聚焦接收键盘输入（编辑才需要）。用户在别的应用输入时按润色键，`show` 让其看到结果，不 `set_focus` 避免抢走当前输入焦点。

### 3.3 前端：事件 → polishNow

`Result/index.tsx` 把现有 polish-now 按钮 onClick 的逻辑抽成 `polishNow`（`useCallback`），按钮与全局事件共用：

```ts
const polishNow = useCallback(async () => {
  if (polishLoading) return;                    // 进行中幂等忽略
  if (!displayedRef.current.trim()) return;     // 无结果静默
  setPolishLoading(true);
  try { await invoke("polish_now"); showToast("润色中…"); }
  catch (e) { setPolishLoading(false); showToast("润色失败：" + e); }
}, [polishLoading, showToast]);
```

- polish-now 工具按钮 `onClick` 改用 `polishNow`（行为零差异）。
- 新增独立 `useEffect`（在 `polishNow` 声明之后）：`listen("global-polish-trigger", () => polishNow())`。
  - **独立 useEffect 规避 TDZ**：与 `global-edit-toggle` 同理（`polishNow` 是 `const`，主事件 useEffect 在其声明之前，前置引用触发 TS2448）。

**无结果时的窗口行为（方案 a，已选定）**：后端 `trigger_global_polish` 无条件 `show + emit`；前端 `polishNow` 判空 return 不润色。即无结果时结果窗被 show（`#container` opacity:0，视觉几乎无害），但不触发润色——与 `edit_global`（无结果 show 窗不进编辑）完全对称。文本在前端 `displayedRef`，后端判不了空，故采用「后端无条件 show + 前端判空」的对称模式，不另加 show command（舍去方案 b 的额外往返）。

### 3.4 热重载 + 冲突检测（复用 asr/edit_global 模式）

- `settings_commands.rs::set_config`：`polish_global_shortcut` 变更时 `unregister` 旧的 + `register_polish_global_shortcut` 新的，注册成功才持久化，失败恢复旧值并返回 Err（同 `asr_shortcut` / `edit_global_shortcut`）。`old_shortcut` 拆分为 `old_asr` + `old_edit_global` + `old_polish_global`。
- `apply_config_value` 加 `polish_global_shortcut` 分支（字符串校验）。
- `check_shortcut` 通用冲突检测（注册 → 立即注销），设置 UI 键盘捕获时复用。

### 3.5 设置 UI

`GeneralPanel.tsx`「快捷键」卡片加「立即润色」行（`ShortcutButton` 组件），复用 `startShortcutCapture("polish_global_shortcut")` → `check_shortcut` + `setVal`。快捷键卡片现有行：语音识别 / 剪贴板浮窗 / 语音编辑，新增「立即润色」（位置紧跟语音编辑之后）。

## 4. 边界与权衡

| 场景 | 行为 |
|------|------|
| 无识别结果（Idle / 空文本） | 后端 show 结果窗（透明，opacity:0 视觉无害）+ emit；前端 `polishNow` `trim()` 判空 return，不润色（与 edit_global 对称） |
| 润色进行中（polishLoading）再按 | 前端 `polishLoading` 门控 return，幂等忽略（与工具栏按钮 disabled 一致） |
| 录音中（Streaming）按 | `polish_now` 对当前 transcript 触发润色（与工具栏按钮录音中按下同行为） |
| 与其它快捷键撞键 | Tauri `on_shortcut` 后注册覆盖/报错；设置 UI 改键时 `check_shortcut` + 注册失败恢复旧值兜底 |
| 结果窗当前隐藏 | `show` 让窗口可见，润色完成后 `update-result` 显示润色文本 |

**为什么只 show 不 set_focus**：润色不需窗口接收键盘（区别于编辑），不抢焦点 = 不打断用户当前应用输入。

## 5. 不改动 / 持久化

- 工具栏「立即润色」按钮**功能不变**，仅 onClick 改抽出的 `polishNow`（行为零差异）。
- `polish_now` 后端命令、`Command::PolishNow`、coordinator 润色逻辑不变——全局键复用现有润色链路。
- `polish_mode`（自动润色）不受影响——全局键是手动立即润色入口，与自动模式独立。

### 5.1 DB 持久化（`crates/infra/src/db.rs`）

`load_app_config_at` / `save_app_config_at` 是显式字段列表，每加一个 `AppConfig` 字段必须同步补行（漏则 `set_config` 不写 DB + `get_config` load 不到 → 设置页回退 serde default）。

`polish_global_shortcut` 必须在两处补：
- `load_app_config_at`：`"polish_global_shortcut" => cfg.polish_global_shortcut = value`
- `save_app_config_at`：`fields` 数组 `("polish_global_shortcut", cfg.polish_global_shortcut.clone())` + 数组长度 `26 → 27`

**隐藏前提**（`edit_global_shortcut` 2026-06-28 踩过的坑，详见 [global-edit-shortcut-design §5.1](./2026-06-27-global-edit-shortcut-design.md)）：`load_app_config_at` 用 `WHERE category='setting'` 过滤，该行 `category` 必须真是 `'setting'`。老库 `category` 列 DEFAULT 曾为 `'default'`，新字段首次 `set_config` 可能拿到 `'default'` → load 漏读 → 设置页回退默认（「改键生效但显示错」）。当前 db.sql DEFAULT=`'setting'` + 既有 migration（`db.rs` `UPDATE ... 'default'→'setting'`）对新装/已迁移库正确；若老库列 DEFAULT 仍是 `'default'`，新字段需确保 seed/migration 写 `category='setting'`。
