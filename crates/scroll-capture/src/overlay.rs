/// 显示器信息（逻辑坐标 + scale factor）。
pub struct MonitorInfo {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub scale_factor: f64,
}

/// 跨平台滚动截屏覆盖窗口抽象。
/// macOS 一期实现，Windows/Linux 二期替换。
pub trait ScrollOverlay: Send {
    /// 为每个显示器创建全屏透明覆盖窗口。
    fn create(monitors: &[MonitorInfo]) -> Self;
    /// 设置滚轮穿透（true = 滚轮穿透到底层应用）。
    fn set_scroll_through(&self, enabled: bool);
    /// 获取所有覆盖窗口的 ID（用于 CGWindowList 排除）。
    fn window_ids(&self) -> Vec<u64>;
    /// 销毁所有覆盖窗口。
    fn destroy(self);
}
