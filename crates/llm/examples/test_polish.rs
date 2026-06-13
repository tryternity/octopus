//! LLM 润色链路测试。
//!
//! 读取 ~/.octopus/config.yaml 与 ~/.octopus/VOICE_POLISH.md，
//! 先发一个原始请求观察返回结构（诊断 reasoning_content 等），
//! 再调用 octopus_llm::polish() 验证封装链路。
//!
//! 用法：cargo run --release --package octopus-llm --example test_polish

use octopus_llm::{polish, set_system_prompt_override, CompatibleLlmConfig};
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Deserialize)]
struct LlmCfg {
    llm_provider: String,
    llm_model: String,
    llm_base_url: String,
    llm_secret_key: String,
}

fn octopus_home() -> PathBuf {
    let home = std::env::var("HOME").expect("HOME 未设置");
    PathBuf::from(home).join(".octopus")
}

fn main() -> anyhow::Result<()> {
    // 1. 加载 prompt override
    let prompt_path = octopus_home().join("VOICE_POLISH.md");
    if prompt_path.exists() {
        let content = std::fs::read_to_string(&prompt_path)?;
        let trimmed = content.trim();
        if !trimmed.is_empty() {
            set_system_prompt_override(trimmed.to_string());
            println!("✓ 已加载 VOICE_POLISH.md（{} 字节）", trimmed.len());
        }
    } else {
        println!("! 未找到 VOICE_POLISH.md，使用内置默认 prompt");
    }

    // 2. 加载 config.yaml
    let cfg_path = octopus_home().join("config.yaml");
    let text = std::fs::read_to_string(&cfg_path)
        .with_context(|| format!("读取配置失败: {}", cfg_path.display()))?;
    let cfg: LlmCfg = serde_yaml::from_str(&text)?;

    let key_preview = if cfg.llm_secret_key.len() > 6 {
        format!("{}…({} 字符)", &cfg.llm_secret_key[..6], cfg.llm_secret_key.len())
    } else {
        format!("({} 字符)", cfg.llm_secret_key.len())
    };
    println!(
        "配置: provider={}, model={}, base_url={}, key={}",
        cfg.llm_provider, cfg.llm_model, cfg.llm_base_url, key_preview
    );

    if cfg.llm_secret_key.is_empty() {
        anyhow::bail!("llm_secret_key 为空，无法测试");
    }

    let config = CompatibleLlmConfig {
        provider: cfg.llm_provider,
        model: cfg.llm_model,
        base_url: cfg.llm_base_url,
        secret_key: cfg.llm_secret_key,
    };

    // 3. 原始请求诊断：打印完整返回结构，定位 reasoning_content / 字段问题
    println!("\n=== ① 原始请求诊断（reply with one word: hello）===");
    let client = reqwest::blocking::Client::new();
    let url = format!("{}/chat/completions", config.base_url.trim_end_matches('/'));
    let body = serde_json::json!({
        "model": config.model,
        "messages": [{"role":"user","content":"reply with the single word: hello"}],
        "max_tokens": 50
    });
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", config.secret_key))
        .json(&body)
        .send();
    match resp {
        Ok(r) => {
            println!("status: {}", r.status());
            let txt = r.text().unwrap_or_default();
            // 美化打印 JSON
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&txt) {
                println!("{}", serde_json::to_string_pretty(&v).unwrap_or(txt));
            } else {
                println!("body (非 JSON): {}", txt);
            }
        }
        Err(e) => println!("请求失败: {:#}", e),
    }

    // 4. 调用封装好的 polish()
    println!("\n=== ② polish() 封装链路验证 ===");
    let input = "嗯那个今天我们就是说一下，关于这个项目的时间，三点不对四点吧，想跟大家同步一下进度";
    println!("输入: {}", input);

    // 4a. 配置中的 model（deepseek-v4-flash，带 reasoning）
    println!("\n-- 4a. model = {} （配置值，带 reasoning）--", config.model);
    match polish(input, &config) {
        Ok(out) => println!("输出 ({} 字符): {}", out.chars().count(), out),
        Err(e) => println!("✗ {:#}", e),
    }

    // 4b. 对照：deepseek-chat（非思考模型）
    let mut cfg_chat = config.clone();
    cfg_chat.model = "deepseek-chat".into();
    println!("\n-- 4b. model = deepseek-chat （对照，非思考模型）--");
    match polish(input, &cfg_chat) {
        Ok(out) => {
            println!("输出 ({} 字符): {}", out.chars().count(), out);
            println!("\n✓ deepseek-chat 润色正常");
        }
        Err(e) => println!("✗ {:#}", e),
    }

    Ok(())
}

// 兼容 with_context（anyhow 已引入，补 Context trait）
use anyhow::Context as _;
