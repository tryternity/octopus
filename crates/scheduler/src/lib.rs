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
/// let mut scheduler = octopus_scheduler::Scheduler::new();
/// scheduler.register_task("clipboard_cleanup", 600, Box::new(|| {
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
        self.register_task_inner(name, interval_secs, run, false);
    }

    /// 注册一个**不受 CPU 空闲检查**的轻量定时任务。
    /// 用于几十 ms 级的轻量任务（如 bigram 索引刷新），不需要等 CPU 空闲。
    pub fn register_task_skip_idle(
        &mut self,
        name: &str,
        interval_secs: u64,
        run: Box<dyn Fn() + Send + 'static>,
    ) {
        self.register_task_inner(name, interval_secs, run, true);
    }

    fn register_task_inner(
        &mut self,
        name: &str,
        interval_secs: u64,
        run: Box<dyn Fn() + Send + 'static>,
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
        // tasks move 进线程（spawn 消费 self），不需要 Arc 共享。
        // 闭包是 Send + 'static，move 进 thread::spawn 合法。
        let tasks = self.tasks;

        std::thread::Builder::new()
            .name("octopus-scheduler".to_string())
            .spawn(move || {
                loop {
                    std::thread::sleep(Duration::from_secs(tick));

                    // 轻量任务（skip_idle_check=true）不受 CPU 空闲检查，每 tick 到期即跑
                    run_due_tasks(&tasks, true, "轻量任务");

                    // 重任务（skip_idle_check=false）需 CPU 空闲才跑
                    if !octopus_infra::cpu::is_cpu_idle(threshold) {
                        log::debug!("[scheduler] CPU 忙，跳过重任务（{} 个）", tasks.len());
                        continue;
                    }

                    run_due_tasks(&tasks, false, "任务");
                }
            })
            .expect("failed to spawn scheduler thread");
    }
}

/// 跑到期任务——`skip_idle_filter` true=只跑轻量任务（skip_idle_check=true），
/// false=只跑重任务（skip_idle_check=false）。`label` 用于日志（"轻量任务"/"任务"）。
///
/// catch_unwind 防一个任务 panic 杀死整个调度线程（闭包非 UnwindSafe，需 AssertUnwindSafe 包装）。
/// panic 也 mark_run：否则 is_due() 下个 tick 仍 true，deterministic panic 每 tick 重跑
/// （默认 interval 600s 给问题任务冷却期）。
///
/// 2026-08-05 抽取：消除 spawn 中轻量/重任务两段逐字重复的 catch_unwind + match 块。
fn run_due_tasks(tasks: &[ScheduledTask], skip_idle_filter: bool, label: &str) {
    for task in tasks {
        // skip_idle_filter 决定本次跑轻量（task.skip_idle_check==true）还是重（==false）任务
        if task.skip_idle_check != skip_idle_filter {
            continue;
        }
        if !task.is_due() {
            continue;
        }
        log::debug!("[scheduler] 执行{}: {}", label, task.name);
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (task.run)())) {
            Ok(_) => task.mark_run(),
            Err(_) => {
                log::error!(
                    "[scheduler] 任务 {} panic，已吞，标记 last_run 避免每 tick 重试，继续调度",
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
            run: Box::new(move || {
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
            run: Box::new(|| panic!("test panic")),
            skip_idle_check,
        }
    }

    /// filter 分流：skip_idle_filter=true 只跑轻量任务（skip_idle_check=true），重任务不跑。
    #[test]
    fn run_due_tasks_light_filter_skips_heavy() {
        let light_counter = Arc::new(AtomicU32::new(0));
        let heavy_counter = Arc::new(AtomicU32::new(0));
        let tasks = vec![
            make_task("light", true, light_counter.clone()),
            make_task("heavy", false, heavy_counter.clone()),
        ];
        run_due_tasks(&tasks, true, "轻量任务");
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
        run_due_tasks(&tasks, false, "任务");
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
        run_due_tasks(&tasks, false, "任务");
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
        run_due_tasks(&tasks, false, "任务");
        assert_eq!(counter.load(Ordering::SeqCst), 1, "首次执行");
        // mark_run 后 interval_secs=600 内不再到期
        run_due_tasks(&tasks, false, "任务");
        assert_eq!(counter.load(Ordering::SeqCst), 1, "mark_run 后不重跑");
    }
}
