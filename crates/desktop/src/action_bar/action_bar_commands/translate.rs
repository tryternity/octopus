//! 翻译引擎策略 + 流式翻译 + 结果缓存（从 action_bar_commands/mod.rs 提取，Task 1.5）。
//!
//! 涵盖：方向检测 / 策略解析（本地 opus-mt / m2m100、云端 OpenAI 兼容、润色 LLM 兜底）、
//! 流式分段翻译 + emit、CompactEditor session 结果缓存（兜底 Tauri 事件丢失）、
//! URL 编码辅助（`url_encode_param` 供 execute_action_bar_inner 复用）。

use parking_lot::Mutex;
use tauri::{AppHandle, Emitter};
use crate::core::error_util::{e2s, e2s_ctx};

/// 按 CJK 检测方向，返回翻译 system prompt。
pub(crate) fn auto_translate_prompt(text: &str) -> &'static str {
    let has_cjk = text.chars().any(|c| {
        matches!(c as u32, 0x4e00..=0x9fff | 0x3040..=0x30ff | 0xac00..=0xd7af)
    });
    if has_cjk {
        "Please translate the following text into English. Only output the translation."
    } else {
        "请将以下文本翻译成中文。只输出翻译结果。"
    }
}

/// 翻译策略——只决定路径，不预加载引擎。
///
/// `translate_engine` 配置项现在存 DB models 行的 id（Task 3 `get_model_by_id`）。
/// 策略解析阶段只查 DB + 基本校验，真正引擎加载延迟到 `do_translate` 里
/// （opus-mt 需按文本方向加载子目录、m2m100/云端按需实例化）。
pub(crate) enum TranslateStrategy {
    /// 本地翻译模型（opus-mt / m2m100 等），engine 字段延迟加载。
    LocalModel { resolved: octopus_asr_local::config::ResolvedEngine },
    /// 云端 LLM 翻译模型（OpenAI 兼容协议），engine 字段延迟加载。
    CloudModel { resolved: octopus_asr_local::config::ResolvedEngine },
    /// 无激活翻译模型 / 未填 key / 未配置润色 LLM → 走润色 LLM 兜底翻译。
    FallbackLlm,
}

/// 解析翻译策略。async 仅因未来可能涉及异步 DB 访问；当前 DB 同步，
/// 但保留 async 签名以匹配 `do_translate` 的 async 上下文（无需 spawn_blocking）。
///
/// Task 2 后：翻译激活模型从 ACTIVE_ENGINES 缓存取（resolve_active_engine("translate")）。
pub(crate) async fn resolve_translate_strategy(_config: &octopus_infra::config::AppConfig) -> TranslateStrategy {
    let resolved = match octopus_asr_local::config::resolve_active_engine("translate") {
        Ok(r) => r,
        Err(_) => return TranslateStrategy::FallbackLlm,
    };
    if resolved.entry.is_local_or_builtin() {
        TranslateStrategy::LocalModel { resolved }
    } else if resolved.entry.secret_key.is_empty() {
        // 云端模型未填 secret_key → fallback（避免到 translate 时才报错）
        TranslateStrategy::FallbackLlm
    } else {
        TranslateStrategy::CloudModel { resolved }
    }
}

fn detect_translate_direction(text: &str) -> (&'static str, &'static str) {
    let has_cjk = text.chars().any(|c| {
        matches!(c as u32, 0x4e00..=0x9fff | 0x3040..=0x30ff | 0xac00..=0xd7af)
    });
    if has_cjk {
        ("zh", "en")
    } else {
        ("en", "zh")
    }
}

/// 执行翻译（公共逻辑）：解析引擎策略 + 执行翻译。
/// 供 do_translate_streaming（worker 线程 block_on）和 coordinator 终翻路径复用。
///
/// async 化以支持 Task 1 的 async `TranslationEngine::translate`（云端引擎走 HTTP）。
/// 两个调用点都在非 tokio 线程（worker / coordinator），通过
/// `tauri::async_runtime::block_on` 进入——不可新建 `tokio::runtime::Runtime`（嵌套 panic）。
pub(crate) async fn do_translate(text: &str, config: &octopus_infra::config::AppConfig) -> Result<String, String> {
    let (source_lang, target_lang) = detect_translate_direction(text);
    match resolve_translate_strategy(config).await {
        TranslateStrategy::LocalModel { resolved } => {
            // opus-mt 按方向加载子目录（zh-en / en-zh）
            if resolved.name.starts_with("opus-mt") {
                let engine = octopus_translation::load_opus_mt(source_lang, target_lang)
                    .map_err(e2s)?;
                return engine.translate(text, source_lang, target_lang).await
                    .map_err(e2s);
            }
            // m2m100 等其他本地引擎：按 model_name 构造 spec 加载
            let manager = octopus_translation::TranslationManager::new(&format!("local:{}", resolved.name));
            let engine = manager.engine()
                .map_err(e2s)?
                .ok_or_else(|| "本地翻译引擎加载失败".to_string())?;
            engine.translate(text, source_lang, target_lang).await
                .map_err(e2s)
        }
        TranslateStrategy::CloudModel { resolved } => {
            // 云端引擎（OpenAI 兼容）——内部 reqwest::blocking。
            // follow-up #7：secret_key 可能是 v1: 加密格式（vault 启用后 Task 20 迁移过），
            // 透明解密得到明文 API Key。本地 / 未迁移明文 → no-op 返回原值。
            // 安全修复 #5：vault 启用但解密失败 → Err，不把密文当 bearer 发到云端。
            let secret_key_plain = crate::vault::vault_secret_access::try_decrypt_secret_global(
                &resolved.entry.secret_key,
            )
            .map_err(|_| "云端翻译失败：保险库未解锁或密文损坏，请先解锁保险库".to_string())?;
            // 第十五轮 P2-A：reqwest::blocking 检测 tokio runtime context，block_on 进入
            // runtime 后 future 在 worker poll → reqwest::blocking panic
            // "Cannot start a runtime from within a runtime"。translation crate 是纯推理库
            //（无 tokio dep），不能在 CloudLlmEngine::translate 内 spawn_blocking，故在此
            //（desktop，有 tokio）用 spawn_blocking 隔离，对齐 FallbackLlm :117 模式。
            let config = octopus_llm::CompatibleLlmConfig {
                provider: resolved.provider.clone(),
                model: resolved.name.clone(),
                base_url: resolved.entry.source.clone(),
                secret_key: secret_key_plain,
                is_thinking: resolved.is_thinking,
                source_type: 2,
                is_enabled: true,
            };
            let prompt = octopus_translation::cloud::build_translate_prompt(source_lang, target_lang);
            let text_owned = text.to_string();
            tokio::task::spawn_blocking(move || {
                octopus_llm::chat_text_with_prompt(&prompt, &text_owned, &config, None)
            }).await
                .map_err(|e| e2s_ctx("云端翻译线程异常: {}", e))?
                .map_err(e2s)
        }
        TranslateStrategy::FallbackLlm => {
            let llm_config = crate::core::config::llm_config_ignore_mode()
                .ok_or_else(|| "翻译 fallback LLM 未配置，请在设置中配置润色模型".to_string())?;
            let prompt = auto_translate_prompt(text); // &'static str，满足 'static move
            let text_owned = text.to_string();
            let llm_config_owned = llm_config.clone();
            // LLM 调用是同步阻塞 HTTP——spawn_blocking 防卡 tokio runtime
            tokio::task::spawn_blocking(move || {
                octopus_llm::chat_text_with_prompt(prompt, &text_owned, &llm_config_owned, None)
            }).await
                .map_err(|e| e2s_ctx("LLM 线程异常: {}", e))?
                .map_err(e2s)
        }
    }
}

/// 流式翻译事件目标——决定 emit 哪套事件名 + payload 结构。
///
/// 2026-07-17 修复发现 1（竞态）+ 6（跨窗口泄漏）+ 8（并发错路由）：
/// - **CompactEditor**：emit 新事件名 `compact-editor://translate-progress|done`，
///   payload 是 `TranslateSessionPayload { sessionId, text }`。前端按 sessionId 路由
///   到具体 tab（Map 而非单值 ref），解决并发翻译错路由 + ActionBar 流式翻译与
///   open-tab emit 的竞态（payload 带 sessionId 即可正确路由，不依赖 ref 时序）。
///   新事件名与 Result 窗口订阅的旧事件名彻底隔离，解决跨窗口泄漏。
/// - **Result**：保留旧事件名 `translate-progress|done`，payload 仍是裸 String。
///   Result 一次只翻译一段，无需 sessionId。
#[derive(Clone)]
pub(crate) enum TranslateEmitTarget {
    Result,
    CompactEditor { session_id: String },
}

impl TranslateEmitTarget {
    /// 根据 target emit 翻译进度。payload 已封装好。
    fn emit_progress(&self, app: &AppHandle, text: &str) {
        match self {
            TranslateEmitTarget::Result => {
                let _ = app.emit("translate-progress", text);
            }
            TranslateEmitTarget::CompactEditor { session_id } => {
                let _ = app.emit(
                    "compact-editor://translate-progress",
                    TranslateSessionPayload { session_id: session_id.clone(), text: text.to_string() },
                );
                // 不缓存 progress——多段翻译时 listener 已注册必接管 progress 增量，
                // 缓存 progress 会让 invoke 回调的旧快照覆盖 listener 更新的译文（瑕疵 1）。
                // progress 增量实时性交给 listener；缓存只兜 done 终止态（瑕疵 3：避免残留）。
            }
        }
    }

    /// 根据 target emit 翻译完成。
    fn emit_done(&self, app: &AppHandle, text: &str) {
        match self {
            TranslateEmitTarget::Result => {
                let _ = app.emit("translate-done", text);
            }
            TranslateEmitTarget::CompactEditor { session_id } => {
                let _ = app.emit(
                    "compact-editor://translate-done",
                    TranslateSessionPayload { session_id: session_id.clone(), text: text.to_string() },
                );
                cache_translate_done(session_id, text);
            }
        }
    }
}

/// CompactEditor 翻译事件 payload——带 sessionId 供前端路由。
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct TranslateSessionPayload {
    session_id: String,
    text: String,
}

/// 翻译结果缓存条目——R2 残余疑点根治（仅缓存 done 终止态）。
///
/// **背景**：Tauri v2 对未注册 listener 不缓存事件（fire-and-forget）。新窗口路径下
/// webview 加载（JS bundle + React mount + 三个串行 await listen）需数百 ms，与
/// `do_translate_streaming` spawn 线程并行；缓存命中 + 短文本时 done 可 < 100ms，
/// 落在 listener 注册前 → 事件直接丢失 → tab 永久 loading。
///
/// **根治**：后端仅缓存每个 CompactEditor session 的 **done 终止态**。前端 mount 完成后
/// 通过 `get_translate_result(sessionId)` 主动拉取——返回 Some → 直接显示，绕过事件丢失。
///
/// **为何只缓存 done 不缓存 progress**（2026-07-17 优化）：
/// - 多段翻译时 progress 增量靠 listener 实时更新；若 invoke 回调返回旧 progress 快照
///   会覆盖 listener 已更新的译文（瑕疵 1：译文瞬时回退闪烁）
/// - listener 未注册阶段丢失 progress 无影响——下一段 progress 会到达，listener 接管
/// - 唯一竞态：单段短文本 + 缓存命中 < 100ms done 落在 listener 注册前 → done 缓存兜底
/// - 不缓存 progress 顺带根治瑕疵 3（listener 正常收到 done 时后端无 progress 残留）
///
/// Result 路径不缓存（无 sessionId，且 Result 窗口常驻不涉及此竞态）。
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedTranslateResult {
    text: String,
}

/// 翻译结果缓存：session_id → done 终止态译文。带上限避免无限增长。
static TRANSLATE_RESULTS: once_cell::sync::Lazy<Mutex<std::collections::HashMap<String, CachedTranslateResult>>> =
    once_cell::sync::Lazy::new(|| Mutex::new(std::collections::HashMap::new()));
const TRANSLATE_RESULTS_MAX: usize = 64;

/// 缓存 session 的 done 终止态译文。仅 done 时调用（progress 不缓存）。
fn cache_translate_done(session_id: &str, text: &str) {
    let mut map = TRANSLATE_RESULTS.lock();
    // 超上限随机删一个。非真 LRU，理论上可能删到尚未被 forget/get 的活跃 session
    // （极低概率：需 64 积压 + 挤出命中未取走条目）。但 listener 主路径会调
    // forget_translate_result 清理，加上翻译完成频率低（用户手动触发），稳态
    // 远低于 64 上限，挤出几乎不发生。即使发生，listener 主路径已显示译文，
    // 仅影响后续 invoke 兜底查询。
    if map.len() >= TRANSLATE_RESULTS_MAX && !map.contains_key(session_id) {
        if let Some(first_key) = map.keys().next().cloned() {
            map.remove(&first_key);
        }
    }
    map.insert(session_id.to_string(), CachedTranslateResult { text: text.to_string() });
}

/// 前端查询翻译结果（兜底 listener 未注册阶段的 done 事件丢失）。
///
/// **仅在 invoke 兜底分支调用**（`registerTranslateSession` 内 pending 未命中时）。
/// listener 主路径（已收到 done）走 `forget_translate_result` 清理，不经过本命令。
///
/// 前端 CompactEditor mount 完成后，对每个 pending / open-tab 携带的 sessionId 调一次：
/// - 返回 Some → session 已 done，直接显示译文终止态
/// - 返回 None → session 未开始 / 进行中 / done 已被取走，等 listener（已注册必接管）
///
/// 查询同时取走（remove），session 一次性消费。
///
/// **单次锁原子操作**（2026-07-17 修复瑕疵 2）：原先 get + remove 分两次 lock 存在
/// TOCTOU 间隙，合并为单次锁 + remove。
#[tauri::command]
pub fn get_translate_result(session_id: String) -> Option<CachedTranslateResult> {
    TRANSLATE_RESULTS.lock().remove(&session_id)
}

/// 通知后端丢弃某 session 的翻译结果缓存——listener 正常收到 done 时调用。
///
/// **为何需要**（2026-07-17 疑点 A 根治）：`get_translate_result` 仅在 invoke 兜底
/// 分支调用，listener 主路径（key 存在、直接 setTabs）不查缓存 → 后端 done 条目
/// 永不被取走，稳态常驻 64 条至 LRU 挤出。此命令让 listener 主路径显式清理——
/// "我已收到 done，你丢弃"幂等语义。session_id 不存在时静默成功。
///
/// 与 `get_translate_result` 的区别：get 返回值（兜底场景需要显示译文），
/// forget 纯清理（listener 已显示，只需后端释放）。
#[tauri::command]
pub fn forget_translate_result(session_id: String) {
    TRANSLATE_RESULTS.lock().remove(&session_id);
}

/// 流式翻译：按段落（换行）切分，逐段翻译，每段完成 emit 累积结果。
///
/// `target` 决定 emit 的事件名 + payload 结构（详见 `TranslateEmitTarget`）。
pub(crate) fn do_translate_streaming(text: &str, app: &AppHandle, target: TranslateEmitTarget) {
    let config = match octopus_infra::config::load_config() {
        Ok(c) => c,
        Err(e) => {
            target.emit_done(app, &format!("❌ 配置加载失败: {}", e));
            return;
        }
    };

    // 按换行切分段落，逐段翻译
    let segments: Vec<&str> = text.split('\n').collect();
    let total = segments.len();
    let mut accumulated = String::new();

    for (i, seg) in segments.iter().enumerate() {
        if seg.trim().is_empty() {
            if i < total - 1 { accumulated.push('\n'); }
            continue;
        }
        // tauri::async_runtime::block_on 复用 Tauri 全局 tokio runtime（cloud_pipeline.rs:122 同模式）。
        // 本函数在 std::thread::spawn 的 worker 线程跑（translate_text / execute_action 都这样调），
        // 非 tokio worker 线程，block_on 不会嵌套 panic。不可用 `tokio::runtime::Runtime::new()`。
        match tauri::async_runtime::block_on(do_translate(seg, &config)) {
            Ok(t) => {
                accumulated.push_str(&t);
            }
            Err(e) => {
                accumulated = format!("❌ 翻译失败: {}", e);
                break;
            }
        }
        if i < total - 1 { accumulated.push('\n'); }

        // 每段完成 emit 增量结果（前端实时更新译文区）
        target.emit_progress(app, &accumulated);
    }

    target.emit_done(app, &accumulated);
}

/// 前端工具栏翻译按钮调用。fire-and-forget：立即返回，翻译结果通过事件 emit。
///
/// `target_type`：`"result"` 走旧事件名（Result 窗口），`"compact_editor"` 走新事件名
/// （CompactEditor，带 sessionId）。返回 sessionId 供前端路由（`"result"` 时返回空串）。
///
/// 前端 invoke 后立即切 contrast 模式（译文区显示 loading），listen 翻译事件更新。
#[tauri::command]
pub fn translate_text(text: String, target_type: String, app: AppHandle) -> Result<String, String> {
    let target = match target_type.as_str() {
        "compact_editor" => {
            let session_id = uuid::Uuid::new_v4().to_string();
            TranslateEmitTarget::CompactEditor { session_id: session_id.clone() }
        }
        _ => TranslateEmitTarget::Result,
    };
    let session_id = match &target {
        TranslateEmitTarget::CompactEditor { session_id } => session_id.clone(),
        TranslateEmitTarget::Result => String::new(),
    };
    let app_clone = app.clone();
    std::thread::spawn(move || {
        do_translate_streaming(&text, &app_clone, target);
    });
    Ok(session_id)
}

/// URL 查询参数编码：保留 RFC 3986 unreserved（A-Za-z0-9-_.~），其余百分号编码。
/// 复用 percent-encoding 库（项目已为 file 协议引入）。
pub(crate) fn url_encode_param(s: &str) -> String {
    use percent_encoding::{utf8_percent_encode, AsciiSet, CONTROLS};
    /// unreserved 之外需编码的 ASCII 字符
    const ENCODE_SET: &AsciiSet = &CONTROLS
        .add(b' ').add(b'!').add(b'"').add(b'#').add(b'$').add(b'%').add(b'&')
        .add(b'\'').add(b'(').add(b')').add(b'*').add(b'+').add(b',').add(b'/')
        .add(b':').add(b';').add(b'<').add(b'=').add(b'>').add(b'?').add(b'@')
        .add(b'[').add(b'\\').add(b']').add(b'^').add(b'`').add(b'{').add(b'|')
        .add(b'}');
    utf8_percent_encode(s, ENCODE_SET).to_string()
}
