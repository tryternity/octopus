# 2026-07-18 Settings 鼠标滑动 CPU 高的调查结论

> 2026-07-18 · 调查「打开系统管理界面，鼠标在界面上滑动时 CPU 涨到 7-10%」
>
> **状态**：调查完成，**不优化**（结论：95% 在 macOS+Tauri+WebKit 框架层，octopus 业务代码占 3%）

## 1. 调查方法论

完整遵循 [`z_perf`](../../../~/.agents/skills/z_perf/) skill 的 measurement-first 工作流：

| 步骤 | 工具 | 关键产出 |
|---|---|---|
| Step 0：profiling 改造 | Cargo.toml `[profile.profiling]` | `debug="full"` + `split-debuginfo="off"` + `strip="none"`，让 samply/atos 能解析符号 |
| Step 1：可测量化 | samply 1kHz + xctrace Time Profiler | 两次 samply + 一次 xctrace，每次基线-触发-基线对照 |
| Step 2：采集证据 | samply record + xctrace record | 3 份 profile（run1 noisy / run2 clean / xctrace） |
| Step 3：分析瓶颈 | atos 反查 + Python 聚合 | 详见下文 |
| Step 4-5：埋点验证 | `log::warn!("[PERF-DEBUG]")` | 推翻 samply 的 unwind 假象 |

## 2. 核心测量数据（clean run）

### 时间窗口对照（xctrace，最准确）

```
                  主线程 CPU    总 CPU    平均单核     说明
────────────────────────────────────────────────────────────────
startup (0-10s)    560ms       1993ms     ~20%       app 初始化、托盘建立、热键注册
mousemove (10-24s) 651ms       1080ms     ~7.7%      用户在 SystemPanel 滑动鼠标 14s
idle (24-30s)        8ms         37ms     ~0.6%      停手后立刻回落
```

**鼠标滑动时主线程 CPU 涨 81 倍**（8ms → 651ms），这是真实开销，不是测量假象。

### samply clean run 数据（与 xctrace 互证）

```
窗口                时长    总 CPU    平均 CPU    样本/s
baseline1 (17-40s)   23s     15ms    0.07%      ~11
baseline2 (51-76s)   25s     29ms    0.12%      ~17
mousemove (77-111s)  34s   1100ms    3.24%      ~520   ← 涨 30x
baseline3 (112-139s) 27s     39ms    0.14%      ~19    ← 停手立刻回基线
```

## 3. 病灶定位（xctrace 原生数据）

mousemove 期主线程 651ms CPU 的 leaf 函数分布：

| 库 | 占比 | 关键函数 | 作用 |
|---|---|---|---|
| **AppKit** | ~35% | `_NSTrackingAreaAKManager _updateActiveTrackingAreasForWindowLocation`、`_routeMouseMovedEvent`、`windowNumberAtPoint`、`___collectTrackingAreasForTargetAndWinLoc` | **鼠标移动事件路由 + NSTrackingArea 命中测试** |
| **WebKit** | ~25% | `RemoteLayerTreeHost::updateLayerTree`、`RemoteLayerTreeDrawingAreaProxy::commitLayerTree`、`PageClientImpl::setCursor` | **WKWebView 渲染层树更新（hover 触发重绘）+ 光标切换** |
| **JavaScriptCore** | ~15% | `pas_thread_local_cache_for_all`、`scavenger_thread_main`、`flush_deallocation_log` | **JSC 垃圾回收（libpas scavenger 持续运行）** |
| CoreFoundation | ~10% | `__CFRunLoopRun`、`__CFRunLoopDoObservers`、`CFEqual` | RunLoop 调度 |
| SkyLight | ~5% | `SLS::TokenizedCoding::ReadDataProvider` | WindowServer IPC |
| **octopus-desktop** | ~3% | `tao::send_event`、`AppState::cleared` | Tauri 事件转发（业务代码 0%） |

### 关键结论

1. **没有 octopus 业务代码热点**——Rust 业务代码 + React 前端代码占 CPU < 3%
2. **70% CPU 在 macOS + WebKit 框架层**，主要是 AppKit 鼠标事件路由 + WebKit RemoteLayerTree 更新
3. **NSTrackingArea 命中测试**：每次鼠标移动，AppKit 遍历所有窗口的所有 NSTrackingArea 做命中测试（即使窗口不可见也会被 `windowNumberAtPoint` 检测）
4. **WebKit RemoteLayerTree 频繁更新**：鼠标 hover 时 WebKit 把 layer tree 打包通过 IPC 发到 UI 进程

## 4. 被排除的假设（含证据）

### ❌ 假设 1：SystemPanel / ClipboardPanel 的 CSS hover 风暴

**最初推测**（基于静态代码审查）：`ClipboardPanel` 有 18 处 hover: + 19 处 transition: + sticky header 的 backdrop-blur-sm + 4 处 hover:scale-110，× 50 行实例化 ≈ 1000 个 hover 节点 → 鼠标滑动触发 style recalc 风暴。

**推翻证据**：用户报 CPU 涨是在 **SystemPanel tab**（系统状态），SystemPanel **没有任何 hover / transition / backdrop-blur CSS**（CSS 层完全干净）。问题在 macOS 渲染层，不在 React/CSS 层。

### ❌ 假设 2：do_translate / execute_action_bar_inner 对鼠标事件过敏

**samply 火焰图显示**：mousemove 期主线程 `do_translate::closure` 被采样 869 次（vs baseline 6 次，**涨 400x**），tokio `Harness::poll` 占 83%。

**埋点验证**（决定性证据）：在 `do_translate` 和 `execute_action_bar_inner` 入口加 `log::warn!("[PERF-DEBUG] xxx ENTER")`，用 `RUST_LOG=warn` 跑 30s，**全程零调用**——日志只有 4 条 action-hotkey 启动注册。

**结论**：samply 的栈 unwind 在 macOS 26 + arm64e（系统库）+ arm64（主 binary）混合环境下**严重损坏**：
- leaf 函数频繁落在 Mach-O header 的字符串数据上（如 `0xc34` 实际是 `"...ks/CoreMedia.framewor"`）
- depth 19+ 大量 no_sym
- 同一个 PC 被误识别为多层栈帧（Harness::poll 自嵌套 18 层）
- 把栈中任意位置出现 `do_translate` 字样误归类为「正在执行 do_translate」

### ❌ 假设 3：tokio runtime 配置错误（current-thread 单点瓶颈）

**排查**：Tauri 2 默认走 `tokio::runtime::Runtime::new()` = multi-thread runtime。xctrace 数据显示有 16 个 tokio-rt-worker 线程，配置正确。主线程上看到的 tokio poll 是 samply unwind 假象。

### ❌ 假设 4：pin_window 的 NSTrackingArea 全局污染

**排查**：pin_window 只在用户钉图时创建，用户没钉图则完全不存在。xctrace mousemove 期采到的 `_NSTrackingAreaAKManager` 不是 pin_window 的，是 Tauri WebView 窗口自带的（WKWebView 自己加 tracking area 用于光标管理）。

### ⚠️ 假设 5：result_window 的 click-through poller 持续 IPC

**排查**：`start_click_through_poller`（`result_window.rs:143`）确实在跑，200ms tick 一次 `win.is_visible()` 走 IPC，每次涉及 NSWindow 引用 + autoreleasepool + 序列化。代码注释（line 137-138）自己也提到「闲置时 ~7% CPU + libpas scavenger 持续 spin」。

**xctrace 验证**：mousemove 14s 中只采到 3 个含 poller 的样本（200ms tick × 0.5% 采样率 = 期望 3-4 个，符合）。**poller 不是 CPU 大头**，但确实是 JSC libpas scavenger 持续活跃的潜在贡献者（每次 IPC 都涉及 JS heap 分配/释放）。

## 5. 不优化的理由

### 5.1 业务代码已经几乎为零开销

octopus-desktop 业务代码在 mousemove 期主线程只占 3%（`tao::send_event` + `AppState::cleared`，Tauri 框架自带的事件转发）。前端 React 代码（SystemPanel 的 SVG sparkline、4 个 Card、模型列表）在 xctrace 火焰图里**完全不可见**——WebKit 处理它们的开销已经包含在 RemoteLayerTree commit 里，且不是热点。

### 5.2 剩余 97% 在框架层，octopus 无法控制

| 框架开销 | octopus 能做什么 |
|---|---|
| AppKit 鼠标事件路由（`_routeMouseMovedEvent`）| 无 |
| AppKit NSTrackingArea 命中测试 | 减少 NSWindow 数量（见 5.3） |
| WebKit RemoteLayerTree 更新 | 减少 DOM 节点（SystemPanel 已经很少：4 Card + 2 SVG + 几个 Badge） |
| WebKit setCursor | 无 |
| JSC libpas scavenger | 减少 JS heap 分配（React 已经很省） |
| CoreFoundation RunLoop | 无 |
| SkyLight WindowServer IPC | 无 |

### 5.3 唯一可改的点风险不抵收益

**可改**：Tauri 启动时建的 3 个 always_on_top + transparent 窗口（`action_bar_window` / `overlay_window` / `result_window`，`main.rs:591/592/733`）会增加 AppKit 的 `windowNumberAtPoint` 开销——macOS 鼠标移动时要检测鼠标下是哪个窗口，窗口越多越贵。理论上改成懒创建（首次使用时才 build）能减少后台 hit-testing。

**不抵收益的原因**：
1. 这些窗口是功能必需——全局热键触发时要立刻 `show`，临时创建会引入首次响应延迟（几十 ms 级，用户可感知）
2. **预期收益不确定**：xctrace 数据中 `windowNumberAtPoint` 只占 5ms / 651ms ≈ 0.8%，懒创建窗口能省的远低于这个数字
3. **风险**：改动涉及 Tauri 窗口生命周期，可能引入新的 race condition（窗口未就绪时收到事件）

## 6. 测量基础设施保留

本次调查改造的工具链**永久保留**，供后续性能调查复用：

### 6.1 Cargo.toml profiling profile

```toml
[profile.profiling]
# inherits optimize：拿到与生产一致的 LTO/内联，再叠符号信息
inherits = "optimize"
debug = "full"
split-debuginfo = "off"
strip = "none"
```

构建命令：`cargo build --profile profiling -p octopus-desktop --features "embedded cloud"`

或直接用脚本：`./run-octopus.sh --profiling`

构建产物 ~104MB（含 DWARF 符号，体积 vs 普通 release 多约 7MB）。inherits `optimize` 保证 perf 测量时的 LTO 内联情况与生产 binary 一致——火焰图看到的函数边界/内联展开与用户实际跑的 release build 相同。代价：链接时间 +1~3 分钟（LTO）。

### 6.2 samply 采样流程（已知问题：macOS 26 栈 unwind 损坏）

```bash
cd crates/desktop
CARGO_TARGET_DIR=../../target samply record --output /tmp/z-perf/profile.json \
  -- ../../target/profiling/octopus-desktop
```

⚠️ **samply 0.13.1 在 macOS 26 + arm64e/arm64 混合环境下栈 unwind 损坏**：leaf 函数会落在 Mach-O header 的字符串数据上（如 `0xc34` 实际是 `"...ks/CoreMedia.framewor"`），depth 19+ 大量 no_sym。**macOS 上的备选**：用 `xctrace`（见 6.3）。

### 6.3 xctrace Time Profiler（macOS 原生，符号化准确）

```bash
./target/profiling/octopus-desktop & APP_PID=$!
sleep 3
xcrun xctrace record --template "Time Profiler" \
  --attach "$APP_PID" --time-limit 30s \
  --output /tmp/z-perf/xctrace.trace
kill $APP_PID

# 导出 XML
xcrun xctrace export --input /tmp/z-perf/xctrace.trace \
  --xpath '/trace-toc/run[@number="1"]/data/table[@schema="time-profile"]' \
  > /tmp/z-perf/xctrace-time-profile.xml

# 用 Instruments.app 看：
open /tmp/z-perf/xctrace.trace
```

### 6.4 埋点验证假设的范式

samply/xctrace 看到的"热点函数"可能只是栈中任意位置出现，**不是真的在执行**。验证范式：

1. 在怀疑的函数入口加 `log::warn!("[PERF-DEBUG] xxx ENTER")`
2. `RUST_LOG=warn ./target/profiling/octopus-desktop 2>&1 | grep PERF-DEBUG`
3. 触发场景（如鼠标滑动），看日志有没有输出
4. 没输出 = 该函数未被调用 = profile 中的"热点"是 unwind 假象

## 7. 中间产物

| 文件 | 内容 |
|---|---|
| `/tmp/z-perf/profile-run1-noisy.json` | samply run1（含下载/翻译任务，污染） |
| `/tmp/z-perf/profile-clean-symbolicated.json` | samply run2（纯滑鼠标，符号化） |
| `/tmp/z-perf/xctrace-mousemove.trace` | xctrace Time Profiler（最准确） |
| `/tmp/z-perf/xctrace-time-profile.xml` | xctrace 导出的 XML |
| `/tmp/z-perf/analyze.py`, `analyze_mousemove.py`, `xctrace_analyze.py` | 分析脚本 |
| `/tmp/z-perf/perf-debug.log` | 埋点验证日志（只有 4 条启动注册，零业务调用） |

## 8. 参考

- AGENTS.md「改动验证纪律」「物理/逻辑坐标转换」
- [z_perf skill](../../../~/.agents/skills/z_perf/)：完整工作流
- [2026-07-17-perf-batch-cpu-memory.md](./2026-07-17-perf-batch-cpu-memory.md)：上一轮性能优化批次
- [2026-07-17-perf-release-lto.md](./2026-07-17-perf-release-lto.md)：release profile LTO/strip（测量基础设施）
