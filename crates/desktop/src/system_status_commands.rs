//! 系统状态页后端：模型内存估算表 + 系统资源采样器 + get_system_status 命令。
//!
//! 「模型占用内存」：同进程 ort 无法 OS 级 per-model 拆分，故用「加载前后进程 RSS 差值」
//! 近似（仅首次记录不覆盖，避免 ort arena 复用导致后续差值偏低/为负）。属估算，前端标注「约」。

// 以下类型/方法在本 task 仅作为公共数据契约存在，尚未注册到 Tauri handler；
// 待 Task 4 接入 get_system_status 命令后即可移除这些 allow。
#![allow(dead_code)]

use parking_lot::Mutex;
use serde::Serialize;
use std::collections::{HashMap, VecDeque};

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
