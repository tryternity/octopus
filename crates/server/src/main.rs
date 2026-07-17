mod pipeline;

use axum::{
    extract::{ws::Message, Query, State, WebSocketUpgrade},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use clap::Parser;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::net::TcpListener;
use tower_http::cors::CorsLayer;
use octopus_asr_local::engine::AsrEngineManager;
use octopus_asr_local::streaming_runner::TranscriptEvent;
use pipeline::{event_to_json, WsStreamSession};

// ── CLI args ──

#[derive(Parser)]
#[command(name = "octopus-server", about = "ASR inference HTTP/WebSocket server", version)]
struct Cli {
    /// Listen port
    #[arg(long, env = "OCTOPUS_PORT", default_value = "3000")]
    port: u16,
    /// Listen address
    #[arg(long, env = "OCTOPUS_HOST", default_value = "127.0.0.1")]
    host: String,
}

// ── Shared state ──

#[derive(Clone)]
struct AppState {
    engine_manager: Arc<AsrEngineManager>,
    active_model: String,
}

// ── API types ──

#[derive(Deserialize)]
struct TranscribeQuery {
    /// ASR engine name (from DB models table; default: sensevoice)
    engine: Option<String>,
    /// Language: "auto" (default), "zh", "en", "ja", ...
    language: Option<String>,
}

#[derive(Serialize)]
struct TranscribeResponse {
    text: String,
    duration_ms: u64,
    rtf: f64,
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
}

#[derive(Serialize)]
struct ModelsResponse {
    asr_engine: String,
    vad_model: String,
}

#[derive(Serialize)]
struct HealthResponse {
    status: String,
}

// ── Routes ──

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".into(),
    })
}

async fn models(State(state): State<AppState>) -> Json<ModelsResponse> {
    let vad_path = octopus_asr_local::config::find_silero_vad()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|e| format!("error: {}", e));
    Json(ModelsResponse {
        asr_engine: state.active_model.clone(),
        vad_model: vad_path,
    })
}

async fn transcribe(
    State(state): State<AppState>,
    Query(query): Query<TranscribeQuery>,
    body: bytes::Bytes,
) -> impl IntoResponse {
    let engine = query.engine.as_deref().unwrap_or(&state.active_model).to_string();
    let language = query.language.as_deref().unwrap_or("auto");

    // Try to parse as WAV, fallback to raw f32
    let samples = match octopus_asr_local::audio::read_wav_16k_from_bytes(&body) {
        Ok(s) => s,
        Err(_) => {
            // Try raw f32 bytes
            let len = body.len() / 4;
            let mut samples = Vec::with_capacity(len);
            for chunk in body.chunks_exact(4) {
                samples.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
            }
            samples
        }
    };

    if samples.is_empty() {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "No audio data received. Send WAV file or raw f32 PCM (16kHz LE)".into(),
            }),
        )
            .into_response();
    }

    let duration_ms = (samples.len() as f64 / 16.0) as u64; // 16kHz → ms
    let start = std::time::Instant::now();

    let cfg = octopus_asr_local::pipeline::PipelineConfig::from_app_config(language);
    // get_engine 取 Arc（不改全局 active）：同模型并发受引擎内 Mutex<Session> 串行化、
    // 跨模型天然并行——不再需要全局 inference_lock 串行化所有 batch 请求。
    let engine_arc = match state.engine_manager.get_engine(&engine) {
        Ok(e) => e,
        Err(e) => {
            return (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: format!("load engine '{}': {}", engine, e),
                }),
            )
                .into_response();
        }
    };
    let text = match tokio::task::spawn_blocking(move || {
        octopus_asr_local::pipeline::transcribe_batch(engine_arc.as_ref(), &samples, &cfg)
    })
    .await
    {
        Ok(result) => result,
        Err(e) => {
            return (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: format!("inference task failed: {}", e),
                }),
            )
                .into_response();
        }
    };

    let elapsed = start.elapsed();
    let rtf = if elapsed.as_millis() > 0 {
        duration_ms as f64 / elapsed.as_millis() as f64
    } else {
        0.0
    };

    match text {
        Ok(t) => (
            axum::http::StatusCode::OK,
            Json(TranscribeResponse {
                text: t,
                duration_ms,
                rtf,
            }),
        )
            .into_response(),
        Err(e) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: format!("ASR inference failed: {}", e),
            }),
        )
            .into_response(),
    }
}

// ── WebSocket ──

#[derive(Deserialize)]
struct WsQuery {
    /// ASR engine name (from DB models table; default: sensevoice)
    engine: Option<String>,
    /// Language: "auto" (default), "zh", "en", "ja", ...
    language: Option<String>,
}

async fn ws_stream(
    State(state): State<AppState>,
    ws: WebSocketUpgrade,
    Query(query): Query<WsQuery>,
) -> impl IntoResponse {
    let engine = query
        .engine
        .unwrap_or_else(|| state.active_model.clone());
    let language = query
        .language
        .unwrap_or_else(|| "auto".to_string());
    ws.on_upgrade(move |socket| handle_ws(socket, state.engine_manager, engine, language))
}

async fn handle_ws(
    mut socket: axum::extract::ws::WebSocket,
    _engine_manager: Arc<AsrEngineManager>,
    engine: String,
    language: String,
) {
    use futures_util::StreamExt;

    // Validate engine
    if octopus_asr_local::config::resolve_engine_category_any(&engine).is_none() {
        let _ = socket
            .send(Message::Text(
                event_to_json(&TranscriptEvent::Error(format!(
                    "Unknown engine '{}'",
                    engine
                )))
                .into(),
            ))
            .await;
        return;
    }

    let session = match octopus_asr_local::streaming_engine::StreamingSession::new(&engine, &language) {
        Ok(s) => s,
        Err(e) => {
            let _ = socket
                .send(Message::Text(
                    event_to_json(&TranscriptEvent::Error(format!(
                        "Failed to create streaming session: {}",
                        e
                    )))
                    .into(),
                ))
                .await;
            return;
        }
    };

    // correct 与批处理 PipelineConfig.correct 同源（app_config.asr_correct）。
    let correct = octopus_asr_local::config::load_app_config_cached().asr_correct;
    let mut stream = match WsStreamSession::new(Arc::new(session), correct) {
        Ok(s) => s,
        Err(e) => {
            let _ = socket
                .send(Message::Text(
                    event_to_json(&TranscriptEvent::Error(format!(
                        "VAD init: {}",
                        e
                    )))
                    .into(),
                ))
                .await;
            return;
        }
    };

    while let Some(msg) = socket.next().await {
        match msg {
            Ok(Message::Binary(data)) => {
                // f32 PCM little-endian chunks
                let chunk: Vec<f32> = data
                    .chunks_exact(4)
                    .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                    .collect();
                if chunk.is_empty() {
                    continue;
                }
                for ev in stream.feed(&chunk) {
                    let _ = socket.send(Message::Text(event_to_json(&ev).into())).await;
                }
            }
            Ok(Message::Text(cmd)) => {
                if cmd == "flush" {
                    let ev = stream.finish();
                    let _ = socket.send(Message::Text(event_to_json(&ev).into())).await;
                    stream.reset();
                }
            }
            Ok(Message::Close(_)) => break,
            Err(_) => break,
            _ => {}
        }
    }
}

// ── Main ──

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();

    // 全局默认引擎：DB models.is_enabled=1 的 ASR 模型（Task 2 后），
    // 无激活 → 回退兜底 zipformer-small-ctc（见 asr::config::resolve_active_engine）。
    let _app_cfg = octopus_infra::config::load_config()?;
    // 启动时加载 ASR 域激活引擎到内存缓存。
    octopus_asr_local::config::load_active_engine("asr")?;
    let active_model = octopus_asr_local::config::resolve_active_engine("asr")?.name;

    // server 多模型并发：缓存上限放大到 8，避免频繁淘汰重载（每引擎数百 MB）。
    let engine_manager = Arc::new(AsrEngineManager::new_with_capacity(8));
    tracing::info!("Preheating active ASR model: {}", active_model);
    if let Err(e) = engine_manager.switch_model(&active_model) {
        tracing::error!("Failed to preheat active ASR model {}: {}", active_model, e);
    }

    let state = AppState {
        engine_manager,
        active_model,
    };

    let app = Router::new()
        .route("/health", get(health))
        .route("/models", get(models))
        .route("/transcribe", post(transcribe))
        .route("/ws/stream", get(ws_stream))
        .layer(axum::extract::DefaultBodyLimit::max(100 * 1024 * 1024)) // 100MB
        .layer(CorsLayer::new()) // 同源策略（本地工具默认不开放 CORS）
        .with_state(state);

    let addr = format!("{}:{}", cli.host, cli.port);
    let listener = TcpListener::bind(&addr).await?;
    tracing::info!("octopus-server listening on {}", addr);
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c().await.expect("install ctrl_c handler");
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install signal handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("server shutting down...");
}
