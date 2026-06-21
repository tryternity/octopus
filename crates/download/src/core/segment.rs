//! 分段规划：把 [0, total) 切成 N 段。单段 = 单流退化。

/// 一段下载区间 [begin, end]（含端点，bytes）。downloaded 为已下字节（续传用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Segment {
    pub begin: u64,
    pub end: u64,
    pub downloaded: u64,
}

impl Segment {
    pub fn len(&self) -> u64 {
        self.end.saturating_sub(self.begin) + 1
    }
    pub fn is_done(&self) -> bool {
        self.downloaded >= self.len()
    }
    /// 下一个要请求的字节偏移（begin + downloaded）。
    pub fn next_offset(&self) -> u64 {
        self.begin + self.downloaded
    }
}

/// 规划分段。
/// - `accept_ranges=false` 或 `total=None` 或 `total < threshold` → 1 段（单流）。
/// - 否则按 `segment_size` 切，段数上限 `max_concurrent`。
pub fn plan_segments(total: u64, accept_ranges: bool, segment_size: u64, threshold: u64, max_concurrent: usize) -> Vec<Segment> {
    let one = || vec![Segment { begin: 0, end: total.saturating_sub(1), downloaded: 0 }];
    let Some(total) = (total != 0).then_some(total) else { return one() };
    if !accept_ranges || total < threshold || segment_size == 0 || max_concurrent == 0 {
        return one();
    }
    let count_by_size = ((total + segment_size - 1) / segment_size) as usize;
    let n = count_by_size.min(max_concurrent).max(1);
    let base = total / n as u64;
    let mut segs = Vec::with_capacity(n);
    let mut start = 0u64;
    for i in 0..n {
        // 余数逐段 +1 均摊到前若干段
        let extra = if i < (total % n as u64) as usize { 1 } else { 0 };
        let size = base + extra;
        let end = start + size - 1;
        segs.push(Segment { begin: start, end, downloaded: 0 });
        start = end + 1;
    }
    // 末段兜底（防浮点/边界使 start 未到 total）
    if let Some(last) = segs.last_mut() {
        last.end = total - 1;
    }
    segs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_file_one_segment() {
        let s = plan_segments(1_000, true, 4 * 1024 * 1024, 16 * 1024 * 1024, 8);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].begin, 0);
        assert_eq!(s[0].end, 999);
    }

    #[test]
    fn no_range_one_segment() {
        let s = plan_segments(100 * 1024 * 1024, false, 4 * 1024 * 1024, 16 * 1024 * 1024, 8);
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn large_file_multi_segment_cover_full_range() {
        let total: u64 = 50 * 1024 * 1024;
        let s = plan_segments(total, true, 4 * 1024 * 1024, 16 * 1024 * 1024, 8);
        assert!(s.len() > 1, "应多段");
        // 段首 = 0，段尾 = total-1，无间隙无重叠
        assert_eq!(s.first().unwrap().begin, 0);
        assert_eq!(s.last().unwrap().end, total - 1);
        for w in s.windows(2) {
            assert_eq!(w[0].end + 1, w[1].begin, "段应连续");
        }
        // 总长 == total
        let sum: u64 = s.iter().map(|x| x.len()).sum();
        assert_eq!(sum, total);
    }

    #[test]
    fn segment_count_capped_by_max_concurrent() {
        let total: u64 = 200 * 1024 * 1024;
        let s = plan_segments(total, true, 4 * 1024 * 1024, 16 * 1024 * 1024, 4);
        assert_eq!(s.len(), 4);
    }

    #[test]
    fn segment_helpers() {
        let seg = Segment { begin: 100, end: 199, downloaded: 30 };
        assert_eq!(seg.len(), 100);
        assert!(!seg.is_done());
        assert_eq!(seg.next_offset(), 130);
    }
}
