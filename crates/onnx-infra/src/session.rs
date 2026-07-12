use anyhow::{Context, Result};
use octopus_infra::config::load_config;

/// ONNX session 硬件加速。读取 `asr_hardware_accelerated` 配置，按平台注册 EP。
///
/// - `skip_coreml`：某些模型含 CoreML 不支持的动态算子（如 qwen3-asr），传 true 跳过 EP
/// - 加速注册失败时 fallback 到纯 CPU
pub fn apply_session_acceleration(
    builder: ort::session::builder::SessionBuilder,
    skip_coreml: bool,
) -> Result<ort::session::builder::SessionBuilder> {
    let app_cfg = load_config().unwrap_or_default();

    if !app_cfg.asr_hardware_accelerated {
        return Ok(builder);
    }

    if skip_coreml {
        log::info!(
            "Skipping hardware EP: caller requested skip_coreml (dynamic ops incompatible). Using CPU."
        );
        return Ok(builder);
    }

    let providers = vec![
        #[cfg(target_os = "macos")]
        ort::ep::CoreMLExecutionProvider::default().build(),
        #[cfg(target_os = "linux")]
        ort::ep::CUDAExecutionProvider::default().build(),
        #[cfg(target_os = "windows")]
        ort::ep::DirectMLExecutionProvider::default().build(),
    ];

    log::info!(
        "Attempting to build session with hardware acceleration EPs on {} ({} provider(s))",
        std::env::consts::OS,
        providers.len()
    );
    match builder.with_execution_providers(providers) {
        Ok(b) => {
            log::info!("Successfully registered EPs!");
            Ok(b)
        }
        Err(e) => {
            log::warn!("Failed to register hardware acceleration EPs: {:?}. Falling back to CPU.", e);
            ort::session::Session::builder().context("Failed to reconstruct fallback session builder")
        }
    }
}
