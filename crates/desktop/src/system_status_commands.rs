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
    /// macOS=phys_footprint（活动监视器「内存」列口径），其他平台=None（serde→null）。
    pub real_bytes: Option<u64>,
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
    /// phys_footprint 时序（macOS），其他平台空数组。
    pub real: Vec<Option<u64>>,
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
/// - `inner`：active 列表——状态页展示「当前加载中」的模型。
/// - `estimated`：首次估算值持久缓存，跨 unload/reload 保留。reload 时 ort arena
///   复用会让 RSS 差值~0（偏低），用首次值避免状态页显示错误的近零估算。
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
    /// 调用方：首次加载传算出的 RSS 差；reload 传 `estimated()` 取回的首次缓存值
    /// （避免重新算偏低的差值）。
    pub fn upsert_active(&self, id: &str, bytes: u64) {
        self.estimated.lock().entry(id.to_string()).or_insert(bytes);
        self.inner.lock().insert(id.to_string(), bytes);
    }

    /// 取首次持久估算值（reload 时复用，避免重新算偏低的 RSS 差）。
    pub fn estimated(&self, id: &str) -> Option<u64> {
        self.estimated.lock().get(id).copied()
    }

    /// 移除 active 条目（模型卸载），保留 `estimated` 供下次 reload 复用首次值。
    /// 不存在则 no-op。
    pub fn remove(&self, id: &str) {
        self.inner.lock().remove(id);
    }

    /// 标记模型已加载但无法测得 RSS 增量（After 时 `now <= before`：ort arena 复用 /
    /// 并发线程释放内存所致）。仅写 active 占位（0，状态页显示该模型「已加载、占用未测」），
    /// **不写 `estimated`**——避免把不可信的近零值持久化成首次估算，下次 reload 仍会重算差值。
    /// 若不登记，该模型会从状态页永久缺失，且 estimated 永不写入 → reload 仍走 now<=b 分支。
    pub fn mark_active_unmeasured(&self, id: &str) {
        self.inner.lock().insert(id.to_string(), 0);
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

/// 单个采样点（ring buffer 元素）。
#[derive(Clone, Copy, Debug)]
pub struct SamplePoint {
    pub timestamp: f64,
    pub rss: u64,
    /// phys_footprint（macOS），其他平台 None。
    pub real: Option<u64>,
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

    /// 导出为前端 TimeSeries（rss / real / cpu / timestamps 四个并行数组）。
    pub fn to_time_series(&self) -> TimeSeries {
        let mut rss = Vec::with_capacity(self.buf.len());
        let mut real = Vec::with_capacity(self.buf.len());
        let mut cpu = Vec::with_capacity(self.buf.len());
        let mut ts = Vec::with_capacity(self.buf.len());
        for p in &self.buf {
            rss.push(p.rss);
            real.push(p.real);
            cpu.push(p.cpu);
            ts.push(p.timestamp);
        }
        TimeSeries { rss, real, cpu, timestamps: ts }
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

/// macOS：通过 proc_pid_rusage 读自身进程 phys_footprint（活动监视器「内存」列口径）。
/// phys_footprint 不计 mmap 的 file-backed 页（模型权重），更接近「真实占用物理 RAM」。
/// 其他平台无此概念，返回 None（前端退 RSS）。
#[cfg(target_os = "macos")]
fn read_self_phys_footprint() -> Option<u64> {
    #[repr(C)]
    struct RusageInfoV0 {
        ri_uuid: [u8; 16],
        ri_user_time: u64,
        ri_system_time: u64,
        ri_pkg_idle_wkups: u64,
        ri_interrupt_wkups: u64,
        ri_pageins: u64,
        ri_wired_size: u64,
        ri_resident_size: u64,
        ri_phys_footprint: u64, // 字节偏移 72（ri_uuid 16B + 7×u64）
        ri_proc_start_abstime: u64,
        ri_proc_exit_abstime: u64,
    }
    extern "C" {
        // libproc（libSystem 已包含，无需 #[link]）。返回 0 成功，-1 失败。
        fn proc_pid_rusage(pid: i32, flavor: i32, buffer: *mut RusageInfoV0) -> i32;
    }
    // flavor：RUSAGE_INFO_V0 = 0（sys/resource.h）。注意这是 rusage flavor，
    // 不是 proc_info 的 PROC_PIDRUSAGE(=16)——后者是另一套 API 的 taste。
    const RUSAGE_INFO_V0: i32 = 0;

    let mut info: RusageInfoV0 = unsafe { std::mem::zeroed() };
    let r = unsafe { proc_pid_rusage(std::process::id() as i32, RUSAGE_INFO_V0, &mut info) };
    if r == 0 {
        Some(info.ri_phys_footprint)
    } else {
        None
    }
}

/// 非 macOS：无 phys_footprint 概念，返回 None。
#[cfg(not(target_os = "macos"))]
fn read_self_phys_footprint() -> Option<u64> {
    None
}

/// 模型内存差值法用的口径：macOS 优先 phys_footprint（模型权重虽 mmap 进 RSS，
/// 但 OS 可回收、不算真实占用，故差值用 phys_footprint 更贴近模型实际占用）；
/// 其他平台无 phys_footprint，退 RSS。
fn read_self_probe_memory() -> Option<u64> {
    read_self_phys_footprint().or_else(read_self_rss)
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
    /// 订阅者计数——SystemPanel mount +1 / unmount -1。
    /// 采样循环仅在 >0 时执行 sysinfo 刷新 + emit；无订阅者时纯 sleep，
    /// 避免闲置时持续 alloc（sysinfo 刷新每 tick 分配 Process/HashMap 节点）。
    /// 2026-07-17 性能优化：闲置时本采样器贡献持续 alloc 峰值。
    subscribers: std::sync::atomic::AtomicU32,
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
            subscribers: std::sync::atomic::AtomicU32::new(0),
        }
    }

    /// 订阅 +1（SystemPanel mount 时调）——开启后台采样。
    pub fn subscribe(&self) {
        self.subscribers.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// 订阅 -1（SystemPanel unmount 时调）——计数归零则后台采样循环空转 sleep。
    /// fetch_sub 后用 saturating_to 防御下溢（极端情况下 unmount 多调一次）。
    pub fn unsubscribe(&self) {
        let prev = self.subscribers.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
        if prev == 0 {
            // 防御：fetch_sub 之前已是 0 → 还原，避免 underflow 成 u32::MAX
            self.subscribers.store(0, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// 是否有订阅者（采样循环每 tick 检查）。
    fn has_subscribers(&self) -> bool {
        self.subscribers.load(std::sync::atomic::Ordering::Relaxed) > 0
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
                real_bytes: read_self_phys_footprint(),
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
            ring.push(SamplePoint { timestamp: now, rss: process.rss_bytes, real: process.real_bytes, cpu: process.cpu_percent });
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
        // probe 闭包：Before 存 RSS，After 算差 record_once。
        // key 含 ThreadId：多线程并发加载同一未缓存模型（如 server 多连接同时 cache miss
        // 同一 ASR 引擎）时，Before/After 按 (线程, 模型) 配对，避免互相覆盖/错拿导致
        // 估算值失真。仅影响状态页「估算内存」显示，不影响模型加载正确性。
        let before_map: Arc<Mutex<HashMap<(std::thread::ThreadId, String), u64>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let registry = self.registry.clone();
        let bm = before_map.clone();
        octopus_infra::model_probe::set_probe(Arc::new(move |phase, id| {
            use octopus_infra::model_probe::LoadPhase;
            match phase {
                LoadPhase::Before => {
                    if let Some(m) = read_self_probe_memory() {
                        // 覆盖式 insert：同线程重试加载刷新 before；若 After 未触发（如 panic），
                        // 条目残留但不增长（线程×模型 组合有界）。
                        bm.lock()
                            .insert((std::thread::current().id(), id.to_string()), m);
                    }
                }
                LoadPhase::After => {
                    let key = (std::thread::current().id(), id.to_string());
                    let before = bm.lock().remove(&key);
                    // reload 场景：estimated 已有首次值（unload 保留了它）→ 直接复用，
                    // 不算 RSS 差（ort arena 复用会让差值~0 偏低）。首次加载才算差。
                    // 这同时修复 OCR idle 释放后重载：旧实现 run_ocr 重载不调 probe，
                    // 而 Unload 已 remove → 重载后状态页永久缺 OCR；现重载补 probe，
                    // After 走 estimated 复用首次值恢复 active。
                    if let Some(cached) = registry.estimated(id) {
                        registry.upsert_active(id, cached);
                    } else if let (Some(b), Some(now)) = (before, read_self_probe_memory()) {
                        if now > b {
                            registry.upsert_active(id, now - b);
                        } else {
                            // 模型已加载成功，但 RSS 增量测不到（now<=b：ort arena 复用 / 并发释放）。
                            // 仍登记 active（状态页显示该模型在加载），但不写 estimated（下次 reload 重算），
                            // 否则条目永久缺失 + estimated 永不写入 → reload 仍走此分支永不显示。
                            registry.mark_active_unmeasured(id);
                        }
                    }
                }
                LoadPhase::Unload => {
                    // 模型从内存卸载（OCR idle 释放 / ASR 缓存淘汰）：仅移除 active 列表
                    // 条目（状态页不再显示），保留 estimated 首次值供下次 reload 复用。
                    registry.remove(id);
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
                // 仅在有订阅者时采样——SystemPanel 关闭时无意义刷新会持续分配
                // sysinfo Process/HashMap 节点 + 全局广播 emit（闲置 alloc 源）。
                if this.has_subscribers() {
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        this.sample_and_emit(&app2);
                    }));
                }
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

/// SystemPanel mount 时调用——开启后台采样循环（订阅 +1）。
/// 2026-07-17 性能优化：闲置时停止无意义 sysinfo 刷新。
#[tauri::command]
pub fn subscribe_system_status(sampler: State<'_, Arc<SystemStatusSampler>>) {
    sampler.subscribe();
}

/// SystemPanel unmount 时调用——订阅 -1，归零则后台循环空转。
#[tauri::command]
pub fn unsubscribe_system_status(sampler: State<'_, Arc<SystemStatusSampler>>) {
    sampler.unsubscribe();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_active_records_and_entries() {
        let r = ModelMemoryRegistry::new();
        r.upsert_active("asr:paraformer", 380_000_000);
        let e = r.entries();
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].id, "asr:paraformer");
        assert_eq!(e[0].kind, "asr");
        assert_eq!(e[0].display_name, "paraformer");
        assert_eq!(e[0].estimated_bytes, Some(380_000_000));
    }

    #[test]
    fn estimated_keeps_first_value_against_low_overwrite() {
        // arena 复用让 reload 时算出的差偏低（50M < 首次 210M）。estimated 仅首次
        // 写入（or_insert），后续偏低值不覆盖——probe 闭包据此在 reload 时复用
        // 首次值，避免状态页显示错误的近零估算。
        let r = ModelMemoryRegistry::new();
        r.upsert_active("ocr:PP-OCRv4", 210_000_000);
        r.upsert_active("ocr:PP-OCRv4", 50_000_000); // 偏低值不应污染 estimated
        assert_eq!(
            r.estimated("ocr:PP-OCRv4"),
            Some(210_000_000),
            "estimated 首次值不被偏低值覆盖"
        );
    }

    #[test]
    fn estimated_persists_across_unload_and_reload_restores_active() {
        // 首次加载 → idle 释放（unload）→ reload：estimated 保留首次值，
        // reload 后 active 用首次值恢复，状态页仍显首次估算（修复 OCR 重载不 probe
        // 致永久缺条目 + ASR 重载 arena 复用致偏低）。
        let r = ModelMemoryRegistry::new();
        r.upsert_active("ocr:PP-OCRv4", 210_000_000);
        r.remove("ocr:PP-OCRv4"); // unload：active 清、estimated 保留
        assert!(r.entries().is_empty(), "unload 后 active 应空");
        assert_eq!(r.estimated("ocr:PP-OCRv4"), Some(210_000_000));
        // reload：probe 闭包取 estimated 缓存值恢复 active
        r.upsert_active("ocr:PP-OCRv4", r.estimated("ocr:PP-OCRv4").unwrap());
        assert_eq!(
            r.entries()[0].estimated_bytes,
            Some(210_000_000),
            "reload 后 active 恢复首次估算"
        );
    }

    #[test]
    fn mark_active_unmeasured_inserts_active_without_estimated() {
        // After 时 now<=b（ort arena 复用 / 并发释放）测不到增量：仍登记 active 占位
        // （状态页显示该模型在加载），但不写 estimated——避免不可信近零值持久化为首次估算，
        // 下次 reload 仍走 estimated miss 重算。若不登记则条目永久缺失 + estimated 永不写。
        let r = ModelMemoryRegistry::new();
        r.mark_active_unmeasured("asr:zipformer");
        let e = r.entries().into_iter().find(|m| m.id == "asr:zipformer").unwrap();
        assert_eq!(e.estimated_bytes, Some(0), "active 占位 0（已加载、增量未测）");
        assert!(
            r.estimated("asr:zipformer").is_none(),
            "不可信近零值不应持久化为首次估算，下次 reload 重算"
        );
    }

    #[test]
    fn entries_sorted_by_id() {
        let r = ModelMemoryRegistry::new();
        r.upsert_active("vad:silero", 30_000_000);
        r.upsert_active("asr:paraformer", 380_000_000);
        let ids: Vec<_> = r.entries().into_iter().map(|m| m.id).collect();
        assert_eq!(ids, vec!["asr:paraformer", "vad:silero"]);
    }

    #[test]
    fn ring_buffer_evicts_oldest_when_full() {
        let mut rb = RingBuffer::new(3);
        for i in 0..5 {
            rb.push(SamplePoint { timestamp: i as f64, rss: i, real: None, cpu: i as f32 });
        }
        let ts = rb.to_time_series();
        // 容量 3：只保留最后 3 个（i=2,3,4）
        assert_eq!(ts.rss, vec![2, 3, 4]);
        assert_eq!(ts.timestamps.len(), 3);
        assert_eq!(ts.real.len(), 3);
    }

    #[test]
    fn ring_buffer_under_capacity_keeps_all() {
        let mut rb = RingBuffer::new(60);
        rb.push(SamplePoint { timestamp: 1.0, rss: 100, real: None, cpu: 5.0 });
        assert_eq!(rb.to_time_series().rss, vec![100]);
    }

    #[test]
    fn ring_buffer_zero_cap_clamps_to_one() {
        // cap=0 经 new 内 cap.max(1) 钳到 1，只留最新一个点。
        let mut rb = RingBuffer::new(0);
        rb.push(SamplePoint { timestamp: 1.0, rss: 1, real: None, cpu: 1.0 });
        rb.push(SamplePoint { timestamp: 2.0, rss: 2, real: None, cpu: 2.0 });
        assert_eq!(rb.to_time_series().rss, vec![2], "cap=0 钳到 1，只留最新");
    }

    #[test]
    fn ring_buffer_propagates_real_to_time_series() {
        // real 字段（phys_footprint）应随 SamplePoint 推入 TimeSeries.real。
        let mut rb = RingBuffer::new(3);
        rb.push(SamplePoint { timestamp: 1.0, rss: 100, real: Some(80), cpu: 1.0 });
        rb.push(SamplePoint { timestamp: 2.0, rss: 200, real: None, cpu: 2.0 });
        let ts = rb.to_time_series();
        assert_eq!(ts.real, vec![Some(80), None]);
    }

    #[test]
    fn registry_remove_clears_active_keeps_estimated() {
        let r = ModelMemoryRegistry::new();
        r.upsert_active("asr:x", 100);
        assert_eq!(r.entries().len(), 1, "upsert 后 active 应有 1 条");
        r.remove("asr:x");
        assert!(r.entries().is_empty(), "remove 后 active 应清空");
        assert_eq!(r.estimated("asr:x"), Some(100), "estimated 保留供 reload 复用");
    }

    #[test]
    fn registry_remove_nonexistent_no_panic() {
        let r = ModelMemoryRegistry::new();
        r.remove("nope"); // 不存在，应 no-op 不 panic
        assert!(r.entries().is_empty());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn read_self_phys_footprint_returns_some_positive() {
        // 自身进程必有 phys_footprint（即使空跑也占若干 MB）。
        assert!(
            read_self_phys_footprint().map(|v| v > 0).unwrap_or(false),
            "macOS 自身进程 phys_footprint 应为 Some(>0)"
        );
    }
}
