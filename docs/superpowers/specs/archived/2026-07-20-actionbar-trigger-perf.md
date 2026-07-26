# 2026-07-20 Action Bar 触发性能优化

## 背景

用户反馈：召唤 action bar 有点迟滞，没有其他 launcher 工具（PopClip/Raycast）快。尤其**无选中场景**特别慢——按热键到 ActionBar 弹出有 1+ 秒延迟。

## 测量数据（debug build）

```
场景                detect_selection  gather  show  total
─────────────────────────────────────────────────────────────
None (无选中) #1    1239ms 🔥        —       0ms   1239ms
None (无选中) #2    1084ms 🔥        —       0ms   1085ms
Text (Sublime) #1   491ms            109ms   0ms   601ms
Text (Sublime) #2   511ms            77ms    0ms   589ms
Folder (Finder) #1  442ms            —       0ms   442ms
Folder (Finder) #2  417ms            —       0ms   417ms
```

**None 分支比选中场景慢一倍**——1.2s vs 0.4-0.6s。

## 病灶定位（detail timing）

`detect_selection` 内部 5 段串行：

```
None 分支（1251ms 拆解）：
  get_mouse_position:          0-11ms        ✅
  is_finder_frontmost(false):  184-413ms  ⚠️  AppleScript 启动
  is_sublime_frontmost(false): 216-222ms  ⚠️  AppleScript 启动
  simulate_copy:               349-394ms  🔥  osascript + delay 0.15
  sleep(200):                  202-205ms  🔥  固定 sleep 等剪贴板
```

**4 段 osascript 串行调用**，每个 200-400ms。osascript 启动开销（fork + AppleScript 编译 + System Events 通信）~200ms 是 macOS AppleScript 固有开销。

## 修复方案（A+B+C）

### 方案 B：frontmost 检测改 NSWorkspace（最大收益 ~400ms）

**问题**：`is_finder_frontmost` 和 `is_sublime_frontmost` 各跑独立 osascript 查询**完全相同的信息**——`bundle identifier of first application process whose frontmost is true`。

**修复**：用 `objc2-app-kit NSWorkspace.sharedWorkspace.frontmostApplication.bundleIdentifier` 直调，< 1ms。

- 抽出 `pub(crate) frontmost_bundle_id()` 在 `app_context/macos_ax.rs`
- `app_context/mod.rs`: `mod macos_ax` 改 `pub(crate) mod macos_ax` 让 sublime_plugin 可访问
- `is_finder_frontmost` / `is_sublime_frontmost` 各改为 NSWorkspace 直调

**设计原则**：frontmost 检测是基础能力，未来加更多应用检测不应增加 osascript 开销——NSWorkspace O(1)。

### 方案 A：simulate_copy 的 delay 0.15 改条件化（~150ms）

**问题**：`focus_tracker::simulate_copy_platform` 的 AppleScript 内固定 `delay 0.15`，给 `set_frontmost` 切焦点 + `keystroke "c"` 之间的 buffer。但仅在「octopus 是 frontmost 需切焦点」时才需要等待焦点切换。

**修复**：把 `delay 0.15` 移到 `if name of p is "octopus"` 分支内，正常情况（octopus 非 frontmost）直接 `keystroke "c"` 不 delay。

### 方案 C：sleep(200) 改 polling + 80ms 超时（~120ms）

**问题**：`thread::sleep(200ms)` 等剪贴板 changeCount 更新。Cmd+C 在无选中时不写剪贴板，changeCount 永远不递增，等满 200ms 纯浪费。

**修复**：改 polling 每 5ms 检查 changeCount，命中递增即退出。超时 200ms → 80ms（实测 Cmd+C 命中 changeCount 最坏 ~30-50ms，80ms 兜底防系统忙时延迟）。

**80ms 权衡**：太短可能错过系统繁忙时的延迟（误判有选中为无选中）；太长无收益。实测 Safari/Notes/Terminal 都能正确捕获（polling 提前命中，~30-50ms 退出）。

## 实测收益（debug build）

```
场景              优化前           优化后          加速比
None              1251ms    →     302-322ms       3.9-4.1x
Text (Sublime)    491ms     →     59-267ms        1.8-8.3x
Folder (Finder)   417ms     →     204-234ms       1.8-2.0x
File              —         →     204ms           
```

用户验证：4 个场景（None/Text/Folder/File）速度都可接受。

## 文件改动

| 文件 | 改动 |
|---|---|
| `app_context/macos_ax.rs` | `frontmost_app()` 改 `pub(crate)`，新增 `pub(crate) frontmost_bundle_id()` |
| `app_context/mod.rs` | `mod macos_ax` → `pub(crate) mod macos_ax` |
| `finder_selection.rs` | `is_finder_frontmost` 改用 `frontmost_bundle_id()` |
| `app_context/sublime_plugin.rs` | `is_sublime_frontmost` 改用 `frontmost_bundle_id()` |
| `focus_tracker.rs` | `simulate_copy_platform` 的 `delay 0.15` 改条件化 |
| `action_bar_commands.rs` | `sleep(200)` 改 polling + 80ms 超时 |

## 未做的方向（后续可继续）

~~- **CGEvent 替代 osascript 发 Cmd+C**~~ → **已完成**（2026-07-20/21）：抽 `keystroke` 模块用 CGEvent 直调（< 5ms），详见 [keystroke spec](2026-07-20-keystroke-module-design.md)。simulate_copy 改为三级 dispatch（WKWebView→osascript / Electron→post_to_pid / 原生→post_to_pid），detect_selection 无选中场景进一步降到 ~100ms。

当前所有方向已完成，无遗留后续。

## 2026-07-21 补充：微信 WKWebView polling 超时演变（实测调优）

原「80ms 超时」描述仅适用于 **CGEvent 路径**（原生/Electron app，Cmd+C 同步触发，changeCount < 50ms 递增）。微信等 WKWebView 嵌套 app 走 **osascript 路径**，超时需要更长。

### 演变过程

| 阶段 | polling 超时 | 微信成功率 | 备注 |
|---|---|---|---|
| `01b7f689` 初版 | 统一 80ms | ~50% | 仅测原生 app（Safari/Notes/Terminal），微信场景漏测 |
| `086cf381` 第 1 次修 | Osascript=400ms | ~90% | dispatch path 返回值 + 动态超时 |
| `0f8cf673` 误判撤销 | 统一 80ms | 完全失效 | 误以为"osascript 同步阻塞已含等待"，撤销 |
| `b813c13d` 最终版 | Osascript=**300ms**（常量）| ~90% | 用户实测确认 |

### 关键约束（osascript 脚本结构）

`keystroke.rs::send_via_osascript` 的 AppleScript 脚本结构有严格约束：

```applescript
-- ✅ 正确（稳定 ~90%）：
tell application "System Events"
    set frontProc to first process whose frontmost is true
    set procName to name of frontProc
    keystroke "c" using command down   -- 在 System Events 块作用域内
    return procName
end tell

-- ❌ 错误（完全失效，changeCount +0）：
tell application "System Events"
    set frontProc to first process whose frontmost is true
    tell frontProc                        -- 加了 tell frontProc 包裹
        keystroke "c" using command down
    end tell
end tell
```

机制推测：`tell frontProc` 把命令绑定到 WeChat process 的 AX 上下文，但 WKWebView 嵌套的 menu bar 不归 WeChat process 的 AX 拥有 → Cmd+C 没走菜单路由。

### 设计：dispatch path 返回值

`focus_tracker.rs::simulate_copy` 返回 `CopyDispatch` 枚举（`Osascript` / `CGEvent`），让 `detect_selection` 据此选 polling 超时：

```rust
let poll_timeout_ms = match copy_dispatch {
    CopyDispatch::Osascript => WECHAT_COPY_POLL_TIMEOUT_MS,  // 300
    CopyDispatch::CGEvent => 80,
};
```

常量 `WECHAT_COPY_POLL_TIMEOUT_MS = 300` 在 `action_bar_commands.rs` 顶部，方便后续根据实测调整。

### 教训

1. **用户实测数据优先于代码理论**：多次"复盘"都建立在错误假设上（osascript 行为没真正理解）
2. **「无意中起作用」的改动要识别并固化**：脚本结构改动（加 `set frontProc`）是无意中修复的，需理解机制才能避免回归
3. **跨 dispatch path 的超时不能一刀切**：CGEvent 同步快（< 50ms），osascript 异步慢（200-400ms）
