//! 系统状态页后端：模型内存估算表 + 系统资源采样器 + get_system_status 命令。
//!
//! 「模型占用内存」：同进程 ort 无法 OS 级 per-model 拆分，故用「加载前后进程 RSS 差值」
//! 近似（仅首次记录不覆盖，避免 ort arena 复用导致后续差值偏低/为负）。属估算，前端标注「约」。

use parking_lot::Mutex;
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use sysinfo::{Pid, ProcessesToUpdate, System};
use tauri::{AppHandle, Emitter, State};

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

const SAMPLE_INTERVAL_SECS: u64 = 2;
const RING_CAPACITY: usize = 60;

/// 读「当前 octopus 进程」RSS（字节）。每次新建 System 并只刷新自身进程。
///
/// 刻意独立于采样器的持久化 `System`：probe 路径只取 RSS 瞬时值（`memory()` 不需差分基线），
/// 且避免在模型加载期间持有采样器的 sys 锁。
fn read_self_rss() -> Option<u64> {
    let pid = Pid::from_u32(std::process::id());
    let mut sys = System::new();
    sys.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
    sys.process(pid).map(|p| p.memory())
}

/// 采样器：常驻后台循环采样 → 更新 ring buffer + current → emit。
/// 由 main.rs setup 创建并 manage；Tauri State 共享给命令与 probe 闭包。
///
/// `sys` 持久化：sysinfo 的 `cpu_usage()` / `global_cpu_usage()` 基于「两次刷新的时间差分」
/// 计算（单次刷新无基准、恒返回 0）。每 tick 新建 System = 永远首次 → 进程 CPU% 与系统
/// CPU% 恒为 0（正确性 bug）。故 System 跨 tick 保留，仅构造时预热一次基线
/// （首次读取仍 0、第二 tick 起准确，符合 spec 注明）。
pub struct SystemStatusSampler {
    sys: Mutex<System>,
    ring: Mutex<RingBuffer>,
    current: Mutex<SystemStatusSnapshot>,
    registry: Arc<ModelMemoryRegistry>,
}

impl SystemStatusSampler {
    pub fn new(registry: Arc<ModelMemoryRegistry>) -> Self {
        let mut sys = System::new();
        // 预热 CPU 基线：建立首次刷新时间戳，后续 tick 的差分才有意义。
        sys.refresh_cpu_usage();
        Self {
            sys: Mutex::new(sys),
            ring: Mutex::new(RingBuffer::new(RING_CAPACITY)),
            current: Mutex::new(SystemStatusSnapshot::default()),
            registry,
        }
    }

    /// 采一次样并 emit。sysinfo 失败则跳过、保留上次快照（不崩）。
    fn sample_and_emit(&self, app: &AppHandle) {
        let pid = Pid::from_u32(std::process::id());

        // 持久化 sys：refresh_processes 默认带 .with_cpu()（0.32.1 源码 system.rs:297-301
        // 证实），故进程级 cpu 无需 refresh_processes_specifics。
        let mut sys = self.sys.lock();
        sys.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
        sys.refresh_memory(); // 系统级 used/total
        sys.refresh_cpu_usage(); // 系统级 CPU（refresh_memory 不刷 CPU）

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
        // 先释放 sys 锁，再取 ring/current 锁——固定锁序（sys 先释放），避免持多锁。
        drop(sys);

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);

        let snap = {
            let mut ring = self.ring.lock();
            ring.push(SamplePoint { timestamp: now, rss: process.rss_bytes, cpu: process.cpu_percent });
            let history = ring.to_time_series();
            let models = self.registry.entries();
            let snap = SystemStatusSnapshot { sampled_at: now, process, system, history, models };
            drop(ring); // 先释放 ring 锁，再取 current 锁——两锁不嵌套，固定锁序
            *self.current.lock() = snap.clone();
            snap
        };
        // emit 在所有 sampler 锁之外：序列化 60 点历史 + 模型列表不持锁
        let _ = app.emit("system-status", snap);
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
                        // 覆盖式 insert：重试加载刷新 before；若 After 未触发（如 panic），
                        // 条目残留但不增长（model id 集合有界）。
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

        // 采样循环（catch panic，不影响主进程）。
        // 用 tauri::async_runtime::spawn 而非 tokio::spawn：start() 在 sync setup 闭包里调用，
        // 此时没有「当前 entered」的 tokio runtime 上下文（tokio::spawn 会 panic「no reactor running」）；
        // tauri::async_runtime::spawn 走全局 handle，不需当前线程 runtime 上下文。
        let this = self.clone();
        let app2 = app.clone();
        tauri::async_runtime::spawn(async move {
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
        assert_eq!(e.len(), 1, "同 id 二次记录不应新增条目");
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

    #[test]
    fn ring_buffer_zero_cap_clamps_to_one() {
        // cap=0 经 new 内 cap.max(1) 钳到 1，只留最新一个点。
        let mut rb = RingBuffer::new(0);
        rb.push(SamplePoint { timestamp: 1.0, rss: 1, cpu: 1.0 });
        rb.push(SamplePoint { timestamp: 2.0, rss: 2, cpu: 2.0 });
        assert_eq!(rb.to_time_series().rss, vec![2], "cap=0 钳到 1，只留最新");
    }
}
