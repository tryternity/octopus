# 2026-07-26 录制控制浮窗（display/window 录制的桌面 pill）

## 背景

录屏 MVP（`screen-record` plan Task 1-15）+ Area 标注 overlay（`record-area-annotation` plan）已落地，但 **display / window 录制时没有任何可视化控制 UI**——用户只能靠 ESC / tray 停止，且录制中无视觉反馈（不知在录、录了多久）。

Area 录制有 RecordAnnotation overlay（带 9 种标注画布 + 工具栏 + 停止按钮），体验完整。display / window 录制零 UI 是体验硬伤。

## 范围（P1-7）

display / window 录制开始时显示一个**桌面右下角 pill 浮窗**：
- 红点（pulse 动画）+ 时长 `mm:ss`
- 暂停 / 恢复按钮（按录制状态切换图标）
- 停止按钮（红方块）

Area 录制不创建本浮窗（已有 RecordAnnotation，互斥）。

> **2026-07-26 补充**：
> 1. **时长 bug 修复**——pill mount 时调 `get_record_status` 拿真实 state + elapsed_secs（修 duration=0 bug：浮窗创建晚于 recording-started 事件，useRecordSession 收不到事件，duration 永远 0）。前端维护本地 `currentState` + `displayDuration`，监听 `record://event` 更新。
> 2. **RecordAnnotation 也加了同款控件**——area 录制的 RecordAnnotation 工具栏尾也加了录制时长 mm:ss + 暂停/继续按钮（与 pill 同范式，commit `bbfebf57`/`323ba014`），保证 area 录制也有时长反馈 + 暂停入口。

## 设计决策（用户确认）

| 决策点 | 选择 | 理由 |
|---|---|---|
| 位置 | 录制所在屏右下角（display_id → `CGDisplay::bounds()` 精确查逻辑边界） | ~~MVP fallback 主屏~~ → **2026-07-26 修复**：副屏录制 pill 跑到主屏右下角 bug。改用 `core_graphics::display::CGDisplay::new(display_id).bounds()` 直接拿逻辑 CGRect（CoreGraphics 原生返回 points，已含 scale），无需 Tauri Monitor 物理坐标 ÷ scale。详见下方「位置算法」。 |
| **尺寸** | **130×38**（原 200×56 太长，用户反馈） | 紧凑布局：红点(7px)+gap+时长(~28px)+暂停(24px)+停止(24px) |
| 被录进视频 | 接受（always_on_top=true） | 用户主动选 display/window 录制，预期接受；不被录需 always_on_top=false 但会失去置顶 |
| 鼠标穿透 | **不穿透** | pill 必须能直接点按钮；穿透需复制 RecordAnnotation 的 33ms poller，复杂度高，收益低 |
| 关闭时机 | 跟随 stop_and_store（与 RecordAnnotation 同） | 四处都调 close_control_window：main.rs stop-requested handler + record_hotkey handle_stop + record_commands record_kill + tray.rs toggle 停止路径（2026-07-29 补） |

## 与 RecordAnnotation 互斥

`start_with_config` 成功后：
- `Source::Area` → 创建 RecordAnnotation（`record_annotation_window::create_annotation_window` 内置过滤）
- `Source::Display` / `Source::Window` → 创建控制浮窗（`record_control_window::create_control_window` 内置过滤）
- 两者不会同时出现（Source 是单选）

## 不变量

1. **pill 时刻可点**：录制中按钮立即响应（不穿透、不被遮挡）
2. **三条 stop 路径都关浮窗**：stop-requested handler / record_hotkey handle_stop / record_kill（异常退出）
3. **Area 录制无 pill**：避免 area 录制时同时有 RecordAnnotation 停止按钮 + pill 停止按钮（UX 冲突）
4. **失败路径不残留**：stop_and_store 失败时浮窗前端监听 `record://stop-failed` 自行 hide

## 实施（5 个新增 + 5 个改动）

### 新增
| 文件 | 作用 |
|---|---|
| `crates/desktop/src/record_control_window.rs` | 后端：create_control_window / close_control_window（仿 record_window.rs） |
| `crates/desktop/frontend/src/pages/RecordControl/index.tsx` | 前端组件：pill + 红点 + 时长 + 暂停 + 停止 |
| `crates/desktop/frontend/src/entries/record-control-main.tsx` | 入口（5 行，mountApp） |
| `crates/desktop/frontend/record-control.html` | 空壳 HTML（主题恢复 script + root + module） |
| `docs/superpowers/specs/2026-07-26-record-control-window.md` + `plans/...` | 本文 + plan |

### 改动
| 文件 | 改动 |
|---|---|
| `main.rs` | `mod record_control_window;` + stop-requested handler 加 close_control_window |
| `record_commands.rs` | start_with_config 成功后 create_control_window；record_kill 加 close（修 RecordAnnotation kill 泄漏） |
| `record_hotkey.rs` | handle_stop 加 close_control_window |
| `vite.config.ts` | input 加 record-control |
| `capabilities/default.json` | windows 数组加 record_control_window |

## 事件流（停止路径）

```
浮窗停止按钮 → emit("record://stop-requested", {from:"control"})
            ↓
main.rs L1104 listen → stop_and_store(session, app, false, None)
                    ↓
                    Ok(Some(meta)) → close_annotation_window + close_control_window + emit record://stopped
                    Err(e)         → emit record://stop-failed → 浮窗 listen hide
```

暂停/恢复：浮窗直接 `invoke("record_pause")` / `invoke("record_resume")`（via useRecordSession hook）。

## 位置算法（2026-07-26 修复副屏 bug）

**Bug**：原 `compute_position` 写 `let _ = display_id;` 丢弃 CGDirectDisplayID、永远用 `app.primary_monitor()`，且 `Monitor::position()` 返回物理像素未除 scale → 双重错误导致副屏录制时 pill 跑到主屏右下角。

**修复**：
```rust
fn compute_position(app: &AppHandle, source: &Source) -> (f64, f64) {
    // 1. Display 录制：display_id 是 CGDirectDisplayID，直接 CGDisplay::bounds() 拿逻辑边界
    if let Source::Display { display_id } = source {
        if let Some((ox, oy, w, h)) = cg_display_logical_bounds(*display_id) {
            return pill_bottom_right(ox, oy, w, h);
        }
        log::warn!("[record] CGDisplay::bounds() 查不到 display_id={display_id}，回退主屏");
    }
    // 2. 回退：Tauri Monitor（window 录制、或 CG 查询失败）——position/size 都是物理像素，÷ scale
    let m = app.primary_monitor()?.unwrap();
    let scale = m.scale_factor();
    pill_bottom_right(
        m.position().x as f64 / scale,
        m.position().y as f64 / scale,
        m.size().width as f64 / scale,
        m.size().height as f64 / scale,
    )
}

fn pill_bottom_right(origin_x, origin_y, w, h) -> (f64, f64) {
    // 注意：副屏在主屏左/上时 origin 是负数，不能 .max(0.0)（会推回主屏）
    (origin_x + w - WIDTH - 16.0, origin_y + h - HEIGHT - 16.0)
}
```

**关键点**：
- `CGDisplay::new(id).bounds()` 返回**逻辑** CGRect（CoreGraphics 原生 points，已含 scale）—— 比 Tauri Monitor heuristic 命中更可靠（Tauri Monitor 不暴露 CGDirectDisplayID）。
- 副屏在主屏**左侧/上方**时 `bounds.origin` 是负数，pill 坐标也应是负数（落在副屏坐标空间内）——不能 `.max(0.0)`。
- 单元测试覆盖 3 种场景：主屏（origin=0,0）、左侧副屏（origin_x<0）、上方副屏（origin_y<0）。

## 验证

详见 plan 验证章节。关键 e2e 场景：
1. display 录制 → 右下角 pill 出现 → 点停止 → pill 消失
2. window 录制 → 主屏右下角 pill（fallback）
3. 暂停/恢复：红点变灰 + 时长停 / 恢复
4. ESC 停止：pill 消失（验证 record_hotkey 路径）
5. tray 停止：pill 消失
6. kill 路径：pill 不残留
7. area 录制：无 pill（只有 RecordAnnotation）

## 不在范围

- ~~window 录制精确显示器定位（MVP fallback 主屏）~~ → window 录制仍 fallback 主屏（window_id → display 查询复杂），display 录制已精确（2026-07-26）
- F12 编码参数 / F13 文件管理 / F14 录制浮窗的其他形态（P1 其他项）
- 浮窗「不被录进视频」开关（未来可选）
