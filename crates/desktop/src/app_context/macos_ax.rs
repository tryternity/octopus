//! macOS Accessibility 实现（Task 2 填充）。

pub struct AxProvider;

impl super::ContextProvider for AxProvider {
    fn gather(&self) -> anyhow::Result<super::ExtraContext> {
        Err(anyhow::anyhow!("not yet implemented"))
    }
}
