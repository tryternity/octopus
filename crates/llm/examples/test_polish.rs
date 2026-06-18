//! LLM 润色链路测试。
//!
//! 读取 ~/.octopus/config.yaml 与 ~/.octopus/VOICE_POLISH.md，
//! 先发一个原始请求观察返回结构（诊断 reasoning_content 等），
//! 再调用 octopus_llm::polish() 验证封装链路。
//!
//! 用法：cargo run --release --package octopus-llm --example test_polish

use octopus_infra::{consts::VOICE_POLISH_FILE, octopus_config_home};
use octopus_llm::{polish, set_system_prompt_override};
use serde::Deserialize;

#[derive(Deserialize)]
struct LlmCfg {
    #[serde(default = "default_polish_llm")]
    polish_llm: String,
}

fn default_polish_llm() -> String {
    "bigmodel:glm:glm-4-flashx".to_string()
}

fn main() -> anyhow::Result<()> {
    // 1. 加载 prompt override
    let prompt_path = octopus_config_home().join(VOICE_POLISH_FILE);
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

    // 2. 加载 config.yaml 获取 polish_llm
    let cfg_path = octopus_config_home().join("config.yaml");
    let text = if cfg_path.exists() {
        std::fs::read_to_string(&cfg_path)?
    } else {
        String::new()
    };
    let cfg: LlmCfg = serde_yaml::from_str(&text).unwrap_or(LlmCfg {
        polish_llm: default_polish_llm(),
    });

    println!("正在初始化数据库以加载模型配置...");
    octopus_asr::db::ensure_db()?;

    println!("正在从数据库加载 LLM 配置: {}...", cfg.polish_llm);
    let config = match octopus_asr::db::load_llm_model(&cfg.polish_llm)? {
        Some(c) => c,
        None => {
            anyhow::bail!("数据库中未找到 LLM 模型 '{}' 的配置", cfg.polish_llm);
        }
    };

    let key_preview = if config.secret_key.len() > 6 {
        format!("{}…({} 字符)", &config.secret_key[..6], config.secret_key.len())
    } else {
        format!("({} 字符)", config.secret_key.len())
    };
    println!(
        "配置: provider={}, model={}, base_url={}, key={}",
        config.provider, config.model, config.base_url, key_preview
    );

    if config.secret_key.is_empty() {
        println!("! 数据库中的 API Key (secret_key) 为空，将尝试无 Key 模式发送请求...");
    }

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


