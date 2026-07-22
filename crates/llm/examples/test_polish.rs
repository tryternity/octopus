//! LLM 润色链路测试。
//!
//! 从 DB 加载激活的润色 prompt 与 LLM 配置，
//! 先发一个原始请求观察返回结构（诊断 reasoning_content 等），
//! 再调用 octopus_llm::polish() 验证封装链路。
//!
//! 用法：cargo run --release --package octopus-llm --example test_polish
//!
//! **注意**：此 example 直接读开发库 ~/.octopus/octopus.db 的 prompt 和 LLM 配置，
//! 并会调真实 LLM API（产生费用）。不会写 DB（只读）。
//! 若要指向其他库，设 `OCTOPUS_DB_PATH=/path/to/test.db`。

use octopus_llm::{polish, set_system_prompt};

fn main() -> anyhow::Result<()> {
    // 1. 从 DB 加载激活的润色 prompt
    octopus_asr_local::db::ensure_db()?;
    let active_id = octopus_infra::db::load_active_prompt_id()?;
    let prompt_record = octopus_infra::db::load_prompt(active_id)?
        .ok_or_else(|| anyhow::anyhow!("DB 中未找到 active prompt id={}", active_id))?;
    set_system_prompt(&prompt_record.content);
    println!("✓ 已加载 prompt（id={} title={}）", prompt_record.id, prompt_record.title);

    // 2. 加载 LLM 激活模型配置（Task 2 重构后：从 ACTIVE_ENGINES 内存缓存取，不再读 AppConfig.polish_llm）
    octopus_asr_local::config::reload_active_engine("llm").ok();
    let resolved = octopus_asr_local::config::resolve_active_engine("llm")
        .map_err(|e| anyhow::anyhow!("LLM 域无激活模型（{}）。请在设置中激活一个 LLM 模型", e))?;
    println!("正在使用激活的 LLM 模型: {}...", resolved.name);
    let config = octopus_llm::CompatibleLlmConfig {
        provider: resolved.provider.clone(),
        model: resolved.name.clone(),
        base_url: resolved.entry.source.clone(),
        secret_key: resolved.entry.secret_key.clone(),
        is_thinking: resolved.is_thinking,
        source_type: resolved.entry.source_type,
        is_enabled: resolved.entry.is_enabled,
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
    match polish(None, input, &config) {
        Ok(out) => println!("输出 ({} 字符): {}", out.chars().count(), out),
        Err(e) => println!("✗ {:#}", e),
    }

    // 4b. 对照：deepseek-chat（非思考模型）
    let mut cfg_chat = config.clone();
    cfg_chat.model = "deepseek-chat".into();
    println!("\n-- 4b. model = deepseek-chat （对照，非思考模型）--");
    match polish(None, input, &cfg_chat) {
        Ok(out) => {
            println!("输出 ({} 字符): {}", out.chars().count(), out);
            println!("\n✓ deepseek-chat 润色正常");
        }
        Err(e) => println!("✗ {:#}", e),
    }

    Ok(())
}


