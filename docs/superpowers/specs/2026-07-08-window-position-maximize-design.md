# 窗口位置 / 最大化 / 多显示器 设计规格

**日期**：2026-07-08
**范围**：`crates/desktop/src/window_position.rs` + `compact_editor_window.rs::WindowState`——Tauri 窗口位置持久化、最大化状态记忆、多显示器越界检测。
**关联**：架构定位见 `docs/architecture.md`（窗口创建注意事项）；本轮迭代史见 main commits `9326403..2038804`（最大化/多显示器 12 次反复）。

---

## 1. 两套并行实现

按窗口对「状态记忆」的需求分两套：

| 实现 | 文件 | 服务窗口 | 记忆内容 | DB key |
|------|------|----------|----------|--------|
| **轻量位置** | `window_position.rs` | `clipboard_window` / `result_window` | 仅 x,y 位置 | `window_pos.{label}` = `"x,y"` |
| **全状态** | `compact_editor_window.rs::WindowState` | `compact_editor_window` | width/height/x/y/**maximized** | `compact_editor_window_state`（serde JSON camelCase） |

两者都走 `octopus_infra::db::save_config_key` / `load_config_key`，`category='system'` 与业务配置隔离。

**为何不统一**：clipboard/result 只需位置记忆（无尺寸/最大化需求，用固定或默认）；compact_editor 需完整状态（可调尺寸 + 最大化）。越界检测逻辑两处各自内联（50px 容差），未抽公共——上下文不同（一个只判点、一个判 width/height+maximized）。

---

## 2. 轻量位置（window_position.rs）

| 函数 | 行为 |
|------|------|
| `save_window_position(label, x, y)` | 存 `"x,y"`（逻辑坐标）到 `window_pos.{label}` |
| `load_window_position(label)` | 读 + parse；坐标不可见时仍返回 Some（可见性由调用方判） |
| `is_position_visible(x, y, monitors)` | **50px 容差**判点是否在任一显示器内（§3 不变量①） |
| `restore_window_position(window, label, fallback)` | load → `available_monitors` → 可见则 `set_position(Logical)`，不可见调 `fallback` |
| `save_current_position(window, label)` | `outer_position() / scale_factor` → 逻辑坐标（window event handler 用） |

调用点：`clipboard_window.rs:27`、`result_window.rs:64`，都传 `fallback` 闭包做居中/默认。

---

## 3. 全状态 WindowState（compact_editor_window.rs）

```rust
struct WindowState { width, height, x, y, maximized: bool }   // camelCase serde
```

**关窗保存**（`on_compact_editor_save_state` L45）：
- `maximized` → 存 `{WIDTH, HEIGHT, 0.0, 0.0, maximized:true}`（位置归零，最大化状态由 builder 还原，§不变量③）
- 非最大化 → `inner_position() + inner_size()` 各 `/ scale`（**inner/inner 对称**，内容区坐标不含标题栏，§不变量⑥）

**开窗还原**（`create_compact_editor_window` L75）：
- 非最大化分支：`state.width/height>0` → 内联 50px 越界检测 → 可见则 `inner_size+position`，不可见 fallback `center`
- 最大化分支：`builder.maximized(true)`（build 前设，§不变量③）
- 非最大化显式 `builder.maximized(false)`（避免设尺寸再被 maximize 覆盖的冗余布局）

---

## 4. 关键不变量（12 次反复确立，违反即回归窗口错位）

**① scale 转换：monitor 物理像素 ÷ scale_factor**
`is_position_visible`（L39-40）与 compact_editor 内联（L133-134）：
```rust
let mw = m.size().width as f64 / m.scale_factor();   // 物理 ÷ scale → 逻辑
let mh = m.size().height as f64 / m.scale_factor();
let mx = m.position().x as f64;                        // position 已是逻辑，不除
```
> 修正史：`c3efb0c` `is_position_visible` 物理像素未除 scale；`790ac15` 显示器坐标物理/逻辑不匹配致副屏永远匹配到主屏（坐标尺度不一致，副屏坐标被判越界 → 永远 fallback 主屏）。

**② 50px 容差（TOLERANCE）**
`is_position_visible` 用 50px 容差。副屏拔接后保存的绝对逻辑坐标失效时 fallback 居中，避免窗口「消失」到不存在的屏。

**③ 最大化必须 `builder.maximized(true)`，不能 build 后 `win.maximize()`**
后者窗口先以记忆尺寸可见出现，再触发 macOS 原生 zoom 动画（~300-500ms）放大到全屏——用户看到「PPT slide」效果。`WebviewWindowBuilder::maximized(bool)` 在 build 前设，窗口首帧即最大化。
> 修正史：`9326403` 先创建接近全屏大窗体再 maximize → 最终定型 builder.maximized(true)。

**④ 关窗时 maximized 特殊存位置**
最大化时存 `{W,H,0,0,true}`（位置归零、占位）。`e6acce0` 修复：保存时先 `un-maximize` 取真实位置；`92951d1` 最大化保存**最后非最大化位置**——副屏最大化窗口还原不再跑到主屏。
> 即：WindowState.maximized=true 时 x/y 不可信（归零），还原走 builder.maximized(true)，不读 x/y。

**⑤ 副屏未连接 fallback 策略**
`b421a53` 屏幕未连接回退主屏**最大化**（非默认小窗）；`138848d` 副屏未连接回退主屏默认大小；`bd8fe4d` 最大化用保存坐标找对应显示器 + 四边留余量。统一原则：坐标越界（屏拔了）→ 回退当前主屏，不丢窗口。

**⑥ inner_position + inner_size 对称**
关窗保存用 `inner_position()` + `inner_size()`（L52-53），都是内容区坐标（不含标题栏）。混用 outer/inner 会因标题栏高度产生坐标偏差。

---

## 5. 设计决策与遗留

- **越界检测两处重复**（`window_position::is_position_visible` + `compact_editor` 内联）：均 50px 容差、相同 scale 换算，但一处判点、一处判状态。可抽公共 helper，但当前两处上下文足够独立，重复成本低于抽象成本。
- **DB 而非 localStorage**：位置记忆存 SQLite app_config（`category='system'`），跨窗口实例、跨重启持久，前端不参与。
- **scale_factor 兜底 1.0**：所有 `/ scale` 处 `unwrap_or(1.0)`，避免 scale 获取失败 panic。
- **result/clipboard 无尺寸记忆**：用各自固定尺寸 + window_position 位置，不需全状态。若将来加尺寸记忆，扩 window_position 或迁全状态。

---

## 6. 边界用例

| 场景 | 行为 |
|------|------|
| 首次开窗（无记忆） | WindowState::default() → compact_editor `inner_size(WIDTH,HEIGHT).center()` |
| 副屏拔了，坐标越界 | `is_position_visible`/内联判 false → fallback 居中（轻量）/ center（全状态） |
| 最大化状态关窗 | 存 `{W,H,0,0,true}`；重开 `builder.maximized(true)`，不读 x/y |
| 副屏最大化窗口 | `92951d1` 存最后非最大化位置，还原不跑主屏 |
| scale 获取失败 | `unwrap_or(1.0)` 兜底，坐标按 1.0 算 |
