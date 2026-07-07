# 窗口位置 / 最大化 / 多显示器 设计规格

**日期**：2026-07-08
**范围**：`crates/desktop/src/window_position.rs` + `compact_editor_window.rs::WindowState`——Tauri 窗口位置持久化、最大化状态记忆、多显示器越界检测与 fallback。
**关联**：架构定位见 `docs/architecture.md`（窗口创建注意事项 §3/§4）；迭代史见 main commits `9cf19a2..2038804`（最大化/多显示器十二次反复）。

> 本 spec 2026-07-08 基于 main 最新实现重写——早先版本据 ec55021 旧代码写了「builder.maximized 必须用 / position 不除 scale / maximized 存归零」等结论，main 的 18 个窗口迭代已全部推翻，以本文为准。

---

## 1. 两套并行实现

| 实现 | 文件 | 服务窗口 | 记忆内容 | DB key |
|------|------|----------|----------|--------|
| **轻量位置** | `window_position.rs` | `clipboard_window` / `result_window` | 仅 x,y 位置 | `window_pos.{label}` = `"x,y"` |
| **全状态** | `compact_editor_window.rs::WindowState` | `compact_editor_window` | width/height/x/y/**maximized** + 最后非最大化位置 | `compact_editor_window_state`（serde JSON）+ `compact_editor_last_normal_pos`=`"x,y,w,h"` |

两者都走 `octopus_infra::db::save_config_key` / `load_config_key`，`category='system'` 与业务配置隔离。

**为何不统一**：clipboard/result 只需位置记忆；compact_editor 需完整状态（可调尺寸 + 最大化 + 跨屏恢复）。越界检测 50px 容差逻辑两处各自内联（一处判点、一处判状态+找显示器），未抽公共。

---

## 2. 轻量位置（window_position.rs）

| 函数 | 行为 |
|------|------|
| `save_window_position(label, x, y)` | 存 `"x,y"`（逻辑坐标）到 `window_pos.{label}` |
| `load_window_position(label)` | 读 + parse |
| `is_position_visible(x, y, monitors)` | **50px 容差**判点是否在任一显示器内（§4 不变量①） |
| `restore_window_position(window, label, fallback)` | load → `available_monitors` → 可见则 `set_position(Logical)`，不可见调 `fallback` |
| `save_current_position(window, label)` | `outer_position() / scale_factor` → 逻辑坐标（window event handler 用） |

调用点：`clipboard_window.rs`、`result_window.rs`，都传 `fallback` 闭包做居中/默认。

---

## 3. 全状态 WindowState（compact_editor_window.rs）

```rust
struct WindowState { width, height, x, y, maximized: bool }   // camelCase serde，STATE_KEY="compact_editor_window_state"
```

另有独立 key `compact_editor_last_normal_pos`=`"x,y,w,h"`（逻辑），记录**最后非最大化位置/尺寸**——最大化恢复时据此匹配显示器。

**关窗保存**（`on_compact_editor_save_state`）：
- `maximized` → **先 `unmaximize()` → 读 `inner_position()`+`inner_size()`（真实位置，反映窗口所在屏幕）→ `re-maximize()`**；存 `last_normal_pos` + `WindowState{真实 w/h/x/y, maximized:true}`。不能直接读最大化时的 `inner_position()`（返回全屏位置，可能跨屏到主屏原点）。
- 非最大化 → `inner_position()` + `inner_size()` 各 `/ scale`（inner/inner 对称，§4 不变量⑥）；同样写 `last_normal_pos`。

**开窗还原**（`create_compact_editor_window`）——`visible(false)` 建窗，所有配置就绪后再 `show`：
- 非最大化分支：`state.w/h>0` → 内联 50px 越界检测 → 可见则 `inner_size+position`，不可见 `center`；`builder.maximized(false)`
- 最大化分支（三层 fallback，§4 不变量⑤）：
  1. **坐标匹配到已连接显示器**（`state.x/y` 逻辑 vs monitor 物理 ÷ scale）→ 该屏尺寸 **减余量**（宽 `mw-160`、高 `mh-120`，即 `margin=80`：`inner_size(mw-80*2, mh-80*1.5)` + `position(mx+80, my+40)`）→ `should_maximize=true`
  2. **未匹配 → `primary_monitor`** → 主屏同样大窗体 → `should_maximize=true`
  3. **连主屏都拿不到 → 默认 880×620 `center`**
- `build()` 后：`show()` + `set_focus()` + 若 `should_maximize` 则 `maximize()`（窗口已是接近全屏大尺寸，maximize 视觉变化极小）

---

## 4. 关键不变量（十二次反复确立，违反即回归窗口错位）

**① scale 转换：Monitor 的 position 和 size 都是物理像素，必须全部 ÷ scale_factor 统一逻辑**
`is_position_visible`（L37-41）与 compact_editor 内联（L156-164 / L185-190）：
```rust
let ms = m.scale_factor();
let mx = m.position().x as f64 / ms;   // position 也是物理，必须除
let my = m.position().y as f64 / ms;
let mw = m.size().width as f64 / ms;
let mh = m.size().height as f64 / ms;
```
> 修正史 `c3efb0c`：旧版 `mx = position().x`（不除）、`mw = size()/scale` 物理逻辑混用 → Retina（scale=2）下副屏逻辑坐标被判进主屏物理范围 → **副屏永远匹配主屏**。position 和 size 都必须除。

**② 50px 容差（TOLERANCE）**
`is_position_visible` 用 50px 容差。副屏拔接后保存的绝对逻辑坐标失效时 fallback 居中，避免窗口「消失」到不存在的屏。

**③ builder.maximized(true) 在 WRY 底层不生效——必须 visible(false) 建大窗体后 show + maximize**
`builder.maximized(true)` build 后 `is_maximized=false`（WRY 未生效）；`build 后 win.maximize()` 在 `show()` 前调则 macOS 隐藏窗 maximize 无 zoom 动画。**最终方案**：`visible(false)` 创建 → 大窗体（屏尺寸减余量）→ `show()` → `maximize()`（窗口已接近全屏，视觉差异极小）→ 确保 `is_maximized=true`。
> 反复史：`builder.maximized` 不生效（d62714d）→ 手动 maximize（9cf19a2 visible(false) 创建后 show）→ 主屏尺寸直创无 maximize（1444b3f，但 is_maximized=false 致保存错误状态）→ 最终大窗体+show+maximize（9326403…）。**不能**绕过 `maximize()` API 直接用主屏尺寸创建——`is_maximized=false` 会让关窗保存错误地记成非最大化。

**④ 最大化保存真实位置（unmaximize → inner_position → re-maximize）**
最大化时直接读 `inner_position()` 返回全屏位置（可能跨屏到主屏原点）。关窗保存必须：`unmaximize()` → 读真实 `inner_position()`/`inner_size()` → `re-maximize()` → 存 `last_normal_pos`。下次打开据 `last_normal_pos` 匹配显示器，**副屏最大化窗口还原不挪到主屏**（`92951d1`）。

**⑤ 副屏未连接三层 fallback**
恢复时按 `last_normal_pos` 匹配已连接显示器：匹配到 → 该屏大窗体 + maximize；未匹配 → `primary_monitor` 大窗体 + maximize（`b421a53` 屏未连接回退主屏最大化，非默认小窗）；连主屏都拿不到 → 默认 880×620 居中（`138848d`）。

**⑥ inner_position + inner_size 对称**
关窗保存用 `inner_position()` + `inner_size()`，都是内容区坐标（不含标题栏）。混用 outer/inner 会因标题栏高度产生坐标偏差。所有坐标 `/ scale` 存逻辑像素。

---

## 5. 设计决策与遗留

- **越界检测两处内联重复**（`is_position_visible` + `compact_editor` 最大化分支各一套）：均 50px 容差 + position/size ÷ scale，但一处判点、一处判状态+找显示器。可抽公共 helper，当前两处上下文独立，重复成本低于抽象成本。
- **DB 而非 localStorage**：位置/状态记忆存 SQLite app_config（`category='system'`），跨窗口实例、跨重启持久，前端不参与。
- **scale_factor 兜底 1.0**：所有 `/ scale` 处 `unwrap_or(1.0)`，避免 scale 获取失败 panic。
- **WindowState.maximized=true 时 x/y 是真实非最大化位置**（非归零）——与早先 ec55021 版「存 {W,H,0,0,true}」不同，现在存 unmaximize 读到的真实坐标，配合 `last_normal_pos` 双写。

---

## 6. 边界用例

| 场景 | 行为 |
|------|------|
| 首次开窗（无记忆） | WindowState::default() → `inner_size(880,620).center()` |
| 副屏拔了，坐标越界 | `is_position_visible`/内联判 false → fallback 居中（轻量）/ 三层 fallback（全状态） |
| 最大化状态关窗 | unmaximize 读真实位置 → re-maximize → 存 `last_normal_pos` + WindowState(maximized:true)；重开按 last_normal_pos 匹配屏 → 大窗体+show+maximize |
| 副屏最大化窗口还原 | `92951d1`：按 last_normal_pos 匹配副屏，不挪主屏 |
| Retina 多屏 | position/size 全 ÷ scale 统一逻辑（`c3efb0c`），副屏坐标正确匹配本屏 |
| scale 获取失败 | `unwrap_or(1.0)` 兜底，坐标按 1.0 算 |
