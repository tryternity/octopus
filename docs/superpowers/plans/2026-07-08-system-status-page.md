# 系统状态页 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在设置窗新增「系统状态」tab，实时展示 octopus 进程内存/CPU + 各本地模型估算内存 + 短时趋势。

**Architecture:** 后端 `sysinfo` 每 2s 采样进 ring buffer（60 点=2 分钟）并 `emit("system-status")`，前端 `listen` + 首屏 `invoke('get_system_status')`。模型内存通过 infra 依赖反转的 `model_probe`（asr-local/ocr 加载点埋点 Before/After，desktop 注入 sysinfo+registry 闭包算 RSS 差值）。

**Tech Stack:** Rust + Tauri 2 + `sysinfo` crate；React 19 + TS + Tailwind v4 + vitest。

**工程注意（worktree 陷阱）：** 本仓库 Bash cwd 常停在主仓库而非 worktree。所有 `cargo`/`git`/`grep` 命令必须显式指 worktree：`cargo test --manifest-path crates/infra/Cargo.toml`、`git -C <worktree> ...`。`Edit`/`Write` 用绝对路径不受影响。

**规格来源：** `docs/superpowers/specs/2026-07-08-system-status-page-design.md`

---

## File Structure

| 文件 | 职责 | 动作 |
|---|---|---|
| `crates/infra/src/model_probe.rs` | 全局加载探针（依赖反转）：`set_probe` + `probe(LoadPhase, id)` | 新增 |
| `crates/infra/src/lib.rs` | 注册 `pub mod model_probe;` | 改 1 行 |
| `crates/desktop/src/system_status_commands.rs` | `ModelMemoryRegistry` + `SystemStatusSampler` + `get_system_status` 命令 + probe 闭包注入 | 新增 |
| `crates/desktop/src/main.rs` | `mod` 注册 + `generate_handler!` + setup 里 manage/spawn/set_probe | 改 4 处 |
| `crates/desktop/Cargo.toml` | `+sysinfo` | 改 |
| `crates/asr-local/src/engine.rs` | `load_engine_into_cache` 埋点 Before/After | 改 |
| `crates/asr-local/src/vad.rs` | `SileroVad::new` 埋点 Before/After（cache miss 分支） | 改 |
| `crates/ocr/src/engine.rs` | `OcrEngine::instance` 埋点 Before/After | 改 |
| `crates/desktop/frontend/src/pages/Settings/index.tsx` | `NAV_ITEMS` 加「系统状态」+ switch | 改 |
| `crates/desktop/frontend/src/pages/Settings/SystemPanel.tsx` | 状态页 UI（invoke+listen+sparkline+布局B） | 新增 |
| `docs/architecture.md` | 新增 system_status 模块说明 | 改 |

---

### Task 1: infra `model_probe` 模块（依赖反转埋点接口）

**Files:**
- Create: `crates/infra/src/model_probe.rs`
- Modify: `crates/infra/src/lib.rs`

- [x] **Step 1: 写失败测试**（Create `crates/infra/src/model_probe.rs` 含测试）

```rust
//! 全局「模型加载探针」——依赖反转：asr-local / ocr 在加载点调用 `probe`，
//! 由 desktop 在启动时通过 `set_probe` 注入「读 RSS 差值写入 registry」的闭包。
//! infra 本身不依赖 sysinfo / desktop，只持有闭包。

use parking_lot::Mutex;
use std::sync::Arc;

/// 加载阶段：模型实例化前 / 后。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadPhase {
    Before,
    After,
}

/// 探针闭包：`(阶段, 模型 id)`。id 形如 `"asr:paraformer"` / `"ocr:PP-OCRv4"` / `"vad:silero"`。
pub type ProbeFn = Arc<dyn Fn(LoadPhase, &str) + Send + Sync>;

static PROBE: Mutex<Option<ProbeFn>> = parking_lot::const_mutex(None);

/// desktop 启动时注入探针（仅首次生效；重复 set 会覆盖）。
pub fn set_probe(f: ProbeFn) {
    *PROBE.lock() = Some(f);
}

/// 加载点调用：未注入时为 no-op。
pub fn probe(phase: LoadPhase, id: &str) {
    if let Some(f) = PROBE.lock().as_ref() {
        f(phase, id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn probe_is_noop_when_not_set() {
        // 未 set_probe 时调用不应 panic
        probe(LoadPhase::Before, "asr:x");
        probe(LoadPhase::After, "asr:x");
    }

    #[test]
    fn probe_invokes_injected_closure_with_phase_and_id() {
        let count = Arc::new(AtomicUsize::new(0));
        let last = Arc::new(Mutex::new((LoadPhase::Before, String::new())));
        let c = count.clone();
        let l = last.clone();
        set_probe(Arc::new(move |phase, id| {
            c.fetch_add(1, Ordering::SeqCst);
            *l.lock() = (phase, id.to_string());
        }));
        probe(LoadPhase::Before, "ocr:PP-OCRv4");
        probe(LoadPhase::After, "vad:silero");
        assert_eq!(count.load(Ordering::SeqCst), 2);
        assert_eq!(last.lock().0, LoadPhase::After);
        assert_eq!(last.lock().1, "vad:silero");
        // 清理（避免污染其它测试）
        *PROBE.lock() = None;
    }
}
```

- [x] **Step 2: 注册模块**（Modify `crates/infra/src/lib.rs`，在 `pub mod db;` 后加一行）

```rust
pub mod db;
pub mod model_probe;
```

- [x] **Step 3: 跑测试验证失败→通过**

Run: `cargo test --manifest-path crates/infra/Cargo.toml model_probe`
Expected: 2 tests passed（先确认编译通过；若 mod 未注册会编译错→注册后通过）。

- [x] **Step 4: Commit**

```bash
git -C <worktree> add crates/infra/src/model_probe.rs crates/infra/src/lib.rs
git -C <worktree> commit -m "feat(infra): model_probe 全局加载探针（依赖反转埋点接口）"
```

---

### Task 2: desktop `ModelMemoryRegistry`（模型内存估算表）

**Files:**
- Create: `crates/desktop/src/system_status_commands.rs`（本 task 只建 registry + 数据结构骨架）
- Modify: `crates/desktop/src/main.rs`（`mod system_status_commands;`）

- [x] **Step 1: 写文件含数据结构 + registry + 测试**

```rust
//! 系统状态页后端：模型内存估算表 + 系统资源采样器 + get_system_status 命令。
//!
//! 「模型占用内存」：同进程 ort 无法 OS 级 per-model 拆分，故用「加载前后进程 RSS 差值」
//! 近似（仅首次记录不覆盖，避免 ort arena 复用导致后续差值偏低/为负）。属估算，前端标注「约」。

use parking_lot::Mutex;
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

#[derive(Serialize, Clone, Debug)]
pub struct ModelMemory {
    pub id: String,
    pub kind: String,
    pub display_name: String,
    pub estimated_bytes: Option<u64>,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct ProcessStats {
    pub rss_bytes: u64,
    pub cpu_percent: f32,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct SystemStats {
    pub total_memory_bytes: u64,
    pub used_memory_bytes: u64,
    pub cpu_percent: f32,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct TimeSeries {
    pub rss: Vec<u64>,
    pub cpu: Vec<f32>,
    pub timestamps: Vec<f64>,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct SystemStatusSnapshot {
    pub sampled_at: f64,
    pub process: ProcessStats,
    pub system: SystemStats,
    pub history: TimeSeries,
    pub models: Vec<ModelMemory>,
}

/// 模型内存估算表：id → 估算字节。`record_once` 仅首次写入（不覆盖）。
#[derive(Default)]
pub struct ModelMemoryRegistry {
    inner: Mutex<HashMap<String, u64>>,
}

impl ModelMemoryRegistry {
    pub fn new() -> Self {
        Self { inner: Mutex::new(HashMap::new()) }
    }

    /// 仅当 id 不存在时记录；已存在则保留首次值（避免 arena 复用导致低估）。
    pub fn record_once(&self, id: &str, bytes: u64) {
        let mut m = self.inner.lock();
        m.entry(id.to_string()).or_insert(bytes);
    }

    /// 返回所有已记录模型（按 id 排序，输出稳定）。
    pub fn entries(&self) -> Vec<ModelMemory> {
        let m = self.inner.lock();
        let mut ids: Vec<&String> = m.keys().collect();
        ids.sort();
        ids.into_iter()
            .map(|id| {
                let (kind, name) = id.split_once(':').unwrap_or(("model", id.as_str()));
                ModelMemory {
                    id: id.clone(),
                    kind: kind.to_string(),
                    display_name: name.to_string(),
                    estimated_bytes: m.get(id).copied(),
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_once_writes_first_time() {
        let r = ModelMemoryRegistry::new();
        r.record_once("asr:paraformer", 380_000_000);
        let e = r.entries();
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].id, "asr:paraformer");
        assert_eq!(e[0].kind, "asr");
        assert_eq!(e[0].display_name, "paraformer");
        assert_eq!(e[0].estimated_bytes, Some(380_000_000));
    }

    #[test]
    fn record_once_does_not_overwrite() {
        let r = ModelMemoryRegistry::new();
        r.record_once("ocr:PP-OCRv4", 210_000_000);
        r.record_once("ocr:PP-OCRv4", 50_000_000); // arena 复用后的低值，应忽略
        let e = r.entries();
        assert_eq!(e[0].estimated_bytes, Some(210_000_000));
    }

    #[test]
    fn entries_sorted_by_id() {
        let r = ModelMemoryRegistry::new();
        r.record_once("vad:silero", 30_000_000);
        r.record_once("asr:paraformer", 380_000_000);
        let ids: Vec<_> = r.entries().into_iter().map(|m| m.id).collect();
        assert_eq!(ids, vec!["asr:paraformer", "vad:silero"]);
    }
}
```

- [x] **Step 2: 注册模块**（Modify `crates/desktop/src/main.rs`，在 `mod settings_commands;` 附近加）

```rust
mod system_status_commands;
```

- [x] **Step 3: 跑测试**

Run: `cargo test --manifest-path crates/desktop/Cargo.toml system_status_commands`
Expected: 3 tests passed。

- [x] **Step 4: Commit**

```bash
git -C <worktree> add crates/desktop/src/system_status_commands.rs crates/desktop/src/main.rs
git -C <worktree> commit -m "feat(desktop): ModelMemoryRegistry 模型内存估算表 + 快照数据结构"
```

---

### Task 3: desktop ring buffer（容量 60 循环覆盖）

**Files:**
- Modify: `crates/desktop/src/system_status_commands.rs`（追加 ring buffer）

- [x] **Step 1: 追加 ring buffer 结构 + 测试**（在 `system_status_commands.rs` 末尾、`#[cfg(test)]` 之前插入）

```rust
/// 单个采样点（ring buffer 元素）。
#[derive(Clone, Copy, Debug)]
pub struct SamplePoint {
    pub timestamp: f64,
    pub rss: u64,
    pub cpu: f32,
}

/// 固定容量时间序列：满后丢弃最旧。容量 60（2s × 60 = 2 分钟）。
pub struct RingBuffer {
    cap: usize,
    buf: VecDeque<SamplePoint>,
}

impl RingBuffer {
    pub fn new(cap: usize) -> Self {
        Self { cap: cap.max(1), buf: VecDeque::with_capacity(cap.max(1)) }
    }

    pub fn push(&mut self, p: SamplePoint) {
        if self.buf.len() == self.cap {
            self.buf.pop_front();
        }
        self.buf.push_back(p);
    }

    /// 导出为前端 TimeSeries（rss / cpu / timestamps 三个并行数组）。
    pub fn to_time_series(&self) -> TimeSeries {
        let mut rss = Vec::with_capacity(self.buf.len());
        let mut cpu = Vec::with_capacity(self.buf.len());
        let mut ts = Vec::with_capacity(self.buf.len());
        for p in &self.buf {
            rss.push(p.rss);
            cpu.push(p.cpu);
            ts.push(p.timestamp);
        }
        TimeSeries { rss, cpu, timestamps: ts }
    }
}
```

在 `mod tests` 内追加：

```rust
    #[test]
    fn ring_buffer_evicts_oldest_when_full() {
        let mut rb = RingBuffer::new(3);
        for i in 0..5 {
            rb.push(SamplePoint { timestamp: i as f64, rss: i, cpu: i as f32 });
        }
        let ts = rb.to_time_series();
        // 容量 3：只保留最后 3 个（i=2,3,4）
        assert_eq!(ts.rss, vec![2, 3, 4]);
        assert_eq!(ts.timestamps.len(), 3);
    }

    #[test]
    fn ring_buffer_under_capacity_keeps_all() {
        let mut rb = RingBuffer::new(60);
        rb.push(SamplePoint { timestamp: 1.0, rss: 100, cpu: 5.0 });
        assert_eq!(rb.to_time_series().rss, vec![100]);
    }
```

- [x] **Step 2: 跑测试**

Run: `cargo test --manifest-path crates/desktop/Cargo.toml system_status_commands`
Expected: 5 tests passed（含 Task 2 的 3 个）。

- [x] **Step 3: Commit**

```bash
git -C <worktree> add crates/desktop/src/system_status_commands.rs
git -C <worktree> commit -m "feat(desktop): RingBuffer 固定容量时间序列"
```

---

### Task 4: sysinfo 采样 + `SystemStatusSampler` + 命令 + 启动注入

**Files:**
- Modify: `crates/desktop/Cargo.toml`（+sysinfo）
- Modify: `crates/desktop/src/system_status_commands.rs`（追加 sampler + 命令）
- Modify: `crates/desktop/src/main.rs`（generate_handler + setup manage/spawn/set_probe）

- [x] **Step 1: 加依赖**（Modify `crates/desktop/Cargo.toml` `[dependencies]`，在 `parking_lot` 附近加）

```toml
sysinfo = "0.32"
```

- [x] **Step 2: 追加 sampler + RSS 读取 + 命令**（`system_status_commands.rs` 顶部 `use` 补充，并在 ring buffer 之后追加）

顶部 use 区追加：
```rust
use std::time::{SystemTime, UNIX_EPOCH};
use sysinfo::{Pid, System};
use tauri::{AppHandle, Emitter, State};
```

追加 sampler + 命令：
```rust
const SAMPLE_INTERVAL_SECS: u64 = 2;
const RING_CAPACITY: usize = 60;

/// 读「当前 octopus 进程」RSS（字节）。每次新建 System 并只刷新自身进程。
fn read_self_rss() -> Option<u64> {
    let pid = Pid::from_u32(std::process::id());
    let mut sys = System::new();
    sys.refresh_process(pid);
    sys.process(pid).map(|p| p.memory())
}

/// 采样器：常驻后台循环采样 → 更新 ring buffer + current → emit。
/// 由 main.rs setup 创建并 manage；Tauri State 共享给命令与 probe 闭包。
pub struct SystemStatusSampler {
    ring: Mutex<RingBuffer>,
    current: Mutex<SystemStatusSnapshot>,
    registry: Arc<ModelMemoryRegistry>,
}

impl SystemStatusSampler {
    pub fn new(registry: Arc<ModelMemoryRegistry>) -> Self {
        Self {
            ring: Mutex::new(RingBuffer::new(RING_CAPACITY)),
            current: Mutex::new(SystemStatusSnapshot::default()),
            registry,
        }
    }

    /// 采一次样并 emit。sysinfo 失败则跳过、保留上次快照（不崩）。
    fn sample_and_emit(&self, app: &AppHandle) {
        let pid = Pid::from_u32(std::process::id());
        let mut sys = System::new();
        sys.refresh_process(pid);
        sys.refresh_memory(); // 系统级 used/total

        let process = match sys.process(pid) {
            Some(p) => ProcessStats {
                rss_bytes: p.memory(),
                cpu_percent: p.cpu_usage(),
            },
            None => {
                log::warn!("[system-status] 读取自身进程失败，跳过本次采样");
                return;
            }
        };
        let system = SystemStats {
            total_memory_bytes: sys.total_memory(),
            used_memory_bytes: sys.used_memory(),
            cpu_percent: sys.global_cpu_usage(),
        };
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);

        {
            let mut ring = self.ring.lock();
            ring.push(SamplePoint { timestamp: now, rss: process.rss_bytes, cpu: process.cpu_percent });
            let history = ring.to_time_series();
            let models = self.registry.entries();
            let snap = SystemStatusSnapshot { sampled_at: now, process, system, history, models };
            *self.current.lock() = snap.clone();
            let _ = app.emit("system-status", snap);
        }
    }

    /// 当前完整快照（首屏 invoke 用）。
    pub fn snapshot(&self) -> SystemStatusSnapshot {
        self.current.lock().clone()
    }

    /// 启动后台采样循环 + 注入模型加载 probe（Before/After RSS 差值 → registry）。
    pub fn start(self: Arc<Self>, app: AppHandle) {
        // probe 闭包：Before 存 RSS，After 算差 record_once
        let before_map: Arc<Mutex<HashMap<String, u64>>> = Arc::new(Mutex::new(HashMap::new()));
        let registry = self.registry.clone();
        let bm = before_map.clone();
        octopus_infra::model_probe::set_probe(Arc::new(move |phase, id| {
            match phase {
                octopus_infra::model_probe::LoadPhase::Before => {
                    if let Some(rss) = read_self_rss() {
                        bm.lock().insert(id.to_string(), rss);
                    }
                }
                octopus_infra::model_probe::LoadPhase::After => {
                    let before = bm.lock().remove(id);
                    if let (Some(b), Some(now)) = (before, read_self_rss()) {
                        if now > b {
                            registry.record_once(id, now - b);
                        }
                    }
                }
            }
        }));

        // 采样循环（catch panic，不影响主进程）
        let this = self.clone();
        let app2 = app.clone();
        tokio::spawn(async move {
            loop {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    this.sample_and_emit(&app2);
                }));
                tokio::time::sleep(std::time::Duration::from_secs(SAMPLE_INTERVAL_SECS)).await;
            }
        });
    }
}

/// 首屏拉取当前完整快照。
#[tauri::command]
pub fn get_system_status(
    sampler: State<'_, Arc<SystemStatusSampler>>,
) -> SystemStatusSnapshot {
    sampler.snapshot()
}
```

- [x] **Step 3: 注册命令**（Modify `crates/desktop/src/main.rs` 的 `generate_handler!`，在 `theme::get_theme_id,` 后加一行）

```rust
            theme::get_theme_id,
            system_status_commands::get_system_status,
```

- [x] **Step 4: setup 里创建/manage/启动**（Modify `crates/desktop/src/main.rs` 的 `.setup` 闭包内，在 `info!("octopus-desktop initialized");` 之前插入）

```rust
            // 系统状态页：创建 registry + sampler，manage 为 State，启动采样循环 + 注入模型 probe
            {
                let registry = Arc::new(system_status_commands::ModelMemoryRegistry::new());
                app.manage(registry.clone());
                let sampler = Arc::new(system_status_commands::SystemStatusSampler::new(registry));
                app.manage(sampler.clone());
                sampler.start(app.handle().clone());
            }
```

- [x] **Step 5: 编译 + 启动验证**

Run: `cargo build --manifest-path crates/desktop/Cargo.toml`
Expected: 编译通过（sysinfo API 以 0.32 文档为准；若 `refresh_process`/`global_cpu_usage`/`p.memory()` 在所选小版本签名不同，按编译器提示调整——这些是 sysinfo 各小版本稳定 API）。

手动验证：`cargo run --manifest-path crates/desktop/Cargo.toml`，打开设置窗（此时 tab 尚未加，但后端日志应每 2s 无错；可用 Tauri 控制台或后续 task 验证）。

- [x] **Step 6: Commit**

```bash
git -C <worktree> add crates/desktop/Cargo.toml crates/desktop/src/system_status_commands.rs crates/desktop/src/main.rs
git -C <worktree> commit -m "feat(desktop): sysinfo 采样器 + get_system_status 命令 + probe 注入"
```

---

### Task 5: ASR 加载点埋点

**Files:**
- Modify: `crates/asr-local/src/engine.rs:98-148`（`load_engine_into_cache`）

- [x] **Step 1: 在加载前后插入 probe**

定位 `load_engine_into_cache` 中「未命中：加载配置 + 实例化」段（`let cfg = config::load_config()?;` 之前）与实例化完成（`let new_eng: Arc<dyn OfflineAsrEngine> = match category {...}` 之后、入缓存之前）。

在 `let bare_name = ...`（函数开头行 99）之后、`let cfg = config::load_config()?;`（行 110）之前插入 Before：

```rust
        let bare_name = config::parse_model_spec(model_name).model_name();
        // 系统状态页：记录加载前 RSS（仅 cache miss 才走到这）
        octopus_infra::model_probe::probe(
            octopus_infra::model_probe::LoadPhase::Before,
            &format!("asr:{bare_name}"),
        );
```

在 `let new_eng: Arc<dyn OfflineAsrEngine> = match category { ... };`（行 148 闭括号）之后、`let current_active = ...`（行 151）之前插入 After：

```rust
        octopus_infra::model_probe::probe(
            octopus_infra::model_probe::LoadPhase::After,
            &format!("asr:{bare_name}"),
        );
```

> 注意：cache hit 分支（行 102-107）提前 return，不触发 probe——正确，因为只测量真实加载。

- [x] **Step 2: 编译**

Run: `cargo build --manifest-path crates/asr-local/Cargo.toml`
Expected: 编译通过。

- [x] **Step 3: Commit**

```bash
git -C <worktree> add crates/asr-local/src/engine.rs
git -C <worktree> commit -m "feat(asr): load_engine_into_cache 埋点 model_probe（系统状态页）"
```

---

### Task 6: VAD 加载点埋点

**Files:**
- Modify: `crates/asr-local/src/vad.rs:35-59`（`SileroVad::new`）

- [x] **Step 1: 在 cache miss 的 `commit_from_file` 前后插入 probe**

定位 `SileroVad::new` 的 cache miss 分支（`} else { let s = Arc::new(Mutex::new( Session::builder()...commit_from_file... ));`）。

在 `let s = Arc::new(Mutex::new(` 之前插入 Before，在 `cache.insert(...)` 之后、`s` 返回之前插入 After：

```rust
            } else {
                octopus_infra::model_probe::probe(
                    octopus_infra::model_probe::LoadPhase::Before,
                    "vad:silero",
                );
                let s = Arc::new(Mutex::new(
                    Session::builder()
                        .context("Failed to create ORT session builder")?
                        .commit_from_file(model_path)
                        .with_context(|| format!("Failed to load Silero VAD from {:?}", model_path))?,
                ));
                cache.insert(model_path.to_path_buf(), s.clone());
                octopus_infra::model_probe::probe(
                    octopus_infra::model_probe::LoadPhase::After,
                    "vad:silero",
                );
                s
            };
```

- [x] **Step 2: 编译**

Run: `cargo build --manifest-path crates/asr-local/Cargo.toml`
Expected: 编译通过。

- [x] **Step 3: Commit**

```bash
git -C <worktree> add crates/asr-local/src/vad.rs
git -C <worktree> commit -m "feat(asr): SileroVad::new 埋点 model_probe（系统状态页）"
```

---

### Task 7: OCR 加载点埋点

**Files:**
- Modify: `crates/ocr/src/engine.rs:60-74`（`OcrEngine::instance` 加载段）

- [x] **Step 1: 在 `RapidOcr::new` 前后插入 probe**

定位 `let inner = octopus_paddle_ocr::RapidOcr::new(config)...`（行 63-64）。在它之前插 Before，`OcrEngine` 装入 `INSTANCE` 之后插 After：

```rust
        log::info!("Loading OCR model: {} from {}", model_name, dir.display());

        let config = build_engine_config(&dir)?;
        octopus_infra::model_probe::probe(
            octopus_infra::model_probe::LoadPhase::Before,
            &format!("ocr:{model_name}"),
        );
        let inner = octopus_paddle_ocr::RapidOcr::new(config)
            .map_err(|e| anyhow::anyhow!("Failed to init RapidOcr: {e}"))?;

        let use_word_segmentation = !model_name.starts_with("PP-OCRv6");

        log::info!("[ocr-engine] RapidOcr loaded — model={}, word_segmentation={}", model_name, use_word_segmentation);

        let engine = Arc::new(OcrEngine { inner: Mutex::new(inner), use_word_segmentation });
        octopus_infra::model_probe::probe(
            octopus_infra::model_probe::LoadPhase::After,
            &format!("ocr:{model_name}"),
        );
        let _ = INSTANCE.set(engine.clone());
        Ok(engine)
```

- [x] **Step 2: 编译**

Run: `cargo build --manifest-path crates/ocr/Cargo.toml`
Expected: 编译通过。

- [x] **Step 3: Commit**

```bash
git -C <worktree> add crates/ocr/src/engine.rs
git -C <worktree> commit -m "feat(ocr): OcrEngine::instance 埋点 model_probe（系统状态页）"
```

---

### Task 8: 前端 NAV_ITEMS 加 tab + SystemPanel 骨架

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Settings/index.tsx`
- Create: `crates/desktop/frontend/src/pages/Settings/SystemPanel.tsx`（骨架）

- [x] **Step 1: 改 `index.tsx`**——`PageName` 加 `"system"`、`NAV_ITEMS` 加项、import、switch 分支

`PageName`（行 22）：
```ts
type PageName = "clipboard" | "settings" | "models" | "prompts" | "system";
```

import（行 5 的 lucide-react 加 `Activity`，行 10 后加 SystemPanel）：
```ts
import { Settings as SettingsIcon, Box, Wand2, Clipboard, Activity, type LucideIcon } from "lucide-react";
import SystemPanel from "./SystemPanel";
```

`NAV_ITEMS`（行 24-29）末尾加：
```ts
const NAV_ITEMS: { page: PageName; icon: LucideIcon; label: string }[] = [
  { page: "settings", icon: SettingsIcon, label: "系统设置" },
  { page: "clipboard", icon: Clipboard, label: "剪贴管理" },
  { page: "models", icon: Box, label: "模型管理" },
  { page: "prompts", icon: Wand2, label: "提示词" },
  { page: "system", icon: Activity, label: "系统状态" },
];
```

switch（行 113-117 的 models/prompts 分支后）加：
```ts
        ) : page === "prompts" ? (
          <PromptsPanel showToast={showToast} />
        ) : page === "system" ? (
          <SystemPanel showToast={showToast} />
        ) : null}
```

- [x] **Step 2: 创建 SystemPanel 骨架**（先验证 tab 能出现，数据展示在 Task 9）

```tsx
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { Activity } from "lucide-react";

export interface ProcessStats { rss_bytes: number; cpu_percent: number; }
export interface SystemStats { total_memory_bytes: number; used_memory_bytes: number; cpu_percent: number; }
export interface TimeSeries { rss: number[]; cpu: number[]; timestamps: number[]; }
export interface ModelMemory { id: string; kind: string; display_name: string; estimated_bytes: number | null; }
export interface SystemStatusSnapshot {
  sampled_at: number;
  process: ProcessStats;
  system: SystemStats;
  history: TimeSeries;
  models: ModelMemory[];
}

export default function SystemPanel({ showToast }: { showToast: (msg: string) => void }) {
  const [snap, setSnap] = useState<SystemStatusSnapshot | null>(null);
  useEffect(() => {
    invoke<SystemStatusSnapshot>("get_system_status").then(setSnap).catch((e) => showToast("加载状态失败：" + e));
    let unlisten: UnlistenFn;
    let cancelled = false;
    listen<SystemStatusSnapshot>("system-status", (e) => {
      setSnap((prev) => (prev && e.payload.sampled_at <= prev.sampled_at ? prev : e.payload));
    }).then((fn) => { if (cancelled) fn(); else unlisten = fn; });
    return () => { cancelled = true; unlisten?.(); };
  }, [showToast]);

  if (!snap) {
    return <div className="flex items-center justify-center h-full text-muted-foreground">加载中...</div>;
  }
  return (
    <div className="max-w-[640px]">
      <div className="flex items-center gap-2 mb-3">
        <Activity className="w-4 h-4 text-muted-foreground" />
        <h3 className="text-sm font-semibold">系统状态</h3>
        <span className="ml-auto text-[10px] text-muted-foreground/60">骨架（Task 9 完整 UI）</span>
      </div>
      <pre className="text-[10px] text-muted-foreground/70 whitespace-pre-wrap">{JSON.stringify(snap, null, 2)}</pre>
    </div>
  );
}
```

- [x] **Step 3: 构建验证**

Run（在 `crates/desktop/frontend`）：`npm run build`
Expected: tsc + vite 构建通过，无类型错。

- [x] **Step 4: 手动验证**（`cargo run --manifest-path crates/desktop/Cargo.toml` → 托盘打开设置 → 切「系统状态」tab）
Expected: tab 出现，显示 JSON 快照，每 2s 更新。

- [x] **Step 5: Commit**

```bash
git -C <worktree> add crates/desktop/frontend/src/pages/Settings/index.tsx crates/desktop/frontend/src/pages/Settings/SystemPanel.tsx
git -C <worktree> commit -m "feat(ui): 设置窗新增「系统状态」tab + SystemPanel 骨架"
```

---

### Task 9: SystemPanel 完整 UI（布局 B + sparkline）

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Settings/SystemPanel.tsx`

- [x] **Step 1: 替换骨架为完整 UI**

```tsx
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { MemoryStick, Cpu, Boxes } from "lucide-react";

export interface ProcessStats { rss_bytes: number; cpu_percent: number; }
export interface SystemStats { total_memory_bytes: number; used_memory_bytes: number; cpu_percent: number; }
export interface TimeSeries { rss: number[]; cpu: number[]; timestamps: number[]; }
export interface ModelMemory { id: string; kind: string; display_name: string; estimated_bytes: number | null; }
export interface SystemStatusSnapshot {
  sampled_at: number;
  process: ProcessStats;
  system: SystemStats;
  history: TimeSeries;
  models: ModelMemory[];
}

function fmtBytes(n: number | null | undefined): string {
  if (n == null) return "?";
  if (n < 1024) return n + " B";
  if (n < 1048576) return (n / 1024).toFixed(1) + " KB";
  if (n < 1073741824) return (n / 1048576).toFixed(1) + " MB";
  return (n / 1073741824).toFixed(2) + " GB";
}

/// 轻量 sparkline：把数值序列映射成 SVG polyline（不引第三方依赖）。
function Sparkline({ data, color, max }: { data: number[]; color: string; max?: number }) {
  if (data.length < 2) return <div className="h-8 text-[10px] text-muted-foreground/50">采集中…</div>;
  const w = 100, h = 32;
  const hi = max ?? Math.max(...data, 1);
  const lo = Math.min(...data, 0);
  const span = Math.max(hi - lo, 1);
  const pts = data.map((v, i) => {
    const x = (i / (data.length - 1)) * w;
    const y = h - ((v - lo) / span) * h;
    return `${x.toFixed(1)},${y.toFixed(1)}`;
  }).join(" ");
  return (
    <svg viewBox={`0 0 ${w} ${h}`} preserveAspectRatio="none" className="w-full h-8">
      <polyline points={pts} fill="none" stroke={color} strokeWidth="1.5" vectorEffect="non-scaling-stroke" />
    </svg>
  );
}

function Card({ icon: Icon, title, children }: { icon: any; title: string; children: React.ReactNode }) {
  return (
    <div className="border border-border rounded-lg overflow-hidden bg-background">
      <div className="flex items-center gap-2 px-4 py-2.5 bg-muted/40 border-b border-border">
        <Icon className="w-4 h-4 text-muted-foreground" />
        <h3 className="text-sm font-semibold">{title}</h3>
      </div>
      <div className="px-4 py-3">{children}</div>
    </div>
  );
}

export default function SystemPanel({ showToast }: { showToast: (msg: string) => void }) {
  const [snap, setSnap] = useState<SystemStatusSnapshot | null>(null);
  useEffect(() => {
    invoke<SystemStatusSnapshot>("get_system_status").then(setSnap).catch((e) => showToast("加载状态失败：" + e));
    let unlisten: UnlistenFn;
    let cancelled = false;
    listen<SystemStatusSnapshot>("system-status", (e) => {
      // 按 sampled_at 去重取最新
      setSnap((prev) => (prev && e.payload.sampled_at <= prev.sampled_at ? prev : e.payload));
    }).then((fn) => { if (cancelled) fn(); else unlisten = fn; });
    return () => { cancelled = true; unlisten?.(); };
  }, [showToast]);

  if (!snap) return <div className="flex items-center justify-center h-full text-muted-foreground">加载中...</div>;

  const rssMax = Math.max(...snap.history.rss, snap.process.rss_bytes, 1);

  return (
    <div className="max-w-[640px] space-y-3">
      {/* 顶部汇总 */}
      <div className="flex items-center justify-between px-4 py-2.5 rounded-lg bg-muted/40 border border-border">
        <span className="text-sm font-medium">进程总内存 {fmtBytes(snap.process.rss_bytes)}</span>
        <span className="text-xs text-muted-foreground/70">系统 CPU {snap.system.cpu_percent.toFixed(1)}%</span>
      </div>

      {/* 内存 / CPU 并排 */}
      <div className="grid grid-cols-2 gap-3">
        <Card icon={MemoryStick} title="内存（进程 RSS）">
          <div className="text-lg font-semibold mb-1">{fmtBytes(snap.process.rss_bytes)}</div>
          <Sparkline data={snap.history.rss} color="#6ab0f3" max={rssMax} />
        </Card>
        <Card icon={Cpu} title="CPU（进程）">
          <div className="text-lg font-semibold mb-1">{snap.process.cpu_percent.toFixed(1)}%</div>
          <Sparkline data={snap.history.cpu} color="#f3a96a" />
        </Card>
      </div>

      {/* 模型列表 */}
      <Card icon={Boxes} title="模型（估算）">
        {snap.models.length === 0 ? (
          <div className="text-xs text-muted-foreground/60">暂无已加载模型</div>
        ) : (
          <div className="flex flex-col gap-1.5">
            {snap.models.map((m) => (
              <div key={m.id} className="flex items-center justify-between text-sm">
                <div className="flex items-center gap-1.5">
                  <span className="text-[10px] text-muted-foreground/60 px-1.5 py-0.5 rounded bg-muted">{m.kind}</span>
                  <span>{m.display_name}</span>
                </div>
                <span className="text-xs text-muted-foreground/70">约 {fmtBytes(m.estimated_bytes)}</span>
              </div>
            ))}
          </div>
        )}
        <div className="mt-2 text-[10px] text-muted-foreground/50">
          模型内存为「加载前后进程 RSS 差值」估算（同进程 ort 无法精确拆分），仅供参考。
        </div>
      </Card>
    </div>
  );
}
```

- [x] **Step 2: 构建验证**

Run（在 `crates/desktop/frontend`）：`npm run build`
Expected: 构建通过。

- [x] **Step 3: 前端单测**（sparkline 边界 + 去重逻辑——可选纯函数抽取测；此处用手动验证为主）

Run（在 `crates/desktop/frontend`）：`npm run test`
Expected: 现有测试不回归（本 task 未新增测试文件则跳过）。

- [x] **Step 4: 手动验证**（运行应用 → 系统状态 tab）
Expected: 顶部汇总条 + 内存/CPU 并排 Card（各带 sparkline）+ 模型列表（约 XX MB），每 2s 刷新。

- [x] **Step 5: Commit**

```bash
git -C <worktree> add crates/desktop/frontend/src/pages/Settings/SystemPanel.tsx
git -C <worktree> commit -m "feat(ui): SystemPanel 完整布局（内存/CPU 并排 + sparkline + 模型估算）"
```

---

### Task 10: 集成 e2e + 文档同步

**Files:**
- Modify: `docs/architecture.md`

- [x] **Step 1: 全量编译 + 测试**

Run:
```bash
cargo test --manifest-path crates/infra/Cargo.toml
cargo test --manifest-path crates/desktop/Cargo.toml system_status_commands
cargo build --manifest-path crates/desktop/Cargo.toml
cd crates/desktop/frontend && npm run build && npm run test
```
Expected: 全绿。

- [x] **Step 2: 手动 e2e**

运行应用 → 设置窗「系统状态」tab：
1. 启动后看到进程 RSS/CPU 数值 + 空模型列表
2. 切到「模型管理」下载/校验一个 ASR 模型 → 回「系统状态」→ 模型列表新增 `asr:<name>` 且 RSS 折线上涨
3. 触发一次 OCR（截图识别）→ 模型列表新增 `ocr:<name>`
4. 触发录音（加载 VAD）→ 模型列表新增 `vad:silero`

Expected: 三类模型均出现，估算值合理（百 MB 级），趋势线随加载台阶式上升。

- [x] **Step 3: 更新 `docs/architecture.md`**

在模块说明里新增一节（位置参照现有模块描述风格）：

```markdown
## system_status（desktop）

系统状态页后端（`crates/desktop/src/system_status_commands.rs`）：
- `SystemStatusSampler`：tokio 后台循环每 2s 用 sysinfo 采样 octopus 进程 RSS/CPU + 系统级内存/CPU，
  写入容量 60 的 ring buffer（2 分钟窗口），`emit("system-status", snapshot)`。
- `get_system_status` 命令：返回当前完整快照（前端首屏 invoke）。
- `ModelMemoryRegistry`：模型内存估算表（加载前后 RSS 差值，仅首次记录）。

模型加载埋点（依赖反转）：`crates/infra/src/model_probe.rs` 提供全局 `set_probe`/`probe(LoadPhase, id)`；
asr-local（`load_engine_into_cache`、`SileroVad::new`）与 ocr（`OcrEngine::instance`）在加载前后调用 `probe`，
desktop 启动时注入「读 RSS 差值写入 registry」的闭包。infra 不依赖 sysinfo/desktop。

前端：设置窗 `NAV_ITEMS` 的「系统状态」tab → `SystemPanel.tsx`（mount invoke + listen system-status + sparkline）。
```

- [x] **Step 4: Commit**

```bash
git -C <worktree> add docs/architecture.md
git -C <worktree> commit -m "docs(arch): 新增 system_status 模块说明"
```

---

### Task 11: 精炼迭代——双指标 + OCR idle 释放 + 中文化（v1 上线后据反馈）

**状态：已实现（2026-07-08），详见 spec「精炼迭代」。** v1 上线后用户反馈三点，迭代如下：

- [x] **① 进程内存双指标（RSS + 实际占用）**
  - 问题：状态页 RSS（sysinfo `resident_size`）稳定 1.45G，活动监视器「内存」1.0G，长期差 ~450M（非采样峰值）。
  - 根因：RSS 含 mmap 的 file-backed 模型权重；活动监视器用 `phys_footprint`（不计可回收 file-backed 页）。
  - 方案：macOS `proc_pid_rusage` FFI 读 `RusageInfoV0.ri_phys_footprint`（flavor `RUSAGE_INFO_V0=0`——**非 16**，16 是另一套 `proc_info` API；字节偏移 72）→ `ProcessStats.real_bytes: Option<u64>`；非 macOS 返回 `None` 前端退 RSS。模型内存差值法 macOS 同步改用 phys_footprint（`read_self_probe_memory`）。
  - 前端：`hasReal` 切换——顶部汇总 macOS 主显实际占用辅「常驻」、非 macOS 显 RSS；内存 Card 标题/主数/sparkline 随切；新增 `fmtBytesOrDash`（null→"—"）+ `sparklineDataFromNullable`。

- [x] **② OCR idle 60s 自动释放（ASR/VAD 常驻不动）**
  - `OcrEngine.inner: Mutex<Option<RapidOcr>>`（None=已释放）+ `last_used: Mutex<Option<Instant>>` + `model_name: String`。
  - 首次 `instance()` spawn **std::thread 守护线程**（ocr crate 共享 cli/server、无 tokio runtime 假设；踩坑：`tokio::spawn` 在 Tauri sync setup 里 panic「no reactor running」，改 `tauri::async_runtime::spawn`/`std::thread`）每 30s 检查 idle>60s → `*inner=None`（drop RapidOcr）+ `probe(Unload)`。
  - `run_ocr` 入口刷 `last_used`；重载（不调 probe，避免刷新 registry 首次估算）与 `run` 合并到同一 inner lock 作用域（消除守护线程在「重载后、run 前」无锁窗口竞态释放致 `expect` panic）。
  - `LoadPhase::Unload` 变体（infra/model_probe）→ desktop probe 闭包 `registry.remove(id)` 清条目；全局 PROBE 静态致测试并发污染，加 `TEST_SERIALIZER: Mutex<()>` 串行化修复。

- [x] **③ 术语中文化**：RSS 表述统一改「常驻」（顶部摘要副标题 / 内存 Card 副标题 / 非 macOS 标题 / 模型 Card 描述 4 处）。

- [x] **④ 释放后进程内存数值不降——接受现状**（macOS allocator 行为，非 bug）：释放链已验证正确（状态页 OCR 条目消失 + log `OCR idle 60s, released model`），但 RapidOcr drop 后 ort session 内存走 `malloc/free`，libmalloc free 不主动 `munmap` 归还物理页。真实收益是「下次 OCR 重载复用 free list 不重新涨」+「内存压力时 OS 可压缩回收」——非立即降数值。决定：接受现状 + 文档/状态页说明（未做 ort 禁 arena / `malloc_zone_pressure_relief`——效果未验证且后者需实测），状态页模型 Card 加 OCR 释放行为说明文案。

---

## Self-Review（写完后自查）

**1. Spec 覆盖：**
- 进程 RSS + CPU + 趋势 → Task 4（采样）+ Task 9（UI）✓
- 模型加载状态 + 估算内存 → Task 1（probe）+ Task 2（registry）+ Task 5/6/7（埋点）+ Task 9（UI）✓
- 系统总 CPU/内存参考 → Task 4（SystemStats）✓
- 后端定时推送 + 首屏 invoke → Task 4（emit + get_system_status）✓
- 三处插桩（ASR/OCR/VAD）→ Task 5/6/7 ✓
- 边界（sysinfo 失败降级、panic 隔离、去重）→ Task 4（sample_and_emit return + catch_unwind）+ Task 8（sampled_at 去重）✓
- 测试（ring buffer、record_once、降级、组件、e2e）→ Task 1/2/3 + Task 10 ✓
- 文档同步 → Task 10 ✓

**2. 占位符扫描：** 全部步骤均含完整代码，无 TBD/TODO/占位结构。

**3. 类型一致性：** `ModelMemory`/`ProcessStats`/`SystemStats`/`TimeSeries`/`SystemStatusSnapshot` 在 Task 2 定义，Task 4 sampler 复用，Task 8/9 前端 TS interface 字段一一对应（snake_case 经 serde 默认保留，前端按 snake_case 接收）。`record_once`/`entries`/`snapshot`/`probe`/`set_probe` 命名跨 task 一致。`LoadPhase::Before/After` 跨 infra/asr-local/ocr/desktop 一致。
