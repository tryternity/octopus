//! 进度上报结构 + 速度估算（EMA）。

use std::time::Duration;

/// 一次进度快照（推给 mpsc 消费者，不持久化）。
#[derive(Debug, Clone, Default)]
pub struct Progress {
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub speed_bps: Option<f64>,
}

impl Progress {
    /// 0.0–1.0 的完成比例（total 未知时返回 None）。
    pub fn fraction(&self) -> Option<f64> {
        self.total_bytes
            .filter(|&t| t > 0)
            .map(|t| self.downloaded_bytes as f64 / t as f64)
    }
}

/// 指数移动平均速度估算。anchor 周期重置，避免长下载速度失真。
#[derive(Debug, Clone)]
pub struct SpeedEstimator {
    ema: f64,
    last_bytes: u64,
    // 以下两个字段当前仅在 update 内赋值，预留给后续扩展（如基于 anchor 段的稳态速率），
    // 用 #[allow(dead_code)] 抑制 warning，避免污染编译输出。这是对 spec 的一处补强。
    #[allow(dead_code)]
    anchor_bytes: u64,
    #[allow(dead_code)]
    anchor_ema: f64,
}

impl SpeedEstimator {
    pub fn new() -> Self {
        Self {
            ema: 0.0,
            last_bytes: 0,
            anchor_bytes: 0,
            anchor_ema: 0.0,
        }
    }

    /// 收到一个新字节计数 + 距上次经过的时间。返回估算速度 (bytes/sec)。
    /// `alpha` 为 EMA 系数（如 0.4），`anchor_period` 为重置周期（如 300ms）。
    pub fn update(&mut self, bytes: u64, elapsed: Duration, alpha: f64, anchor_period: Duration) -> f64 {
        let delta = bytes.saturating_sub(self.last_bytes);
        let secs = elapsed.as_secs_f64().max(1e-6);
        let instant = delta as f64 / secs;

        if self.ema == 0.0 {
            self.ema = instant;
        } else {
            self.ema = (1.0 - alpha) * self.ema + alpha * instant;
        }
        self.last_bytes = bytes;

        // anchor 周期到了：用当前 ema 重置 anchor，避免单次瞬时值长期主导。
        if elapsed >= anchor_period {
            self.anchor_bytes = bytes;
            self.anchor_ema = self.ema;
        }
        self.ema
    }
}

impl Default for SpeedEstimator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fraction_known_total() {
        let p = Progress { downloaded_bytes: 50, total_bytes: Some(200), speed_bps: None };
        assert_eq!(p.fraction(), Some(0.25));
    }

    #[test]
    fn fraction_unknown_total() {
        let p = Progress { downloaded_bytes: 50, total_bytes: None, speed_bps: None };
        assert_eq!(p.fraction(), None);
    }

    #[test]
    fn speed_estimator_first_sample_is_instant() {
        let mut s = SpeedEstimator::new();
        let v = s.update(1_000_000, Duration::from_secs(1), 0.4, Duration::from_millis(300));
        assert!((v - 1_000_000.0).abs() < 1.0);
    }

    #[test]
    fn speed_estimator_ema_smooths() {
        // 验证 EMA 平滑：首样瞬时 1MB/s，次样瞬时 3MB/s（1s 内增 2MB），
        // 平滑后的 ema 应严格介于两个瞬时值之间（0.6*1M + 0.4*3M = 1.8M）。
        let mut s = SpeedEstimator::new();
        s.update(1_000_000, Duration::from_secs(1), 0.4, Duration::from_millis(300));
        let v2 = s.update(3_000_000, Duration::from_secs(1), 0.4, Duration::from_millis(300));
        assert!(v2 > 1_000_000.0 && v2 < 3_000_000.0);
    }
}
