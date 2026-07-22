//! octopus-scheduler：通用后台调度器。
//!
//! 后台线程每 `tick_interval_secs`（默认 600 秒 = 10 分钟）醒一次，检查 CPU 是否空闲
//! （调 `infra::cpu::is_cpu_idle`），空闲则执行所有到期任务。CPU 忙则跳过本轮，等下次。
//!
//! scheduler 不知道任务的具体业务逻辑（删什么、同步什么）——由调用方通过 `run` 闭包注入。
//! scheduler 只管「到点 + CPU 空闲 → 跑闭包」。

use std::time::{Duration, Instant};

use parking_lot::Mutex;

/// 默认检查间隔（秒）。每 10 分钟醒一次。
const DEFAULT_TICK_INTERVAL_SECS: u64 = 600;

/// 默认 CPU 空闲阈值。CPU 使用率 < 30% 视为空闲。
const DEFAULT_IDLE_THRESHOLD: f32 = 30.0;

/// 单个注册的定时任务。
struct ScheduledTask {
    name: String,
    interval_secs: u64,
    last_run: Mutex<Option<Instant>>,
    run: Box<dyn Fn() + Send + 'static>,
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
/// let mut scheduler = octopus_scheduler::Scheduler::new();
/// scheduler.register_task("trash_purge", 600, Box::new(|| {
///     // 业务逻辑（scheduler 不知道这里做什么）
/// }));
/// scheduler.spawn(); // 后台线程，永不退出
/// ```
pub struct Scheduler {
    tasks: Vec<ScheduledTask>,
    tick_interval_secs: u64,
    idle_threshold: f32,
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
        }
    }

    /// 注册一个定时任务。
    ///
    /// - `name`：任务名（仅日志用）
    /// - `interval_secs`：执行间隔（秒）。scheduler 每 tick 醒一次检查是否到期，
    ///   到期 + CPU 空闲才执行。建议 interval >= tick_interval。
    /// - `run`：执行闭包。scheduler 不关心闭包做什么。
    pub fn register_task(
        &mut self,
        name: &str,
        interval_secs: u64,
        run: Box<dyn Fn() + Send + 'static>,
    ) {
        self.tasks.push(ScheduledTask {
            name: name.to_string(),
            interval_secs,
            last_run: Mutex::new(None),
            run,
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
        // tasks move 进线程（spawn 消费 self），不需要 Arc 共享。
        // 闭包是 Send + 'static，move 进 thread::spawn 合法。
        let tasks = self.tasks;

        std::thread::Builder::new()
            .name("octopus-scheduler".to_string())
            .spawn(move || {
                loop {
                    std::thread::sleep(Duration::from_secs(tick));

                    // CPU 空闲检测——忙则跳过本轮所有任务
                    if !octopus_infra::cpu::is_cpu_idle(threshold) {
                        log::debug!("[scheduler] CPU 忙，跳过本轮（{} 个任务）", tasks.len());
                        continue;
                    }

                    for task in tasks.iter() {
                        if task.is_due() {
                            log::debug!("[scheduler] 执行任务: {}", task.name);
                            (task.run)();
                            task.mark_run();
                        }
                    }
                }
            })
            .expect("failed to spawn scheduler thread");
    }
}
