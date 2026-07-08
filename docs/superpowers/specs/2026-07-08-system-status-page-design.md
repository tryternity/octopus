# 系统状态页 设计

- 日期：2026-07-08
- 分支：worktree-system-status-page
- 状态：设计中

## 背景

设置窗（`settings_window`，前端 React 19 + TS + Tailwind v4）现有 4 个 tab：系统设置 / 剪贴管理 / 模型管理 / 提示词。团队在排查内存类问题（如「截图窗口 Object URL 内存泄漏」）时，没有任何界面能直观看到 octopus 进程的内存/CPU 占用与各模型常驻情况。本设计在设置窗新增「系统状态」tab，提供实时资源监控 + 短时趋势，辅助诊断。

## 目标

- 展示 octopus 进程的内存（RSS）与 CPU 占用，含最近 2 分钟趋势
- 展示各本地模型（ASR/OCR/VAD）的加载状态与「估算」占用内存
- 系统总 CPU/内存作为参考

## 非目标（YAGNI，本期不做）

- 云端模型（polish/LLM，走 HTTP 不占本地内存）统计
- 长时间历史 / 导出 / 暂停采样（完整诊断面板）
- per-core CPU 拆分
- per-model 精确内存（同进程 ort 架构下无法 OS 级拆分，仅给估算）
- ASR active/cached 引擎细分（仅展示「已加载」）

## 用户场景

开发者打开设置窗 → 切到「系统状态」→ 看到 RSS/CPU 数值与折线、各模型加载情况；加载一个 ASR 模型后 RSS 折线上涨、模型列表新增该条目，用于判断「加载某模型涨了多少内存」「是不是有泄漏在持续涨」。

## 架构与数据流

后端持续采样 + 推送，前端订阅展示：

```
后端 SystemStatusSampler (Tauri State, 单例)
  ├ ring buffer: RSS / CPU 时间序列 (2s × 60 = 2 分钟)
  ├ ModelMemoryRegistry: { model_id → 估算字节 }
  └ 后台采样循环 (tokio, 每 2s):
      sysinfo 取 RSS+CPU → 更新 buffer → app.emit("system-status", snapshot)

命令 get_system_status → 返回当前完整快照（首屏用）

前端 SystemPanel.tsx（设置窗新 tab「系统状态」）
  mount : invoke('get_system_status') 拿全量
  listen: 'system-status' 每 2s 增量更新
  unmount: unlisten
```

## 后端设计

### 依赖

- `crates/desktop/Cargo.toml` 新增 `sysinfo`

### 新增模块 `crates/desktop/src/system_status_commands.rs`

**`SystemStatusSampler`**（`tauri::State`，应用启动时单例 `manage`）

- `ring_buffer: VecDeque<SamplePoint>`，容量 60，满则弹出最旧
- `current: SystemStatusSnapshot`，最近一次完整快照（供 invoke 返回）
- `registry: ModelMemoryRegistry`
- 启动 `tokio::spawn` 采样循环：每 2s 用 `sysinfo::System` 刷新，读 octopus 进程（pid = `std::process::id()`）的 `memory()` / `cpu_usage()`，及系统级 `used_memory()` / `global_cpu_usage()`；推入 ring buffer；组装快照；`app_handle.emit("system-status", snapshot)`
- 注：`sysinfo` 首次 `cpu_usage()` 返回 0，常驻循环下第二次采样起即正常

**命令**

- `#[tauri::command] get_system_status(sampler) -> SystemStatusSnapshot`：返回 `current`（首屏全量）
- 注册到 `main.rs` 的 `generate_handler!`

**`ModelMemoryRegistry`**：`Arc<Mutex<HashMap<String, u64>>>`（Tauri State）

- `record_once(id, bytes)`：已存在则不覆盖
- `entries() -> Vec<ModelMemory>`：registry 内条目均代表「已加载」模型

### 模型内存插桩（加载点前后采 RSS 差值）

| 模型 | 插桩点 | id |
|---|---|---|
| ASR | `asr-local::AsrEngineManager::load_engine_into_cache` 前后 | `asr:<engine>` |
| OCR | `ocr::engine` 首次初始化前后 | `ocr:paddle` |
| VAD | `asr-local::vad` 首次加载前后 | `vad:silero` |

- 加载前读进程 RSS → 加载后再读 → 差值 `record_once` 进 registry
- 仅首次记录、不覆盖（ort arena 复用会让后续差值偏低甚至为负，覆盖会失真）
- 属「估算」，前端固定标注「约」

### 数据结构

```rust
struct SystemStatusSnapshot {
    sampled_at: f64,            // unix 秒
    process: ProcessStats,
    system: SystemStats,
    history: TimeSeries,        // ring buffer 内容，各 60 点
    models: Vec<ModelMemory>,   // registry 内已加载模型
}
struct ProcessStats { rss_bytes: u64, cpu_percent: f32 }
struct SystemStats  { total_memory_bytes: u64, used_memory_bytes: u64, cpu_percent: f32 }
struct TimeSeries   { rss: Vec<u64>, cpu: Vec<f32>, timestamps: Vec<f64> }
struct ModelMemory  { id: String, kind: String, display_name: String, estimated_bytes: Option<u64> }
```

## 前端设计

- `Settings/index.tsx` 的 `NAV_ITEMS` 增 `{ page: "system", icon: Activity, label: "系统状态" }`，switch 渲染 `<SystemPanel/>`
- 新增 `pages/Settings/SystemPanel.tsx`：
  - mount：`invoke<SystemStatusSnapshot>('get_system_status')` 初始化
  - `listen('system-status')`：按 `sampled_at` 去重取最新、更新 state
  - unmount：`unlisten`
- 复刻 `ModelsPanel.tsx` 的 Card + 进度条 pattern
- sparkline：轻量手画 SVG（不引第三方依赖），宽随容器

### 布局（已选 B）

```
┌─ 顶部汇总：进程总内存 1.2GB · 系统 CPU 8% ─────────────┐
├─ 内存（进程 RSS） ──────┬─ CPU（进程） ──────────────┤
│  1.2GB   ▁▂▃▅▆▇         │ 8%   ▂▃▅▄▃▂                │
├─────────────────────────┴────────────────────────────┤
│ 模型                                                  │
│  ASR paraformer   约 380MB                            │
│  OCR paddle       约 210MB                            │
│  VAD silero       约 30MB                             │
└───────────────────────────────────────────────────────┘
```
内存与 CPU 同级并排（各 ~300px，含 sparkline），模型列表整宽在下。

## 关键决策

1. **模型内存用「进程总 RSS + 加载增量估算」**：同进程 ort 无法 OS 级 per-model 拆分；给精确总量 + 标注「估算」的 per-model 增量，诚实不误导。
2. **CPU 维度**：进程自身 CPU% 为主、系统总 CPU% 作参考。
3. **采样 2s / 窗口 2 分钟（60 点）**：平衡趋势可见性与开销。
4. **刷新：后端定时推送 + 首屏 invoke 拉取**：采样集中、多窗口共享、首屏不延迟，符合现有事件模式（config-changed / download-progress）。
5. **采样循环常驻**：不随设置窗开关启停（多窗口共享、避免反复重建 sysinfo System）。

## 边界与错误处理

- sysinfo 读取失败 → 跳过本次采样、保留上次快照，不崩
- 采样任务 `tokio::spawn` 包裹，panic 不影响主进程
- 前端首屏 invoke 与 listen 首包可能重复 → 按 `sampled_at` 去重取最新
- pid 取 `std::process::id()`（octopus 自身），固定

## 测试

- ring buffer 容量上限与循环覆盖（单测）
- `ModelMemoryRegistry::record_once` 首次写入、已存在不覆盖（单测）
- sysinfo 失败降级（mock，单测）
- 前端 `SystemPanel`：mount→invoke、listen→更新、unmount→unlisten（组件测）
- 手动 e2e：加载 ASR 模型 → 状态页出现该条目、RSS 折线上涨

## 涉及文件

- 新增：`crates/desktop/src/system_status_commands.rs`、`crates/desktop/frontend/src/pages/Settings/SystemPanel.tsx`
- 修改：`crates/desktop/Cargo.toml`（+sysinfo）、`crates/desktop/src/main.rs`（注册命令 + manage State + 启动采样）、`crates/desktop/frontend/src/pages/Settings/index.tsx`（NAV_ITEMS + switch）、`asr-local` / `ocr` 加载点（插桩）

## 文档同步

实现完成后按 CLAUDE.md 要求更新 `docs/architecture.md`（新增 system_status 模块说明）。
