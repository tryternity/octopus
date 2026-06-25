//! 云端 ASR 配置解析 + provider 分发（复刻 desktop cloud_pipeline.rs 的 open 部分）。
//!
//! 与 desktop 差异：无 tauri runtime 依赖；`open_cloud_session` 同步返回 `CloudStreamHandle`
//!（各 provider `open()` 内部 `tokio::spawn`，须在 tokio 上下文调用）。

use crate::cloud_types::CloudStreamHandle;
use octopus_asr::config::{self, EngineCategory};

/// 通用云端配置解析：从 DB section 取 ModelEntry + 校验 secret_key 非空。
fn resolve_cloud_entry<'a>(
    section: Option<&'a std::collections::HashMap<String, octopus_infra::db::ModelEntry>>,
    provider: &'a str,
    model_name: &'a str,
) -> std::result::Result<&'a octopus_infra::db::ModelEntry, String> {
    let entry = section
        .and_then(|m| m.get(model_name))
        .ok_or_else(|| format!("{} ASR 模型 '{}' 未在 DB 配置", provider, model_name))?;
    if entry.secret_key.is_empty() {
        return Err(format!("{} ASR 模型 '{}' 的 secret_key 为空", provider, model_name));
    }
    Ok(entry)
}

/// 解析 Aliyun（DashScope）配置（endpoint + key + model_name）。
fn resolve_aliyun_config(engine_spec: &str) -> std::result::Result<(String, String, String), String> {
    let cfg = config::load_config().map_err(|e| e.to_string())?;
    let model_name = octopus_infra::db::parse_model_spec(engine_spec)
        .model_name()
        .to_string();
    let entry = resolve_cloud_entry(cfg.asr.aliyun.as_ref(), "aliyun", &model_name)?;
    Ok((entry.source.clone(), entry.secret_key.clone(), model_name))
}

/// 解析 ByteDance（豆包）配置（resource_id + api_key + model_name）。
fn resolve_bytedance_config(engine_spec: &str) -> std::result::Result<(String, String, String), String> {
    let cfg = config::load_config().map_err(|e| e.to_string())?;
    let model_name = octopus_infra::db::parse_model_spec(engine_spec)
        .model_name()
        .to_string();
    let entry = resolve_cloud_entry(cfg.asr.bytedance.as_ref(), "bytedance", &model_name)?;
    Ok((entry.source.clone(), entry.secret_key.clone(), model_name))
}

/// 解析 Tencent（腾讯云）配置（appid:secretid + secret_key + engine_model_type）。
fn resolve_tencent_config(engine_spec: &str) -> std::result::Result<(String, String, String), String> {
    let cfg = config::load_config().map_err(|e| e.to_string())?;
    let model_name = octopus_infra::db::parse_model_spec(engine_spec)
        .model_name()
        .to_string();
    let entry = resolve_cloud_entry(cfg.asr.tencent.as_ref(), "tencent", &model_name)?;
    if !entry.source.contains(':') {
        return Err(format!(
            "tencent ASR 模型 '{}' 的 source 字段格式应为 appid:secretid（当前='{}'）",
            model_name, entry.source
        ));
    }
    Ok((entry.source.clone(), entry.secret_key.clone(), model_name))
}

/// 解析 Baidu（百度云）配置（appid + api_key + dev_pid）。
fn resolve_baidu_config(engine_spec: &str) -> std::result::Result<(String, String, String), String> {
    let cfg = config::load_config().map_err(|e| e.to_string())?;
    let model_name = octopus_infra::db::parse_model_spec(engine_spec)
        .model_name()
        .to_string();
    let entry = resolve_cloud_entry(cfg.asr.baidu.as_ref(), "baidu", &model_name)?;
    if entry.source.is_empty() {
        return Err(format!(
            "baidu ASR 模型 '{}' 的 source 字段（AppID）为空",
            model_name
        ));
    }
    Ok((entry.source.clone(), entry.secret_key.clone(), model_name))
}

/// 根据 spec 解析配置 + 打开对应云端 WS session（同步返回句柄）。
///
/// `asr_engine` 是完整 spec。**须在 tokio runtime 上下文调用**
///（各 provider `open` 内部 `tokio::spawn`）。
///
/// 返回 `Result<_, String>`（与 desktop 版 `cloud_pipeline::open_cloud_session` 一致）：
/// 各 resolve helper 已返回 `Result<_, String>`、provider `open` 返回 anyhow —
/// 后者用 `map_err(|e| e.to_string())` 统一到 String，避免 anyhow → String 转换里
/// `String: std::error::Error` 未实现导致 `?` 编译失败。
pub fn open_cloud_session(
    asr_engine: &str,
    language: &str,
    pre_roll: Vec<f32>,
) -> std::result::Result<CloudStreamHandle, String> {
    match config::resolve_engine_category(asr_engine) {
        Some(EngineCategory::Aliyun) => {
            let (endpoint, key, model) = resolve_aliyun_config(asr_engine)?;
            crate::aliyun_stream::open(endpoint, key, model, language.to_string(), pre_roll)
                .map_err(|e| e.to_string())
        }
        Some(EngineCategory::ByteDance) => {
            let (resource_id, api_key, _) = resolve_bytedance_config(asr_engine)?;
            crate::bytedance_stream::open(api_key, resource_id, language.to_string(), pre_roll)
                .map_err(|e| e.to_string())
        }
        Some(EngineCategory::Tencent) => {
            let (appid_secretid, secret_key, engine_model_type) =
                resolve_tencent_config(asr_engine)?;
            crate::tencent_stream::open(
                appid_secretid,
                secret_key,
                engine_model_type,
                language.to_string(),
                pre_roll,
            )
            .map_err(|e| e.to_string())
        }
        Some(EngineCategory::Baidu) => {
            let (appid, appkey, dev_pid) = resolve_baidu_config(asr_engine)?;
            crate::baidu_stream::open(appid, appkey, dev_pid, language.to_string(), pre_roll)
                .map_err(|e| e.to_string())
        }
        None => Err(format!("spec='{}' 未匹配任何已配置 ASR 引擎（resolve_engine_category 返回 None）", asr_engine)),
        _ => Err(format!("当前引擎非云端（spec='{}'），无法开启 WSS", asr_engine)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_cloud_session_rejects_unresolvable_spec() {
        // 不存在的 spec → resolve_engine_category 返回 None → Err。
        // 无需 tokio runtime（在 spawn 前就返回 Err），不依赖 DB 是否有该引擎条目。
        let res = open_cloud_session("nonexistent:foo:bar", "zh", Vec::new());
        assert!(res.is_err());
        if let Err(msg) = res {
            // 未知 spec → None 分支返回 "未匹配任何已配置 ASR 引擎"
            assert!(
                msg.contains("未匹配任何已配置") || msg.contains("非云端") || msg.contains("无法开启 WSS"),
                "unexpected error: {}",
                msg
            );
        }
    }

    #[test]
    fn open_cloud_session_rejects_local_spec() {
        // 本地/非云端 spec（whisper）→ resolve_engine_category 返回 None 或 Some(本地族)，
        // 两种情况下 open_cloud_session 都应返回 Err（不进入任何 provider open 分支）。
        // 不依赖 DB 是否实际有 whisper 条目：None 走 None 分支、本地族走 `_` 分支，均 Err。
        // 无需 tokio runtime（在 spawn 前就返回 Err）。
        let res = open_cloud_session("whisper", "zh", Vec::new());
        assert!(res.is_err());
        if let Err(msg) = res {
            assert!(
                msg.contains("非云端") || msg.contains("无法开启 WSS") || msg.contains("未匹配任何已配置"),
                "unexpected error: {}",
                msg
            );
        }
    }
}
