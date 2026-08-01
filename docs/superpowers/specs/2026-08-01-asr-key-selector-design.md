# 快捷键设置重构——asr_shortcut 升级为单键选择器 + 清理废弃快捷键 — 设计规格

- **日期**：2026-08-01
- **类型**：重构（配置语义变更 + UI 改造 + 废弃代码清理）
- **范围**：`asr_shortcut` 从 Tauri 组合快捷键（Alt+A）升级为 handy-keys 单键名（OptRight），PTT 只是该键的三种模式之一；删除 `polish_global_shortcut`（立即润色快捷键）+ `record_mode`（死代码）；`ptt_key` 合并进 `asr_shortcut`
- **动机**：单键三模式（PTT/toggle/hands-free）已落地，原组合快捷键（Alt+A）+ 立即润色快捷键（Alt+S）已被单键取代。设置页需提供单键选择器替代原快捷键 capture

## 核心设计

### asr_shortcut 语义升级

| 维度 | 原 | 新 |
|---|---|---|
| 值格式 | Tauri 加速键（`"Alt+A"` / `"CmdOrCtrl+Shift+A"`） | handy-keys 单键名（`"OptRight"` / `"CmdRight"` / `"CtrlRight"` / `"ShiftRight"` / `"Fn"`） |
| 注册方式 | `core::shortcut::register_shortcut`（Tauri global-shortcut） | `ptt::register_ptt`（handy-keys keydown/keyup 监听 + PttFsm 状态机） |
| 触发 | toggle 开始/停止 | 单键三模式（长按=PTT / 双击=toggle / 短按=hands-free） |
| UI | ShortcutButton（capture 组合键） | dropdown（5 选 1） |

### ptt_key 合并进 asr_shortcut

`ptt_key` 字段删除——其值并入 `asr_shortcut`。`setup.rs` 的 `register_ptt(&config.ptt_key)` 改为 `register_ptt(&config.asr_shortcut)`。

### 删除项

**polish_global_shortcut（立即润色快捷键 Alt+S）**：
- 整条删除（seed / config.rs / setup.rs 注册 / result_window.rs 函数 / settings_commands 热重载 / GeneralPanel UI / locale）
- `Command::PolishNow` + `polish_now` Tauri command **保留**（工具栏按钮 + toggle 中按单键仍用）

**record_mode（死代码）**：
- 整条删除（seed / config.rs 字段+default+Default impl）
- ptt.rs:448 注释更新（去掉 record_mode 引用）

**core/shortcut.rs::register_shortcut**：
- asr_shortcut 不再用 Tauri global-shortcut 注册 → `register_shortcut` 无调用点 → 删除整个函数（确认无其他调用点后）

### 不删除项

- `edit_global_shortcut`（语音编辑 Alt+E）——保留，与单键三模式无关（跨应用唤起结果窗编辑）
- `clipboard_shortcut` / `action_bar_shortcut` / `screenshot_shortcut` / `record_shortcut` / `vault_autotype_shortcut`——保留
- `Command::PolishNow` + `polish_now` Tauri command + 工具栏立即润色按钮——保留

## 后端改动

### config.rs

- `asr_shortcut`：default fn `default_asr_shortcut` 改返回 `"OptRight"`（handy-keys 名）；doc 更新（单键名，非 Tauri 加速键）
- `ptt_key`：删字段 + default fn + Default impl 行
- `polish_global_shortcut`：删字段 + default fn + Default impl 行
- `record_mode`：删字段 + default fn + Default impl 行

### db.sql

- `asr_shortcut` seed：`'Alt+A'` → `'OptRight'`，description 更新
- `ptt_key` seed：删行
- `polish_global_shortcut` seed：删行
- `record_mode` seed：删行

### setup.rs

- `register_shortcut(asr_shortcut)` 调用删除
- `register_polish_global_shortcut(polish_global_shortcut)` 调用删除
- `register_ptt(&config.ptt_key)` → `register_ptt(&config.asr_shortcut)`
- **register_ptt 兜底**：parse `asr_shortcut` 值失败时（不在合法 5 值内），fallback 注册默认右 Alt（OptRight）+ warn log

### core/shortcut.rs

- `register_shortcut` 函数删除（无调用点）
- 若文件只剩 mod 声明 / 空文件，评估删除整个文件 + mod.rs 声明

### result_window.rs

- `register_polish_global_shortcut` 函数删除
- `trigger_global_polish` 函数删除（仅 polish_global_shortcut handler 调用）

### settings_commands.rs

- `apply_config_value`：删 `asr_shortcut`（旧 Tauri 加速键语义）+ `polish_global_shortcut` + `record_mode` match arms；加 `asr_shortcut`（新单键名语义）arm——校验值在 `["OptRight","CmdRight","CtrlRight","ShiftRight","Fn"]` 内
- `set_config` 热重载：
  - 删 `asr_shortcut`（旧：register_shortcut 注销/注册）+ `polish_global_shortcut` 分支
  - 加 `asr_shortcut`（新：`ptt::unregister_ptt` + `ptt::register_ptt(new)`，失败回滚）

## 前端改动

### GeneralPanel.tsx

- 「语音识别」行：ShortcutButton（capture）→ dropdown（5 选 1 单键）
  - 选项：右 Option / 右 Command / 右 Control / 右 Shift / Fn（值 OptRight/CmdRight/CtrlRight/ShiftRight/Fn）
  - 改选时 `invoke("set_config", { key: "asr_shortcut", value: "OptRight" })`
  - 选中值用 kbd 风格展示（⌥ 右 / ⌘ 右 / ⌃ 右 / ⇧ 右 / fn）
- 「识别润色」行（polish_global_shortcut）：删除

### ShortcutButton.tsx

- 保留（edit_global_shortcut 等仍用）。如需展示单键名可加映射，但 PTT 键选择器用 dropdown 不经 ShortcutButton

### locale

- `asrShortcut` label 保留（仍是「语音识别」），值展示改为单键名
- `polishShortcut` / `polishShortcutHint`：删
- 加 PTT 键选项 label（右 Option / 右 Command / 右 Control / 右 Shift / Fn）

## 不变量

1. edit_global_shortcut（语音编辑）完全不变
2. clipboard/action_bar/screenshot/record/vault 快捷键不变
3. PTT 单键三模式行为不变（FSM + RECORDING_MODE）
4. toggle 入口从双击单键触发（原 asr_shortcut 组合键删除）
5. 立即润色从工具栏按钮 + toggle 中按单键触发（原 polish_global_shortcut 删除）
6. `Command::PolishNow` / `polish_now` command 保留

## 风险

- **toggle 失去独立组合快捷键**：用户必须双击单键触发 toggle。双击已验证可用。托盘菜单保留 toggle 入口（现有）
- **asr_shortcut 值不合法**：`register_ptt` parse 失败时（值不在 OptRight/CmdRight/CtrlRight/ShiftRight/Fn 内），fallback 到默认右 Alt（OptRight）+ warn。无旧用户兼容包袱，开发者手动清 DB
- **register_shortcut 删除影响**：确认 core/shortcut.rs 无其他导出/调用后删除；若文件还有其他函数（如 unregister），保留文件删函数

## 文件改动

| 文件 | 操作 |
|---|---|
| `crates/infra/src/config.rs` | asr_shortcut default 改 OptRight；删 ptt_key/polish_global_shortcut/record_mode 字段+default+Default |
| `crates/infra/src/db.sql` | asr_shortcut seed 改 OptRight；删 ptt_key/polish_global_shortcut/record_mode seed 行 |
| `crates/desktop/src/core/setup.rs` | 删 register_shortcut + register_polish_global_shortcut 调用；register_ptt 改用 asr_shortcut |
| `crates/desktop/src/core/shortcut.rs` | 删 register_shortcut 函数（确认无其他调用） |
| `crates/desktop/src/ui/result_window.rs` | 删 register_polish_global_shortcut + trigger_global_polish |
| `crates/desktop/src/commands/settings_commands.rs` | apply_config_value + set_config 热重载改（删旧 arms + 加 asr_shortcut 新 arm + ptt 热重载） |
| `crates/desktop/src/platform/ptt.rs` | 注释更新（去 record_mode 引用） |
| `crates/desktop/frontend/src/pages/Settings/GeneralPanel.tsx` | 「语音识别」改 dropdown；删「识别润色」行 |
| `crates/desktop/frontend/src/locales/{zh-CN,en}.yaml` | 删 polishShortcut labels；加 PTT 键选项 labels |

## 验证

- cargo build + cargo test（删除字段后所有引用点全清理）
- tsc + vite build（dropdown UI 编译）
- e2e：① 设置页选不同 PTT 键→即时生效 ② 改键后三模式仍工作 ③ edit_global_shortcut 不受影响 ④ 工具栏立即润色按钮仍工作 ⑤ asr_shortcut 值不合法时 fallback OptRight

## 实现状态（2026-08-01 完成）

### 已实现

- ✅ **config.rs**（Task 1）：`asr_shortcut` default 改 `"OptRight"` + doc 更新；删 `ptt_key` / `polish_global_shortcut` / `record_mode` 字段 + default fn + Default impl 行。
- ✅ **db.sql**（Task 1）：`asr_shortcut` seed `'Alt+A'` → `'OptRight'`；删 `ptt_key` / `polish_global_shortcut` / `record_mode` 3 行 seed。注：未升 schema version（旧库需手动清 DB 重建）。
- ✅ **setup.rs**（Task 2）：`register_shortcut(asr_shortcut)` + `register_polish_global_shortcut(...)` 两处调用删除；`register_ptt(&config.ptt_key)` → `register_ptt(&config.asr_shortcut)`；**register_ptt 兜底**：parse 失败（值不在 `["OptRight","CmdRight","CtrlRight","ShiftRight","Fn"]` 内）时 fallback 注册 `OptRight` + warn log。
- ✅ **result_window.rs**（Task 2）：`register_polish_global_shortcut` + `trigger_global_polish` 函数删除（setup.rs 是唯一调用方）。
- ✅ **core/shortcut.rs**（Task 3）：整个文件删除（`register_shortcut` 函数无任何调用点）+ `core/mod.rs` 的 `pub mod shortcut;` 声明删除。
- ✅ **settings_commands.rs**（Task 3）：
  - `apply_config_value`：删 `polish_global_shortcut` / `record_mode` / 旧 `asr_shortcut`（Tauri 加速键）3 个 arm；加新 `asr_shortcut` arm——校验值在 `["OptRight","CmdRight","CtrlRight","ShiftRight","Fn"]` 白名单内。
  - `set_config` 热重载：删 `polish_global_shortcut` 分支；`asr_shortcut` 分支改走 `ptt::unregister_ptt` + `ptt::register_ptt(new)`，新键注册失败时回滚到旧键 + warn。
- ✅ **GeneralPanel.tsx**（Task 4）：「语音识别」行 `ShortcutButton` → `<Select>` dropdown 5 选 1（右 Option/右 Command/右 Control/右 Shift/Fn）；改选 `invoke("set_config", {key:"asr_shortcut", value})`；选中值用 `cfg.asr_shortcut || "OptRight"` 兜底；「识别润色」行（polish_global_shortcut）整行删除。
- ✅ **locale zh-CN/en**（Task 4）：删 `polishShortcut` / `polishShortcutHint`；保留 `asrShortcut`（label）+ `asrShortcutHint`（改为「单键三模式」说明）；加 `asrKeyOptRight` / `asrKeyCmdRight` / `asrKeyCtrlRight` / `asrKeyShiftRight` / `asrKeyFn` 5 个选项 label（含 kbd 符号 ⌥/⌘/⌃/⇧/fn）。
- ✅ **ptt.rs**（Task 2）：`unregister_ptt` 注释由 `record_mode` 改为 `asr_shortcut`（热重载用途）。
- ✅ **不删除项验证**：`edit_global_shortcut` / `clipboard_shortcut` / `screenshot_shortcut` / `action_bar_shortcut` / `record_shortcut` / `vault_autotype_shortcut` 全部保留；`Command::PolishNow` + `polish_now` Tauri command 保留（工具栏按钮 + toggle 中按单键仍调）。

### 偏差与决策

1. **未升 schema version**：db.sql 改了 seed（删 3 行 + 改 1 行值）但不升 `CURRENT_SCHEMA_VERSION`——单用户开发库 + 旧 seed 值（如 `Alt+A`）在运行期会被 `register_ptt` fallback 兜底（不合法 → OptRight），开发者手动清 DB 即可。避免为已废弃字段加迁移代码。
2. **asr_shortcut 兜底策略选 fallback 而非 fail**：spec 风险项「值不合法」原设想两选项——fail（应用启动失败）或 fallback（用默认 OptRight + warn）。实现选 fallback：① 不阻塞应用启动（单键错误不应让整个 app 无法用）② 与 settings_commands 的 set_config 回滚策略一致（不合法值在 UI 改选时就被拦下，DB 脏数据只在手动改库才可能出现）。
3. **register_ptt 与 unregister_ptt 不对称的 `#[allow(dead_code)]`**：`unregister_ptt` 在 Task 2 实现时还是 dead code（Task 3 set_config 才用到），暂加 `#[allow(dead_code)]` + 注释「预留给热重载」。Task 3 完成后实际有用，但 `#[allow]` 属性保留无害（避免反复改属性）。
4. **dropdown 用原生 `<Select>` 而非 ShortcutButton 扩展**：spec 原设想「ShortcutButton 加映射展示单键名」，但 dropdown 与 capture 交互差异大（capture 监听 keydown，dropdown 是静态列表），强行复用 ShortcutButton 反而复杂。原生 `<select>` 5 个 `<option>` 最简，label 含 kbd 符号（⌥/⌘/⌃/⇧/fn）保持视觉一致。
5. **`asrShortcutHint` 文案补强**：原 locale hint 缺失，新增「单键三模式：长按说话 / 双击切换免提 / 短按开关」描述触发方式（解决用户不知道 dropdown 选的键如何用的问题）。

### 验证结果

- `cargo build -p octopus-desktop --features embedded` ✅ 0 error 0 warning
- `cargo test -p octopus-desktop --features embedded` ✅ all pass（含新增 `apply_config_value` 白名单校验测试：合法值通过 / 非法值 `"Ctrl+Alt+Z"` 返回 Err）
- `npx tsc --noEmit` ✅ 0 error
- `npm run build` ✅ vite build 成功
- ✅ e2e 手动验证通过（2026-07-31）：① 设置页选不同 PTT 键→即时生效 ② 改键后三模式仍工作 ③ edit_global_shortcut 不受影响 ④ 工具栏立即润色按钮仍工作 ⑤ asr_shortcut 值不合法 fallback OptRight

### 文件改动汇总

| 文件 | 操作 |
|---|---|
| `crates/infra/src/config.rs` | asr_shortcut default `"OptRight"`；删 ptt_key/polish_global_shortcut/record_mode 字段+default+Default |
| `crates/infra/src/db.sql` | asr_shortcut seed `'OptRight'`；删 ptt_key/polish_global_shortcut/record_mode seed 行 |
| `crates/infra/src/db/config.rs` | 删 record_mode 反序列化字段 |
| `crates/desktop/src/core/setup.rs` | 删 register_shortcut + register_polish_global_shortcut 调用；register_ptt 改用 asr_shortcut + 兜底 |
| `crates/desktop/src/core/shortcut.rs` | **整个文件删除** |
| `crates/desktop/src/core/mod.rs` | 删 `pub mod shortcut;` 声明 + 注释 |
| `crates/desktop/src/ui/result_window.rs` | 删 register_polish_global_shortcut + trigger_global_polish |
| `crates/desktop/src/commands/settings_commands.rs` | apply_config_value 删 3 旧 arm + 加 asr_shortcut 新 arm（白名单校验）；set_config 热重载改 PTT |
| `crates/desktop/src/platform/ptt.rs` | unregister_ptt 注释更新 |
| `crates/desktop/src/clipboard/clipboard_window.rs` | 注册回滚 import 路径微调 |
| `crates/desktop/src/record/screenshot_commands/area.rs` | 注册回滚 import 路径微调 |
| `crates/desktop/frontend/src/pages/Settings/GeneralPanel.tsx` | 「语音识别」改 dropdown；删「识别润色」行 |
| `crates/desktop/frontend/src/locales/zh-CN.yaml` | 删 polishShortcut labels；加 5 个 asrKey* label |
| `crates/desktop/frontend/src/locales/en.yaml` | 同上 |
