# 系统状态页 Implementation Plan

> ✅ **已实现**（2026-07-08/09，system_status + model_probe 依赖反转）。所有 Task 已完成、checkbox 已勾选。下方为原始实施计划，保留作执行记录。

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

/// 模型内存估算表。
/// - `inner`：active 列表（状态页展示当前加载中模型）。
/// - `estimated`：首次估算值持久缓存，跨 unload/reload 保留（Task 14）。
#[derive(Default)]
pub struct ModelMemoryRegistry {
    inner: Mutex<HashMap<String, u64>>,
    estimated: Mutex<HashMap<String, u64>>,
}

impl ModelMemoryRegistry {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            estimated: Mutex::new(HashMap::new()),
        }
    }

    /// 记录 active 估算值（覆盖式），并把首次值持久化到 `estimated`（or_insert 仅首次）。
    pub fn upsert_active(&self, id: &str, bytes: u64) {
        self.estimated.lock().entry(id.to_string()).or_insert(bytes);
        self.inner.lock().insert(id.to_string(), bytes);
    }

    /// 取首次持久估算值（reload 时复用，避免重新算偏低的 RSS 差）。
    pub fn estimated(&self, id: &str) -> Option<u64> {
        self.estimated.lock().get(id).copied()
    }

    /// 移除 active 条目（模型卸载），保留 `estimated` 供下次 reload 复用首次值。
    pub fn remove(&self, id: &str) {
        self.inner.lock().remove(id);
    }

    /// 返回所有 active 模型（按 id 排序，输出稳定）。
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
    fn upsert_active_records_and_entries() {
        let r = ModelMemoryRegistry::new();
        r.upsert_active("asr:paraformer", 380_000_000);
        let e = r.entries();
        assert_eq!(e[0].id, "asr:paraformer");
        assert_eq!(e[0].estimated_bytes, Some(380_000_000));
    }

    #[test]
    fn estimated_keeps_first_value_against_low_overwrite() {
        // arena 复用偏低值不污染 estimated（Task 14）
        let r = ModelMemoryRegistry::new();
        r.upsert_active("ocr:PP-OCRv4", 210_000_000);
        r.upsert_active("ocr:PP-OCRv4", 50_000_000);
        assert_eq!(r.estimated("ocr:PP-OCRv4"), Some(210_000_000));
    }

    #[test]
    fn estimated_persists_across_unload_and_reload_restores_active() {
        let r = ModelMemoryRegistry::new();
        r.upsert_active("ocr:PP-OCRv4", 210_000_000);
        r.remove("ocr:PP-OCRv4");
        assert!(r.entries().is_empty());
        r.upsert_active("ocr:PP-OCRv4", r.estimated("ocr:PP-OCRv4").unwrap());
        assert_eq!(r.entries()[0].estimated_bytes, Some(210_000_000));
    }

    #[test]
    fn entries_sorted_by_id() {
        let r = ModelMemoryRegistry::new();
        r.upsert_active("vad:silero", 30_000_000);
        r.upsert_active("asr:paraformer", 380_000_000);
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
        // probe 闭包：Before 存 RSS，After 优先 estimated 复用首次值、否则算差 upsert_active。
        // key 含 ThreadId（Task 12）：多线程并发加载同一未缓存模型时按线程×模型配对防错拿。
        // probe 实现 clone 闭包后释放锁再调（Task 14④，避免持锁执行用户闭包）。
        let before_map: Arc<Mutex<HashMap<(std::thread::ThreadId, String), u64>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let registry = self.registry.clone();
        let bm = before_map.clone();
        octopus_infra::model_probe::set_probe(Arc::new(move |phase, id| {
            use octopus_infra::model_probe::LoadPhase;
            match phase {
                LoadPhase::Before => {
                    if let Some(m) = read_self_probe_memory() {
                        bm.lock().insert((std::thread::current().id(), id.to_string()), m);
                    }
                }
                LoadPhase::After => {
                    let key = (std::thread::current().id(), id.to_string());
                    let before = bm.lock().remove(&key);
                    // reload 场景 estimated 已有首次值（Task 14③）→ 复用，不算偏低差
                    if let Some(cached) = registry.estimated(id) {
                        registry.upsert_active(id, cached);
                    } else if let (Some(b), Some(now)) = (before, read_self_probe_memory()) {
                        if now > b {
                            registry.upsert_active(id, now - b);
                        }
                    }
                }
                LoadPhase::Unload => {
                    // 模型卸载（OCR idle / ASR 淘汰，Task 13④）：仅清 active，estimated 保留供 reload
                    registry.remove(id);
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

### Task 12: 后端审查修复——probe race ThreadId 隔离 + yt-dlp 原子下载（2026-07-08）

**状态：已实现（`ff4a37f`）。** 后端代码审查复查 3 问题，2 修 1 反馈：

- [x] **① probe race ThreadId 隔离**（`system_status_commands.rs`）：`load_engine_into_cache` 读缓存 miss 后释放读锁、到入缓存写锁前无保护，多线程并发加载同一未缓存模型（server 多连接 cache miss 同一 ASR 引擎）时 probe `before_map` 的 key 覆盖/错拿。修复：`before_map` key 从 `String` 改 `(ThreadId, String)`，Before/After 同线程配对。仅影响状态页估算显示，不影响加载正确性。
- [x] **② yt-dlp 原子下载**（`dlp/src/main.rs`）：`download_file` 直接写 dest，中断残留半成品被 `get_binary_path` 的 `exists()` 误判跳过下载 → 永久执行损坏 binary。修复：改写 `.part` 临时文件 + `drop` 句柄后 `fs::rename` 原子到 dest；`.part` 残留无害（dest 仍不存在 → 下次重新下载，`create` 自动 truncate 覆盖）。
- [x] **③ async 同步阻塞**（反馈不修）：`sample_and_emit` 在 async task 里同步调 sysinfo refresh，是反模式但影响极小（每 2s ms 级 syscall、独占 task 不饿死其他 worker），建议保持现状。

### Task 13: 后端审查二轮修复——OCR 坐标去重 + 双开 tab 竞态 + ASR 淘汰通知（2026-07-08）

**状态：已实现。** 后端审查二轮复查 5 问题，4 修 1 反馈（tsc 无错 / vitest 105 passed / cargo test 203 passed）：

- [x] **① OCR 长图坐标去重 fold max**（`ocr/src/engine.rs`）：`recognize_long_image_with_blocks` 用 `blocks.last().y+h` 更新 `covered_until_y`，det 框按 y 中心排序时末尾矮行底边非最大，极端混排（贯穿大框+底部矮行）少记 → 下一 chunk 重叠区行逃过去重 → 重复行。修复：改 `fold(covered_until_y, max)` 取 chunk 内真正最大底边。边缘场景（正常行高一致时与原逻辑等价）。
- [x] **② 截图 OCR 双开 tab 中间态丢失**（`compact_editor_commands.rs`）：`ocr_screenshot` 连续两次 `open_compact_editor_tab`，首次 `build()` 注册窗口 label 后第二次命中 `get_webview_window=Some` 走 emit（React 未 mount 丢）+ `push_pending_tab` 覆盖首个 tab → ocr 文本 tab 丢失（图片 tab 经 URL 注入幸存）。修复：`PENDING_TAB: Option` → `PENDING_TABS: Vec<PendingTabFull>`，新增 `open_compact_editor_tabs(items)` 批量一次 push + 一次 create/emit（无中间态）；窗口存在只 emit 不 push（防残留污染下次建窗；⚠️ Task 14① 改 `PENDING_TABS.is_empty()` 判 React mount）；前端 mount `get_pending_compact_tabs` take 全部与 URL 首个按 key 去重。`open_compact_editor_tab` 单开命令保留（转调批量版）。
- [x] **③ 截图 OCR blocks emit 早于 mount**（`screenshot_commands.rs` + `ImagePreview`）：`emit("ocr-screenshot://result")` 早于新窗 React mount 被丢，图片 tab 高亮遮罩不自动显示（须手点 ScanText 重跑）。修复：后端 `LAST_SCREENSHOT_OCR: Mutex<Option<(image_id, OcrResult)>>` 缓存 + `get_last_screenshot_ocr(image_id)` 命令（按 image_id 校验 take）；ImagePreview mount 时 invoke 拉取兜底，listen 供已 mount 即时收。`OcrResult`/`OcrTextBlock` 加 `Clone`。
- [x] **④ ASR 缓存淘汰漏 probe Unload**（`asr-local/src/engine.rs`）：`load_engine_into_cache` 的 `cache.remove(&k)` 淘汰旧引擎时未通知状态页（与 OCR idle 释放不对称）→ 状态页残留已淘汰模型估算。修复：淘汰后补 `probe(Unload, "asr:{k}")`。
- [x] **⑤ probe before_map 加载失败残留**（反馈不修）：Before 写入后 `?` 提前返回则 After 不执行，条目残留。但有界（线程×模型组合数、同线程重试覆盖），`system_status_commands.rs:299-300` 注释已说明，无需 RAII guard 增复杂度。

---

### Task 14: 后端审查三轮修复——PENDING_TABS 竞态 + 模型内存 estimated_cache + probe 持锁（2026-07-08）

**状态：已实现。** 后端审查三轮复查 4 问题，全部修复（tsc 无错 / vitest 105 passed / cargo test 439 passed）：

- [x] **① PENDING_TABS 窗口存在/未 mount 竞态**（`compact_editor_commands.rs`）：`open_compact_editor_tabs` 窗口存在分支假设「React 已 mount」直接 emit，但 `create_compact_editor_window` 同步注册 label 后 `get_webview_window` 立即返回 Some、React mount 滞后 50-200ms——连续第二次 open（用户快速点两个条目）的 emit 落在未 mount 窗口被丢。修复：窗口存在分支用 `PENDING_TABS.is_empty()` 判 React 是否已 mount take 清空（空=emit、非空=push 进队列让 mount 一并 take）。修正 Task 13 ② 的 emit-only 中间方案。
- [x] **② PENDING_TABS 关窗残留 stale**（`compact_editor_commands.rs`）：建窗失败 / React mount 前关窗 → PENDING_TABS 残留，下次建窗 `first()` 返回 stale 污染首屏。修复：else 分支 push 前 `take_pending_tabs()` 清空残留。
- [x] **③ 模型内存估算 reload 丢失/偏低**（`system_status_commands.rs` + `ocr/src/engine.rs`）：(a) OCR idle 释放 `probe(Unload)` remove 后，`run_ocr` 重载不调 probe → 状态页永久缺 OCR；(b) ASR 淘汰后重载 ort arena 复用致 RSS 差~0 → 估算偏低。修复：`ModelMemoryRegistry` 加 `estimated` 首次值持久缓存（`upsert_active` or_insert 首次、`remove` 仅清 active 保留 estimated）；probe After 优先 `estimated(id)` 命中复用首次值；OCR `run_ocr` 重载补 probe Before/After。
- [x] **④ probe 持锁调用户闭包**（`infra/src/model_probe.rs`）：`probe` 持 PROBE 锁调 f，fallback 路径 sysinfo 扫全部进程慢、阻塞其他线程。修复：clone `Option<ProbeFn>`（Arc +1）释放锁后再调 f。

---

### Task 15: 后端审查四轮——滚动截图鼠标轮询 cfg gate（2026-07-08）

**状态：Q2 已实现，Q1 反馈 false positive。** 后端审查四轮复查 2 问题（macOS cargo check 无错 / cargo test 439 passed）：

- [x] **② 滚动截图鼠标轮询 spawn 非 mac 编译失败**（`screenshot_commands.rs`）：`start_scroll_recording`（跨平台 `tauri::command`）内鼠标穿透轮询 spawn 块用 `core_graphics::event::CGEvent`（macOS-only dep）但无 cfg gate——`use core_graphics` + `CGEventSource`/`CGEvent` 调用在非 mac 编译失败（unresolved import）。修复：整个 spawn 块（变量定义 + spawn）用 `#[cfg(target_os = "macos")]` 块包裹，移除冗余内层 cfg（`get_window_pid_at_point`/`activate_app_by_pid` 本就 cfg-mac），非 mac 加 `let _ = interactive_rects` 抑制 unused。验证：macOS cargo check 过；linux 交叉编译因缺 webkit2gtk/gtk 系统库卡 build.rs（环境限制，非代码），但 core_graphics 已 gate 是 Cargo target-specific dep 的确定行为。
- [x] **① get_window_cocoa_frame 非 mac 编译失败**（反馈：**false positive**）：报告称该函数「无平台 gate」，但实测 `screenshot_commands.rs:799` 已有 `#[cfg(target_os = "macos")]`（调用点 601/888 也 gate）。报告漏看 799 行 cfg 属性，非真问题。

---

### Task 16: 后端审查五轮修复——流式 ASR 接 probe + StreamingSessionManager max_cache + 状态页 now<=b 占位（2026-07-09）

**状态：已实现。** 后端审查五轮（`d6c2d71` + `6e73257`）状态页相关 3 项修复（全量 cargo test 442 passed）：

- [x] **① 流式 ASR 引擎接入 model_probe**（`asr-local/src/streaming_engine.rs`，审查 Q4）：`StreamingSessionManager::switch_model` 加 `probe(Before/After)`（id=`asr:<bare>`，与离线 `load_engine_into_cache` 同前缀，同一模型算一条），驱逐时 `probe(Unload)`。修复前状态页完全不统计流式引擎内存（流式走独立 `StreamingSessionManager`，离线 probe 只埋 `AsrEngineManager`）。
- [x] **② StreamingSessionManager 加 max_cache=2 驱逐**（`asr-local/src/streaming_engine.rs`，审查 Q3）：原 spec §10 决策「不设上限（流式种类少）」未覆盖用户配置多流式引擎反复切换场景 → 内存无界增长 / OOM。`set_active` 入缓存前淘汰非 active（保护正用）+ `probe(Unload)`，对齐离线 `AsrEngineManager`。spec §10 / streaming plan YAGNI 表已同步修订。+2 单测。
- [x] **③ 状态页 After now<=b 占位**（`system_status_commands.rs`，审查 Q2）：After 分支 `now>before` 才 upsert，`now<=b`（ort arena 复用 / 并发释放）跳过 → 条目永久缺失 + estimated 永不写 → reload 仍走此分支。加 `mark_active_unmeasured(id)`：`now<=b` 时登记 active 占位（状态页显示「已加载」），不写 estimated（避免不可信近零值持久化，下次 reload 重算可自愈）。

> 同期 `d6c2d71` 另含滚动截图 `SCROLL_RECORDING` RAII guard、embedded 流式不预热离线 `AsrEngineManager`、final polish `catch_unwind` 兜底；`6e73257` 另含 CLI `transcribe-url` PCM 流跨 read 对齐——均非状态页范畴，见 `architecture.md`。

---

## Self-Review（写完后自查）

**1. Spec 覆盖：**
- 进程 RSS + CPU + 趋势 → Task 4（采样）+ Task 9（UI）✓
- 模型加载状态 + 估算内存 → Task 1（probe）+ Task 2（registry）+ Task 5/6/7（埋点）+ Task 9（UI）✓
- 系统总 CPU/内存参考 → Task 4（SystemStats）✓
- 后端定时推送 + 首屏 invoke → Task 4（emit + get_system_status）✓
- 三处插桩（ASR/OCR/VAD）→ Task 5/6/7 ✓
- 边界（sysinfo 失败降级、panic 隔离、去重）→ Task 4（sample_and_emit return + catch_unwind）+ Task 8（sampled_at 去重）✓
- 测试（ring buffer、upsert_active/estimated、降级、组件、e2e）→ Task 1/2/3 + Task 10/14 ✓
- 文档同步 → Task 10 ✓

**2. 占位符扫描：** 全部步骤均含完整代码，无 TBD/TODO/占位结构。

**3. 类型一致性：** `ModelMemory`/`ProcessStats`/`SystemStats`/`TimeSeries`/`SystemStatusSnapshot` 在 Task 2 定义，Task 4 sampler 复用，Task 8/9 前端 TS interface 字段一一对应（snake_case 经 serde 默认保留，前端按 snake_case 接收）。`upsert_active`/`estimated`/`entries`/`snapshot`/`probe`/`set_probe` 命名跨 task 一致。`LoadPhase::Before/After/Unload` 跨 infra/asr-local/ocr/desktop 一致。
