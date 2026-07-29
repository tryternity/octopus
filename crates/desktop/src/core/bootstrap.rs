//! 应用启动初始化（从 main.rs::run() 提取，2026-07-29 拆分第一步）。
//!
//! panic hook → config → DB → 模型软链 → builtin 同步 → 4 域引擎预热 →
//! 搜索引擎 → 引擎模式校验 → 润色配置校验 → prompt 加载。
//! 返回 AppConfig 供 run() 的 builder/setup 使用。

use log::info;

/// 应用启动初始化。返回加载的 AppConfig。
pub(crate) fn bootstrap() -> octopus_infra::config::AppConfig {
    // panic hook：catch_unwind 已处理降级，panic 仅记 warning（不刷屏）
    std::panic::set_hook(Box::new(|info| {
        let location = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
        let payload = info.payload();
        let msg = if let Some(s) = payload.downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = payload.downcast_ref::<String>() {
            s.clone()
        } else {
            "<non-string panic payload>".to_string()
        };
        log::warn!("Recovered panic at {}: {}", location, msg);
    }));

    let config = octopus_infra::config::load_config().unwrap_or_else(|e| {
        log::warn!("config load failed ({}), using defaults", e);
        octopus_infra::config::AppConfig::default()
    });

    // 初始化嵌入式 DB（建表 + seed 默认引擎）。asr 的 load_config 首次调用时也会
    // lazy init，这里显式预热（日志早出 + 错误前置）。模型配置唯一来源即此 DB。
    // 失败仅告警，不阻断启动（识别历史写入会失败，但应用可用）
    if let Err(e) = octopus_asr_local::db::ensure_db() {
        log::error!("DB init failed: {}, storage disabled", e);
    }

    // 创建模型路径软链（HF cache → ~/.octopus/models/{source}/）
    // 必须在 sync_builtin_models_availability + preheat 之前——builtin 兜底引擎的
    // 文件可能只在 HF cache，软链建好后 resolve_model_dir 才能命中。
    if let Err(e) = crate::commands::model_migrate::create_model_symlinks() {
        log::warn!("模型路径迁移失败（非致命）: {e:?}");
    }

    // Builtin 模型 is_available 同步（必须在 create_model_symlinks 之后、preheat 之前）：
    // builtin 兜底引擎的 is_available 反映文件真实状态。ensure_builtin_seed 注入时
    // is_available=0，软链建好后文件就绪 → 置 1，否则 resolve_engine_any（要求
    // is_available=1）查不到 → ASR 报 Unknown engine。详见 spec 2026-07-22-builtin-models.md §3。
    crate::commands::builtin_models::sync_builtin_models_availability();

    // Task 2 模型激活语义重构：启动时加载 4 域激活引擎到 ACTIVE_ENGINES 内存缓存。
    // 后续所有使用方（推理 / tray / 管理页 / 流式判定）经 resolve_active_engine(domain)
    // 纯读此缓存。ASR 域带兜底（zipformer-small-ctc），其余域无激活仅告警不阻断。
    for domain in ["asr", "llm", "ocr", "translate"] {
        match octopus_asr_local::config::load_active_engine(domain) {
            Ok(r) => info!("Active {} engine: {} [{}]", domain, r.name, r.provider),
            Err(e) => log::warn!("Active {} engine 未激活：{}", domain, e),
        }
    }
    info!(
        "Config: mode={}, asr_shortcut={}",
        config.engine_mode, config.asr_shortcut
    );
    // 初始化搜索引擎（应用索引 + 书签扫描）
    octopus_search::init_search_engine();

    // 校验引擎模式
    if config.engine_mode == "embedded" && !crate::core::config::is_streaming_engine() {
        let active_name = octopus_asr_local::config::resolve_active_engine("asr")
            .map(|r| r.name)
            .unwrap_or_else(|_| "<未激活>".to_string());
        log::info!("引擎 '{}' 使用 VAD 分段伪流式模式", active_name);
    }

    // 润色配置校验（三档模式）
    use crate::core::config::PolishMode;
    if config.polish_mode != PolishMode::Disabled {
        if config.polish_mode == PolishMode::Intermediate && config.polish_min_interval <= 0.0 {
            log::warn!(
                "polish_mode=2 但 polish_min_interval={}<=0，将使用下限 {}s",
                config.polish_min_interval,
                crate::engine::coordinator::MIN_POLISH_INTERVAL_SEC
            );
        }
        match crate::core::config::llm_config(config.polish_mode) {
            Some(llm_cfg) => {
                let mode_str = match config.polish_mode {
                    PolishMode::FinalOnly => "仅最终润色",
                    PolishMode::Intermediate => "中间+最终",
                    // Disabled 理论上不会进 Some(llm_cfg) 分支（llm_config 返回 None），
                    // 但显式列出避免新增变体时 unreachable! panic 扼杀启动。
                    PolishMode::Disabled => "已禁用",
                };
                if config.polish_mode == PolishMode::Intermediate {
                    log::info!(
                        "润色模式: {} (min_interval={}s, provider={}, model={})",
                        mode_str,
                        config.polish_min_interval,
                        llm_cfg.provider,
                        llm_cfg.model
                    );
                } else {
                    log::info!(
                        "润色模式: {} (provider={}, model={})",
                        mode_str,
                        llm_cfg.provider,
                        llm_cfg.model
                    );
                }
            }
            None => {
                let active_llm = octopus_asr_local::config::resolve_active_engine("llm")
                    .map(|r| r.name)
                    .unwrap_or_default();
                log::warn!(
                    "polish_mode={:?} 但未找到有效的 LLM 配置（当前激活 LLM=\"{}\"，请检查 DB 中的 API Key 字段）",
                    config.polish_mode,
                    active_llm
                );
            }
        }
    }

    // 从 DB 加载激活的润色 prompt（prompts 表 content 字段存文件名引用）
    // 读 ~/.octopus/.sync/prompts/polish/<content>.md 文件内容。失败 fallback id=1。
    let active_id = octopus_infra::db::load_active_prompt_id().unwrap_or(1);
    let prompt_content = match octopus_infra::db::load_prompt(active_id) {
        Ok(Some(p)) => crate::commands::settings_commands::read_prompt_file(&p.content),
        Ok(None) => {
            log::warn!("active_polish_prompt id={} 不存在，fallback 到 id=1", active_id);
            let _ = octopus_infra::db::save_active_prompt_id(1);
            octopus_infra::db::load_prompt(1)
                .ok()
                .flatten()
                .map(|p| crate::commands::settings_commands::read_prompt_file(&p.content))
                .unwrap_or_default()
        }
        Err(e) => {
            log::warn!("DB 加载 prompt 失败（id={}）：{} —— 使用空 content 降级", active_id, e);
            String::new()
        }
    };
    octopus_llm::set_system_prompt(&prompt_content);
    log::info!("已加载润色 prompt（active id={}）", active_id);

    config
}
