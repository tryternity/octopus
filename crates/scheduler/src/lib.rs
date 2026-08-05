//! octopus-scheduler：通用后台调度器。
//!
//! 后台线程每 `tick_interval_secs`（默认 600 秒 = 10 分钟）醒一次，检查 CPU 是否空闲
//! （调 `infra::cpu::is_cpu_idle`），空闲则执行所有到期任务。CPU 忙则跳过本轮，等下次。
//!
//! scheduler 不知道任务的具体业务逻辑（删什么、同步什么）——由调用方通过 `run` 闭包注入。
//! scheduler 只管「到点 + CPU 空闲 → 跑闭包」。

use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

/// 默认检查间隔（秒）。每 10 分钟醒一次。
const DEFAULT_TICK_INTERVAL_SECS: u64 = 600;

/// 默认 CPU 空闲阈值。CPU 使用率 < 30% 视为空闲。
const DEFAULT_IDLE_THRESHOLD: f32 = 30.0;

/// 默认单任务超时（秒）。超时后 log error + mark_run + 继续下一任务（孤儿线程自然结束）。
///
/// 第二十三轮 A1（P2-srv1）：原 run_due_tasks 串行调 `(task.run)()`，hang 在 IO/锁/死循环
/// 时后续所有任务永久饿死。现每任务 spawn 独立线程 + recv_timeout。300s 覆盖最慢任务
/// （vault sync 10-30s、clipboard cleanup <5s、hotword GC <10s），hang 任务不会无限阻塞。
const DEFAULT_TASK_TIMEOUT_SECS: u64 = 300;

/// 单个注册的定时任务。
struct ScheduledTask {
    name: String,
    interval_secs: u64,
    last_run: Mutex<Option<Instant>>,
    /// 第二十三轮 A1：Box→Arc<dyn Fn() + Send + Sync>——run_due_tasks 每次执行 spawn
    /// 独立线程（超时兜底），Arc clone 进线程。Sync bound 因 Arc 跨线程共享所需
    /// （实际只有一个线程调 run()，但 Arc<dyn Fn()> 要求 Sync）。
    run: Arc<dyn Fn() + Send + Sync + 'static>,
    /// true=不受 CPU 空闲检查（轻量任务，几十 ms 级，不需等空闲）。
    skip_idle_check: bool,
}

impl ScheduledTask {
    /// 任务是否到期（距上次执行已过 interval_secs）。
    /// 从未执行过的任务视为立即到期。
    fn is_due(&self) -> bool {
        let last = self.last_run.lock();
        match *last {
            None => true,
            Some(t) => t.elapsed() >= Duration::from_secs(self.interval_secs),
        }
    }

    /// 标记为刚执行过。
    fn mark_run(&self) {
        *self.last_run.lock() = Some(Instant::now());
    }
}

/// 通用调度器。
///
/// 用法：
/// ```no_run
/// use std::sync::Arc;
/// let mut scheduler = octopus_scheduler::Scheduler::new();
/// scheduler.register_task("clipboard_cleanup", 600, Arc::new(|| {
///     // 业务逻辑（scheduler 不知道这里做什么）
/// }));
/// scheduler.spawn(); // 后台线程，永不退出
/// ```
pub struct Scheduler {
    tasks: Vec<ScheduledTask>,
    tick_interval_secs: u64,
    idle_threshold: f32,
    task_timeout: Duration,
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl Scheduler {
    /// 创建调度器，默认 10 分钟 tick + 30% CPU 空闲阈值。
    pub fn new() -> Self {
        Self {
            tasks: Vec::new(),
            tick_interval_secs: DEFAULT_TICK_INTERVAL_SECS,
            idle_threshold: DEFAULT_IDLE_THRESHOLD,
            task_timeout: Duration::from_secs(DEFAULT_TASK_TIMEOUT_SECS),
        }
    }

    /// 注册一个定时任务。
    ///
    /// - `name`：任务名（仅日志用）
    /// - `interval_secs`：执行间隔（秒）。scheduler 每 tick 醒一次检查是否到期，
    ///   到期 + CPU 空闲才执行。建议 interval >= tick_interval。
    /// - `run`：执行闭包。scheduler 不关心闭包做什么。
    ///
    /// 第二十三轮 A1：run 类型 Box→Arc（run_due_tasks spawn 独立线程时 clone Arc）。
    /// Sync bound 因 Arc<dyn Fn()> 跨线程共享所需。调用方传 `Arc::new(|| {...})`。
    pub fn register_task(
        &mut self,
        name: &str,
        interval_secs: u64,
        run: Arc<dyn Fn() + Send + Sync + 'static>,
    ) {
        self.register_task_inner(name, interval_secs, run, false);
    }

    /// 注册一个**不受 CPU 空闲检查**的轻量定时任务。
    /// 用于几十 ms 级的轻量任务（如 bigram 索引刷新），不需要等 CPU 空闲。
    pub fn register_task_skip_idle(
        &mut self,
        name: &str,
        interval_secs: u64,
        run: Arc<dyn Fn() + Send + Sync + 'static>,
    ) {
        self.register_task_inner(name, interval_secs, run, true);
    }

    fn register_task_inner(
        &mut self,
        name: &str,
        interval_secs: u64,
        run: Arc<dyn Fn() + Send + Sync + 'static>,
        skip_idle_check: bool,
    ) {
        self.tasks.push(ScheduledTask {
            name: name.to_string(),
            interval_secs,
            last_run: Mutex::new(None),
            run,
            skip_idle_check,
        });
    }

    /// 启动后台调度线程（spawn 后 self 消费，线程永不退出）。
    ///
    /// 线程逻辑：
    /// ```text
    /// loop {
    ///     sleep(tick_interval)
    ///     if !is_cpu_idle(threshold) { continue }  // CPU 忙则跳过
    ///     for task in tasks { if task.is_due() { (task.run)(); task.mark_run() } }
    /// }
    /// ```
    pub fn spawn(self) {
        let tick = self.tick_interval_secs;
        let threshold = self.idle_threshold;
        let task_timeout = self.task_timeout;
        // tasks move 进线程（spawn 消费 self），不需要 Arc 共享。
        // 闭包是 Send + 'static，move 进 thread::spawn 合法。
        let tasks = self.tasks;

        std::thread::Builder::new()
            .name("octopus-scheduler".to_string())
            .spawn(move || {
                loop {
                    std::thread::sleep(Duration::from_secs(tick));

                    // 轻量任务（skip_idle_check=true）不受 CPU 空闲检查，每 tick 到期即跑
                    run_due_tasks(&tasks, true, "轻量任务", task_timeout);

                    // 重任务（skip_idle_check=false）需 CPU 空闲才跑
                    if !octopus_infra::cpu::is_cpu_idle(threshold) {
                        log::debug!("[scheduler] CPU 忙，跳过重任务（{} 个）", tasks.len());
                        continue;
                    }

                    run_due_tasks(&tasks, false, "任务", task_timeout);
                }
            })
            .expect("failed to spawn scheduler thread");
    }
}

/// 跑到期任务——`skip_idle_filter` true=只跑轻量任务（skip_idle_check=true），
/// false=只跑重任务（skip_idle_check=false）。`label` 用于日志（"轻量任务"/"任务"）。
///
/// **超时兜底**（第二十三轮 A1 / P2-srv1）：每任务 spawn 独立线程 + `recv_timeout`。
/// 原串行 `(task.run)()` 若 hang 在 IO/锁/死循环 → 后续所有任务永久饿死。现超时后
/// log error + mark_run + 继续下一任务。孤儿线程无法强制 cancel（Rust 无 cancel token），
/// 让其自然结束（绝大多数 hang 是 IO 等待，资源释放后自愈）。
///
/// catch_unwind 防任务 panic 杀死 worker 线程；panic 也 mark_run（否则 deterministic panic
/// 每 tick 重跑，默认 interval 600s 给问题任务冷却期）。
fn run_due_tasks(
    tasks: &[ScheduledTask],
    skip_idle_filter: bool,
    label: &str,
    task_timeout: Duration,
) {
    use std::sync::mpsc;

    for task in tasks {
        // skip_idle_filter 决定本次跑轻量（task.skip_idle_check==true）还是重（==false）任务
        if task.skip_idle_check != skip_idle_filter {
            continue;
        }
        if !task.is_due() {
            continue;
        }
        log::debug!("[scheduler] 执行{}: {}", label, task.name);

        // task.run 是 Arc<dyn Fn() + Send + Sync>——clone 进 spawn 线程（Arc 计数 +1），
        // 超时后调度线程释放自己的 Arc，孤儿线程持自己的 Arc 继续跑到自然结束。
        let (tx, rx) = mpsc::channel::<Result<(), &'static str>>();
        let run = task.run.clone();
        let task_name = task.name.clone();
        std::thread::Builder::new()
            .name(format!("octopus-task-{}", task_name))
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run()));
                let _ = tx.send(result.map_err(|_| "panic"));
            })
            .expect("[scheduler] failed to spawn task thread");

        match rx.recv_timeout(task_timeout) {
            Ok(Ok(_)) => task.mark_run(),
            Ok(Err(_)) => {
                log::error!(
                    "[scheduler] 任务 {} panic，已吞，标记 last_run 避免每 tick 重试，继续调度",
                    task.name
                );
                task.mark_run();
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                log::error!(
                    "[scheduler] 任务 {} 超时（{}s），标记 last_run 继续下一任务（孤儿线程自然结束）",
                    task.name,
                    task_timeout.as_secs()
                );
                task.mark_run();
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                // worker 线程结束但没 send（不应发生——catch_unwind 总会返回）
                log::error!(
                    "[scheduler] 任务 {} worker 线程异常断开，标记 last_run 继续",
                    task.name
                );
                task.mark_run();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    /// 构造一个任务，run 闭包递增 counter。never_run=true 保持 last_run=None（is_due 返 true）。
    fn make_task(name: &str, skip_idle_check: bool, counter: Arc<AtomicU32>) -> ScheduledTask {
        let c = counter.clone();
        ScheduledTask {
            name: name.to_string(),
            interval_secs: 600,
            last_run: Mutex::new(None), // None → is_due() 返 true（从未执行过）
            run: Arc::new(move || {
                c.fetch_add(1, Ordering::SeqCst);
            }),
            skip_idle_check,
        }
    }

    /// 构造一个 panic 任务（验证 catch_unwind 吞 panic 不杀调用方）。
    fn make_panic_task(name: &str, skip_idle_check: bool) -> ScheduledTask {
        ScheduledTask {
            name: name.to_string(),
            interval_secs: 600,
            last_run: Mutex::new(None),
            run: Arc::new(|| panic!("test panic")),
            skip_idle_check,
        }
    }

    /// 第二十三轮 A1 回归：构造一个 hang 任务（sleep 超过 timeout），验证超时兜底。
    fn make_hang_task(name: &str, skip_idle_check: bool, hang_secs: u64) -> ScheduledTask {
        ScheduledTask {
            name: name.to_string(),
            interval_secs: 600,
            last_run: Mutex::new(None),
            run: Arc::new(move || {
                std::thread::sleep(Duration::from_secs(hang_secs));
            }),
            skip_idle_check,
        }
    }

    /// 测试用短超时（避免 hang 测试真的等 300s）。
    const TEST_TIMEOUT: Duration = Duration::from_millis(200);

    /// filter 分流：skip_idle_filter=true 只跑轻量任务（skip_idle_check=true），重任务不跑。
    #[test]
    fn run_due_tasks_light_filter_skips_heavy() {
        let light_counter = Arc::new(AtomicU32::new(0));
        let heavy_counter = Arc::new(AtomicU32::new(0));
        let tasks = vec![
            make_task("light", true, light_counter.clone()),
            make_task("heavy", false, heavy_counter.clone()),
        ];
        run_due_tasks(&tasks, true, "轻量任务", TEST_TIMEOUT);
        assert_eq!(light_counter.load(Ordering::SeqCst), 1, "轻量任务应执行");
        assert_eq!(heavy_counter.load(Ordering::SeqCst), 0, "重任务不应执行");
    }

    /// filter 分流：skip_idle_filter=false 只跑重任务，轻量任务不跑。
    #[test]
    fn run_due_tasks_heavy_filter_skips_light() {
        let light_counter = Arc::new(AtomicU32::new(0));
        let heavy_counter = Arc::new(AtomicU32::new(0));
        let tasks = vec![
            make_task("light", true, light_counter.clone()),
            make_task("heavy", false, heavy_counter.clone()),
        ];
        run_due_tasks(&tasks, false, "任务", TEST_TIMEOUT);
        assert_eq!(light_counter.load(Ordering::SeqCst), 0, "轻量任务不应执行");
        assert_eq!(heavy_counter.load(Ordering::SeqCst), 1, "重任务应执行");
    }

    /// panic 吞掉不传播：panic 任务后 mark_run（is_due 变 false），不影响同批其他任务。
    #[test]
    fn run_due_tasks_swallows_panic_and_marks_run() {
        let ok_counter = Arc::new(AtomicU32::new(0));
        let tasks = vec![
            make_panic_task("panic", false),
            make_task("ok", false, ok_counter.clone()),
        ];
        // 不应 panic（catch_unwind 吞掉）
        run_due_tasks(&tasks, false, "任务", TEST_TIMEOUT);
        // panic 任务也应被 mark_run（last_run 变 Some → is_due 变 false）
        assert!(!tasks[0].is_due(), "panic 任务应被 mark_run，is_due 变 false");
        // 后续任务正常执行
        assert_eq!(ok_counter.load(Ordering::SeqCst), 1, "panic 后的任务应正常执行");
    }

    /// 到期判定：mark_run 后 is_due 变 false，再次 run_due_tasks 不重跑。
    #[test]
    fn run_due_tasks_respects_mark_run() {
        let counter = Arc::new(AtomicU32::new(0));
        let tasks = vec![make_task("task", false, counter.clone())];
        run_due_tasks(&tasks, false, "任务", TEST_TIMEOUT);
        assert_eq!(counter.load(Ordering::SeqCst), 1, "首次执行");
        // mark_run 后 interval_secs=600 内不再到期
        run_due_tasks(&tasks, false, "任务", TEST_TIMEOUT);
        assert_eq!(counter.load(Ordering::SeqCst), 1, "mark_run 后不重跑");
    }

    /// 第二十三轮 A1（P2-srv1）回归：hang 任务超时后不阻塞后续任务。
    ///
    /// hang 任务（sleep 10s，远超 TEST_TIMEOUT=200ms）+ 正常任务，run_due_tasks 应：
    /// ① hang 任务超时 mark_run（不阻塞）；② 正常任务仍执行（counter 递增）。
    /// 原 bug：串行 (task.run)() 会无限阻塞，后续任务永不到达。
    #[test]
    fn run_due_tasks_timeout_unblocks_subsequent_tasks() {
        let ok_counter = Arc::new(AtomicU32::new(0));
        let tasks = vec![
            make_hang_task("hang", false, 10), // sleep 10s >> TEST_TIMEOUT
            make_task("ok", false, ok_counter.clone()),
        ];
        let start = Instant::now();
        run_due_tasks(&tasks, false, "任务", TEST_TIMEOUT);
        let elapsed = start.elapsed();
        // 应在 ~TEST_TIMEOUT（200ms）+ 正常任务执行时间内返回，远小于 hang 的 10s
        assert!(elapsed < Duration::from_secs(2), "hang 任务超时不应阻塞，elapsed={:?}", elapsed);
        // hang 任务被 mark_run（超时也算执行过）
        assert!(!tasks[0].is_due(), "hang 任务超时后应 mark_run");
        // 后续正常任务执行了
        assert_eq!(ok_counter.load(Ordering::SeqCst), 1, "hang 超时后后续任务应正常执行");
    }
}
