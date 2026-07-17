//! 云端 ASR 配置解析 + provider 分发（复刻 desktop cloud_pipeline.rs 的 open 部分）。
//!
//! 与 desktop 差异：无 tauri runtime 依赖；`open_cloud_session` 同步返回 `CloudStreamHandle`
//!（各 provider `open()` 内部 `tokio::spawn`，须在 tokio 上下文调用）。

use anyhow::{bail, Result};
use crate::cloud_types::CloudStreamHandle;
use octopus_asr_local::config::{self, EngineCategory};

/// 解析 spec → ModelEntry（resolve_engine_any 查 DB 任意可用 ASR，不限激活），
/// 校验 secret_key 非空，返回 (entry, model_name)。
fn resolve_cloud_entry(engine_spec: &str, provider: &str) -> Result<(octopus_infra::db::ModelEntry, String)> {
    let model_name = octopus_infra::db::parse_model_spec(engine_spec)
        .model_name()
        .to_string();
    let (_cat, entry) = config::resolve_engine_any(engine_spec)
        .ok_or_else(|| anyhow::anyhow!("{} ASR 模型 '{}' 未在 DB 配置", provider, model_name))?;
    if entry.secret_key.is_empty() {
        bail!("{} ASR 模型 '{}' 的 secret_key 为空", provider, model_name);
    }
    Ok((entry, model_name))
}

/// 解析 Aliyun（DashScope）配置（endpoint + key + model_name）。
fn resolve_aliyun_config(engine_spec: &str) -> Result<(String, String, String)> {
    let (entry, model_name) = resolve_cloud_entry(engine_spec, "aliyun")?;
    Ok((entry.source, entry.secret_key, model_name))
}

/// 解析 ByteDance（豆包）配置（resource_id + api_key + model_name）。
fn resolve_bytedance_config(engine_spec: &str) -> Result<(String, String, String)> {
    let (entry, model_name) = resolve_cloud_entry(engine_spec, "bytedance")?;
    Ok((entry.source, entry.secret_key, model_name))
}

/// 解析 Tencent（腾讯云）配置（appid:secretid + secret_key + engine_model_type）。
fn resolve_tencent_config(engine_spec: &str) -> Result<(String, String, String)> {
    let (entry, model_name) = resolve_cloud_entry(engine_spec, "tencent")?;
    if !entry.source.contains(':') {
        bail!(
            "tencent ASR 模型 '{}' 的 source 字段格式应为 appid:secretid（当前='{}'）",
            model_name,
            entry.source
        );
    }
    Ok((entry.source, entry.secret_key, model_name))
}

/// 解析 Baidu（百度云）配置（appid + api_key + dev_pid）。
fn resolve_baidu_config(engine_spec: &str) -> Result<(String, String, String)> {
    let (entry, model_name) = resolve_cloud_entry(engine_spec, "baidu")?;
    if entry.source.is_empty() {
        bail!("baidu ASR 模型 '{}' 的 source 字段（AppID）为空", model_name);
    }
    Ok((entry.source, entry.secret_key, model_name))
}

/// 根据 spec 解析配置 + 打开对应云端 WS session（同步返回句柄）。
///
/// `asr_engine` 是完整 spec。**须在 tokio runtime 上下文调用**
///（各 provider `open` 内部 `tokio::spawn`）。
///
/// 返回 `anyhow::Result`：批引擎 `CloudBatchEngine::transcribe` 用 `?` 传播，须 anyhow。
pub fn open_cloud_session(
    asr_engine: &str,
    language: &str,
    pre_roll: Vec<f32>,
) -> Result<CloudStreamHandle> {
    match config::resolve_engine_category_any(asr_engine) {
        Some(EngineCategory::Aliyun) => {
            let (endpoint, key, model) = resolve_aliyun_config(asr_engine)?;
            crate::aliyun_stream::open(endpoint, key, model, language.to_string(), pre_roll)
                .map_err(|e| anyhow::anyhow!("{}", e))
        }
        Some(EngineCategory::ByteDance) => {
            let (resource_id, api_key, _) = resolve_bytedance_config(asr_engine)?;
            crate::bytedance_stream::open(api_key, resource_id, language.to_string(), pre_roll)
                .map_err(|e| anyhow::anyhow!("{}", e))
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
            .map_err(|e| anyhow::anyhow!("{}", e))
        }
        Some(EngineCategory::Baidu) => {
            let (appid, appkey, dev_pid) = resolve_baidu_config(asr_engine)?;
            crate::baidu_stream::open(appid, appkey, dev_pid, language.to_string(), pre_roll)
                .map_err(|e| anyhow::anyhow!("{}", e))
        }
        None => bail!("spec='{}' 未匹配任何已配置 ASR 引擎（resolve_engine_category_any 返回 None）", asr_engine),
        _ => bail!("当前引擎非云端（spec='{}'），无法开启 WSS", asr_engine),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_cloud_session_rejects_unresolvable_spec() {
        // 不存在的 spec → resolve_engine_category_any 返回 None → Err。
        // 无需 tokio runtime（在 spawn 前就返回 Err），不依赖 DB 是否有该引擎条目。
        let res = open_cloud_session("nonexistent:foo:bar", "zh", Vec::new());
        assert!(res.is_err());
        let msg = format!("{}", res.err().unwrap());
        // 未知 spec → None 分支返回 "未匹配任何已配置 ASR 引擎"
        assert!(
            msg.contains("未匹配任何已配置") || msg.contains("非云端") || msg.contains("无法开启 WSS"),
            "unexpected error: {}",
            msg
        );
    }

    #[test]
    fn open_cloud_session_rejects_local_spec() {
        // 本地/非云端 spec（whisper）→ resolve_engine_category_any 返回 None 或 Some(本地族)，
        // 两种情况下 open_cloud_session 都应返回 Err（不进入任何 provider open 分支）。
        // 不依赖 DB 是否实际有 whisper 条目：None 走 None 分支、本地族走 `_` 分支，均 Err。
        // 无需 tokio runtime（在 spawn 前就返回 Err）。
        let res = open_cloud_session("whisper", "zh", Vec::new());
        assert!(res.is_err());
        let msg = format!("{}", res.err().unwrap());
        assert!(
            msg.contains("非云端") || msg.contains("无法开启 WSS") || msg.contains("未匹配任何已配置"),
            "unexpected error: {}",
            msg
        );
    }
}
