# asr_shortcut 升级为单键选择器 + 清理废弃快捷键 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `asr_shortcut` 从 Tauri 组合快捷键升级为 handy-keys 单键名（dropdown 5 选 1），删除 polish_global_shortcut + record_mode 死代码，ptt_key 合并进 asr_shortcut。

**Architecture:** asr_shortcut 值从 `"Alt+A"` → `"OptRight"`（handy-keys 单键名）。注册从 `register_shortcut`（Tauri global-shortcut）→ `register_ptt`（PttFsm）。设置页 UI 从 ShortcutButton（capture）→ dropdown（5 选 1）。set_config 热重载改为 unregister_ptt/register_ptt。值不合法 fallback OptRight。

**Tech Stack:** Rust + Tauri 2 + handy-keys + React + TypeScript

## Global Constraints

- `asr_shortcut` 合法值：`"OptRight"` / `"CmdRight"` / `"CtrlRight"` / `"ShiftRight"` / `"Fn"`（handy-keys 单键名）
- `asr_shortcut` 值不合法时 fallback 默认右 Alt（`"OptRight"`）+ warn
- edit_global_shortcut（语音编辑 Alt+E）**保留不变**
- clipboard/action_bar/screenshot/record/vault 快捷键**保留不变**
- `Command::PolishNow` + `polish_now` Tauri command + 工具栏按钮**保留**（只删快捷键注册 + UI 行）
- PTT 单键三模式行为不变（FSM + RECORDING_MODE）
- 无旧用户兼容包袱（开发者手动清 DB）

**Spec:** `docs/superpowers/specs/2026-08-01-asr-key-selector-design.md`

---

## File Structure

**删除字段/函数：**
- `config.rs`：删 `ptt_key` / `polish_global_shortcut` / `record_mode` 字段 + default fn + Default impl 行
- `core/shortcut.rs`：删 `register_shortcut` 函数（确认无其他调用后）
- `result_window.rs`：删 `register_polish_global_shortcut` + `trigger_global_polish`

**修改：**
- `config.rs`：`asr_shortcut` default → `"OptRight"`
- `db.sql`：asr_shortcut seed → OptRight；删 ptt_key/polish_global_shortcut/record_mode seed
- `setup.rs`：删 register_shortcut + register_polish 调用；register_ptt 改用 asr_shortcut
- `settings_commands.rs`：apply_config_value 改 asr_shortcut arm（校验 5 值）+ 删 polish arm；set_config 热重载改 asr_shortcut（ptt）+ 删 polish
- `GeneralPanel.tsx`：「语音识别」行改 dropdown；删「识别润色」行
- `locales/{zh-CN,en}.yaml`：删 polishShortcut labels；加 PTT 键选项 labels

---

## Task 1: 后端 config.rs + db.sql 字段清理

**Files:**
- Modify: `crates/infra/src/config.rs`
- Modify: `crates/infra/src/db.sql`

**Interfaces:**
- Produces: `asr_shortcut` default = `"OptRight"`；`ptt_key` / `polish_global_shortcut` / `record_mode` 字段删除

- [ ] **Step 1: config.rs — asr_shortcut default 改 OptRight**

`crates/infra/src/config.rs` line 271-273（`default_asr_shortcut` 函数）：
```rust
fn default_asr_shortcut() -> String {
    "OptRight".into()
}
```
同时更新字段 doc（line 78-79）：从「全局 ASR 激活/关闭快捷键」改为「单键三模式触发键（handy-keys 名：OptRight/CmdRight/CtrlRight/ShiftRight/Fn），长按=PTT / 双击=toggle / 短按=hands-free」。

- [ ] **Step 2: config.rs — 删 ptt_key 字段 + default + Default impl**

- 删字段声明（line 231-232 `pub ptt_key: String`）
- 删 `default_ptt_key` 函数（line 352-354）
- 删 Default impl 里的 `ptt_key: default_ptt_key(),` 行（约 line 398）

- [ ] **Step 3: config.rs — 删 polish_global_shortcut 字段 + default + Default impl**

- 删字段声明（line 164-165）
- 删 `default_polish_global_shortcut` 函数（line 313-315）
- 删 Default impl 里的 `polish_global_shortcut: default_polish_global_shortcut(),` 行（约 line 392）

- [ ] **Step 4: config.rs — 删 record_mode 字段 + default + Default impl**

- 删字段声明 + doc（line 221-227）
- 删 `default_record_mode` 函数（line 349-351）
- 删 Default impl 里的 `record_mode: default_record_mode(),` 行（约 line 402）

- [ ] **Step 5: db.sql — asr_shortcut seed 改 + 删 3 行**

- `asr_shortcut` seed（line 405）：`'Alt+A'` → `'OptRight'`，description 改 `'单键三模式触发键（handy-keys 名：OptRight/CmdRight/CtrlRight/ShiftRight/Fn）'`
- 删 `polish_global_shortcut` seed 行（line 408）
- 删 `record_mode` seed 行（line 420）
- 删 `ptt_key` seed 行（line 421）

- [ ] **Step 6: build 验证（预期有 error——下游引用 ptt_key/polish_global_shortcut/record_mode）**

Run: `cargo build -p octopus-infra 2>&1 | grep -E "^error" | head`
Expected: infra 自身编译过（infra 不引用这些字段）。desktop 引用在 Task 2/3 修。

Run: `cargo build -p octopus-desktop --features embedded 2>&1 | grep -E "^error" | head -20`
Expected: error（setup.rs/settings_commands.rs 引用 ptt_key/polish_global_shortcut/record_mode）——Task 2/3 修复。

- [ ] **Step 7: Commit**

```bash
git add crates/infra/src/config.rs crates/infra/src/db.sql
git commit -m "refactor(config): asr_shortcut 默认改 OptRight + 删 ptt_key/polish_global_shortcut/record_mode 字段"
```

---

## Task 2: 后端 setup.rs + shortcut.rs + result_window.rs 清理

**Files:**
- Modify: `crates/desktop/src/core/setup.rs`
- Modify: `crates/desktop/src/core/shortcut.rs`
- Modify: `crates/desktop/src/ui/result_window.rs`
- Modify: `crates/desktop/src/platform/ptt.rs`（注释更新）

**Interfaces:**
- Consumes: Task 1 的 config.rs（asr_shortcut 单键名，ptt_key 已删）

- [ ] **Step 1: setup.rs — 删 register_shortcut + register_polish 调用，register_ptt 改用 asr_shortcut**

`crates/desktop/src/core/setup.rs` `register_shortcuts` 方法（line 686-710）：
- 删 line 689-691（register_shortcut(asr_shortcut) 调用 + error log）
- 删 line 698-701（register_polish_global_shortcut 调用 + error log）
- line 705：`&self.config.ptt_key` → `&self.config.asr_shortcut`
- 更新注释（line 687-688, 703-704）：去掉「toggle（asr_shortcut）」表述，改为「单键三模式（asr_shortcut）」

- [ ] **Step 2: setup.rs — register_ptt 兜底（值不合法 fallback OptRight）**

`register_ptt` 调用处（原 line 705）改为：
```rust
// 单键三模式：注册 asr_shortcut 键监听（handy-keys）。值不合法时 fallback OptRight。
let asr_key = if ["OptRight", "CmdRight", "CtrlRight", "ShiftRight", "Fn"].contains(&self.config.asr_shortcut.as_str()) {
    &self.config.asr_shortcut
} else {
    log::warn!("[setup] asr_shortcut '{}' 不合法，fallback OptRight", self.config.asr_shortcut);
    "OptRight"
};
if let Err(e) = crate::platform::ptt::register_ptt(self.app.handle(), asr_key) {
    log::warn!("[ptt] 注册失败: {}", e);
}
```

- [ ] **Step 3: core/shortcut.rs — 删 register_shortcut 函数**

确认无其他调用点（grep `register_shortcut` 除 setup.rs/settings_commands.rs 外无）后，删 `register_shortcut` 函数（line 20 起）。若文件变空或只剩 mod 声明，删整个文件 + `core/mod.rs` 的 `mod shortcut;` 声明。
注：settings_commands.rs 的调用在 Task 3 清理——此步先确认 grep 结果，函数删除可在 Task 3 调用点清理后做（避免编译期 error 指向已删函数）。

- [ ] **Step 4: result_window.rs — 删 register_polish_global_shortcut + trigger_global_polish**

- 删 `register_polish_global_shortcut` 函数（line 463 起）
- 删 `trigger_global_polish` 函数（仅 polish handler 调用）
- 确认无其他调用点

- [ ] **Step 5: ptt.rs — 注释更新**

`crates/desktop/src/platform/ptt.rs` line 448（unregister_ptt 的 `#[allow(dead_code)]` 注释）：去掉 `record_mode 切换时` 引用，改为 `asr_shortcut 热重载时注销旧键`。

- [ ] **Step 6: build 验证（仍有 error——settings_commands.rs 引用）**

Run: `cargo build -p octopus-desktop --features embedded 2>&1 | grep -E "^error" | head`
Expected: settings_commands.rs 引用 register_shortcut/register_polish_global_shortcut 的 error——Task 3 修。

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "refactor(setup): 删 register_shortcut/register_polish 调用 + register_ptt 改用 asr_shortcut + 兜底"
```

---

## Task 3: 后端 settings_commands.rs 热重载 + apply_config_value

**Files:**
- Modify: `crates/desktop/src/commands/settings_commands.rs`

**Interfaces:**
- Consumes: Task 1/2 的 config.rs（无 ptt_key/polish_global_shortcut）+ setup.rs（register_ptt 用 asr_shortcut）
- Produces: `asr_shortcut` 热重载（unregister_ptt/register_ptt）+ apply_config_value 校验

- [ ] **Step 1: set_config — 删 old tuple 的 old_asr_sc/old_polish_global + 改 asr 热重载**

`set_config`（line 140-186）：
- old tuple（line 140-143）：删 `old_asr_sc` + `old_polish_global`（改为只读 asr_shortcut 新值 for ptt 热重载）。实际上保留 `old_asr_sc`（语义变为 PTT 键旧值）。
- 删 old tuple 的 `old_polish_global`（line 140,142）+ `let _ = (&old_vault_autotype_sc, &old_record_sc);` 调整（去掉不存在的变量）
- 删 polish_global_shortcut 热重载块（line 176-186）
- asr_shortcut 热重载块（line 151-161）改为 PTT 热重载：
```rust
if key == "asr_shortcut" && cfg.asr_shortcut != old_asr_sc {
    // PTT 键热重载：unregister 旧 + register 新（失败回滚）
    let _ = crate::platform::ptt::unregister_ptt(&app_handle);
    if let Err(e) = crate::platform::ptt::register_ptt(&app_handle, &cfg.asr_shortcut) {
        log::warn!("[set_config] register_ptt 新键失败，回滚旧键: {}", e);
        let _ = crate::platform::ptt::register_ptt(&app_handle, &old_asr_sc);
        return Err(format!("PTT 键注册失败，配置未更改: {}", e));
    }
}
```

- [ ] **Step 2: apply_config_value — asr_shortcut arm 加校验 + 删 polish arm**

`apply_config_value`（line 335）：
- asr_shortcut arm（line 423-425）改为校验合法值：
```rust
"asr_shortcut" => {
    let v = value.as_str().ok_or("asr_shortcut 需要字符串")?;
    if !["OptRight", "CmdRight", "CtrlRight", "ShiftRight", "Fn"].contains(&v) {
        return Err("asr_shortcut 必须是 OptRight/CmdRight/CtrlRight/ShiftRight/Fn 之一".into());
    }
    cfg.asr_shortcut = v.to_string();
}
```
- 删 polish_global_shortcut arm（line 432-434）

- [ ] **Step 3: 确认 register_shortcut 调用全删 + 删函数**

grep `register_shortcut` 确认 settings_commands.rs 无残留调用 → core/shortcut.rs 的函数可删（Task 2 Step 3 已确认无其他调用，此步执行删除或确认已删）。

- [ ] **Step 4: build 验证**

Run: `cargo build -p octopus-desktop --features embedded 2>&1 | grep -E "^error|^warning"`
Expected: 0 error 0 warning

- [ ] **Step 5: cargo test 验证**

Run: `cargo test -p octopus-desktop --features embedded 2>&1 | tail -3`
Expected: 全过（apply_config_value 测试若有 asr_shortcut 用例需更新）

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "refactor(settings): asr_shortcut 热重载改 PTT + apply_config_value 校验 + 删 polish 分支"
```

---

## Task 4: 前端 GeneralPanel dropdown + locale

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Settings/GeneralPanel.tsx`
- Modify: `crates/desktop/frontend/src/locales/zh-CN.yaml`
- Modify: `crates/desktop/frontend/src/locales/en.yaml`

**Interfaces:**
- Consumes: `set_config({ key: "asr_shortcut", value })` Tauri command

- [ ] **Step 1: GeneralPanel.tsx — 「语音识别」行改 dropdown**

找到快捷键卡片的 `asrShortcut` 行（line 203-204，用 ShortcutButton）。替换为 dropdown：
```tsx
{/* 语音识别（单键三模式触发键）——dropdown 5 选 1，非 ShortcutButton capture */}
<div className="shortcut-row">
  <label>{t("settings.asrShortcut")}</label>
  <select
    value={cfg.asr_shortcut}
    onChange={(e) => setVal("asr_shortcut", e.target.value)}
  >
    <option value="OptRight">⌥ 右 Option</option>
    <option value="CmdRight">⌘ 右 Command</option>
    <option value="CtrlRight">⌃ 右 Control</option>
    <option value="ShiftRight">⇧ 右 Shift</option>
    <option value="Fn">Fn</option>
  </select>
</div>
```
样式对齐现有 shortcut-row（kbd 风格可选，native select 最简）。

- [ ] **Step 2: GeneralPanel.tsx — 删「识别润色」行**

删 `polishShortcut` 行（line 206-207，ShortcutButton for polish_global_shortcut）。
同时清理 `startShortcutCapture` 的 `"polish_global_shortcut"` case（如 startShortcutCapture 有按 key 的 switch/map，确认清理）。

- [ ] **Step 3: locale — 删 polishShortcut + 加 PTT 键说明**

`zh-CN.yaml` + `en.yaml`：
- 删 `polishShortcut` / `polishShortcutHint`
- `asrShortcut` label 保留（仍是「语音识别」/「Voice」），可加 hint 说明「单键三模式：长按说话 / 双击切换 / 短按免提」

- [ ] **Step 4: tsc 验证**

Run: `cd crates/desktop/frontend && npx tsc --noEmit 2>&1 | tail -5`
Expected: 0 error

- [ ] **Step 5: vite build 验证**

Run: `cd crates/desktop/frontend && npm run build 2>&1 | tail -3`
Expected: build 成功

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(settings): 语音识别改 dropdown 单键选择器 + 删识别润色快捷键行"
```

---

## Task 5: 文档同步 + 全量验证

**Files:**
- Modify: `docs/architecture.md`
- Modify: `docs/superpowers/specs/2026-07-31-single-key-three-modes-design.md`（不变量 #5 更新）

- [ ] **Step 1: architecture.md 更新**

找到快捷键/录音模式段，更新：asr_shortcut 从组合快捷键→单键名；删 polish_global_shortcut/record_mode/ptt_key 描述；PTT 键选择器 dropdown。

- [ ] **Step 2: single-key-three-modes spec 不变量 #5 更新**

`docs/superpowers/specs/2026-07-31-single-key-three-modes-design.md` line 153（不变量 #5）：
原 `5. asr_shortcut（Alt+Shift+A）保留——toggle 的备用入口`
改为 `5. ~~asr_shortcut 保留——toggle 的备用入口~~（2026-08-01 废弃）：asr_shortcut 已升级为单键名（OptRight），toggle 仅由双击触发，原组合快捷键删除。详见 asr-key-selector-design.md`

- [ ] **Step 3: spec 加实现状态段**

`docs/superpowers/specs/2026-08-01-asr-key-selector-design.md` 末尾加「实现状态」（已实现清单 + 偏差 + 验证结果）。

- [ ] **Step 4: 全量验证**

Run:
```bash
cargo build -p octopus-desktop --features embedded 2>&1 | grep -E "^error|^warning|Finished"
cargo test -p octopus-desktop --features embedded 2>&1 | tail -3
cd crates/desktop/frontend && npx tsc --noEmit && npm run build 2>&1 | tail -3
```
Expected: build 0 error 0 warning；test 全过；tsc 0 error；vite build 成功

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "docs(sync): asr_shortcut 单键选择器文档同步 + 不变量 #5 更新"
```

- [ ] **Step 6: e2e 提示**

提示用户 e2e：① 设置页选不同 PTT 键→即时生效 ② 改键后三模式仍工作 ③ edit_global_shortcut 不受影响 ④ 工具栏立即润色按钮仍工作 ⑤ asr_shortcut 值不合法 fallback OptRight。

---

## Self-Review

**Spec coverage:**
- ✅ asr_shortcut 语义升级（Task 1 default + Task 2 setup + Task 3 settings + Task 4 UI）
- ✅ ptt_key 合并进 asr_shortcut（Task 1 删字段 + Task 2 register_ptt 改用 asr_shortcut）
- ✅ 删 polish_global_shortcut（Task 1 删字段 + Task 2 删函数 + Task 3 删热重载 + Task 4 删 UI）
- ✅ 删 record_mode（Task 1 删字段/seed）
- ✅ PTT 键 dropdown（Task 4）
- ✅ 即时生效热重载（Task 3 unregister_ptt/register_ptt）
- ✅ 值不合法 fallback OptRight（Task 2 Step 2）
- ✅ edit_global_shortcut 保留（无 task 触碰）
- ✅ Command::PolishNow 保留（无 task 触碰）

**Placeholder scan:** 无 TBD/TODO；每个 Step 有具体代码或命令。

**Type consistency:** `asr_shortcut` 在 config.rs（Task 1）/ setup.rs（Task 2）/ settings_commands.rs（Task 3）/ GeneralPanel.tsx（Task 4）一致用 String + handy-keys 名。`unregister_ptt`/`register_ptt` 签名在 Task 2/3 一致。
