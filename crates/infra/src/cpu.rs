//! CPU 使用率检测（sysinfo 封装）。
//!
//! sysinfo 的 `global_cpu_usage()` 基于「两次 refresh_cpu_usage 的时间差分」计算，
//! 单次刷新无基准恒返回 0。故 System 实例跨调用持久化（`OnceLock<Mutex<System>>`），
//! 首次调用初始化 + 预热 0 基线，后续每次 lock + refresh + 读取差分。
//!
//! 与 desktop 的 SystemStatusSampler 各自持独立 System 实例——sampler 还需要
//! refresh_processes/memory 等更多 API，不合并到此处。

use std::sync::OnceLock;

use parking_lot::Mutex;
use sysinfo::System;

static SYS: OnceLock<Mutex<System>> = OnceLock::new();

/// 取持久化 System 实例（首次调用初始化 + 预热 CPU 基线）。
fn sys() -> &'static Mutex<System> {
    SYS.get_or_init(|| {
        let mut sys = System::new();
        // 预热基线：建立首次刷新时间戳，后续差分才有意义（否则恒返回 0）。
        sys.refresh_cpu_usage();
        Mutex::new(sys)
    })
}

/// 全局 CPU 使用率（0.0–100.0）。
///
/// 首次调用返回 0（刚预热基线无差分）；第二次起返回准确值。
/// 每次调用内部 refresh_cpu_usage 刷新差分。
pub fn global_cpu_usage() -> f32 {
    let mut guard = sys().lock();
    guard.refresh_cpu_usage();
    guard.global_cpu_usage()
}

/// CPU 是否空闲（使用率 < threshold）。
///
/// 典型用法：`is_cpu_idle(30.0)` — CPU < 30% 视为空闲。
/// 后台调度器（octopus-scheduler）在每轮 tick 调此判断，忙则跳过任务。
pub fn is_cpu_idle(threshold: f32) -> bool {
    global_cpu_usage() < threshold
}
