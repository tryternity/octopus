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
use octopus_asr::engine::AsrEngineManager;

// ── CLI args ──

#[derive(Parser)]
#[command(name = "octopus-server", about = "ASR inference HTTP/WebSocket server", version)]
struct Cli {
    /// Listen port
    #[arg(long, env = "OCTOPUS_PORT", default_value = "3000")]
    port: u16,
    /// Listen address
    #[arg(long, env = "OCTOPUS_HOST", default_value = "0.0.0.0")]
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
    let vad_path = octopus_asr::config::find_silero_vad()
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
    let engine = query.engine.as_deref().unwrap_or(&state.active_model);
    let language = query.language.as_deref().unwrap_or("auto");

    // Try to parse as WAV, fallback to raw f32
    let samples = match octopus_asr::audio::read_wav_16k_from_bytes(&body) {
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

    let text = state.engine_manager.switch_model(engine)
        .and_then(|_| state.engine_manager.transcribe(&samples, language));

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

fn detect_silence_gap_local(
    vad: &mut octopus_asr::vad::SileroVad,
    samples: &[f32],
    silence_duration: &mut f64,
) -> bool {
    let prev_silence = *silence_duration;
    let mut speech_chunks = 0usize;
    let mut silent_chunks = 0usize;

    const VAD_CHUNK_SIZE: usize = 512;
    const VAD_SPEECH_THRESHOLD: f32 = 0.5;

    for chunk in samples.chunks(VAD_CHUNK_SIZE) {
        if chunk.len() < VAD_CHUNK_SIZE {
            break;
        }
        match vad.compute(chunk) {
            Ok(prob) => {
                if prob >= VAD_SPEECH_THRESHOLD {
                    speech_chunks += 1;
                } else {
                    silent_chunks += 1;
                }
            }
            Err(_) => {
                speech_chunks += 1;
            }
        }
    }

    let total_chunks = speech_chunks + silent_chunks;
    if total_chunks == 0 {
        return false;
    }

    let chunk_duration = VAD_CHUNK_SIZE as f64 / 16000.0;

    if speech_chunks >= 2 {
        *silence_duration = 0.0;
    } else {
        *silence_duration += total_chunks as f64 * chunk_duration;
    }

    prev_silence >= 0.5
}

async fn handle_ws(
    mut socket: axum::extract::ws::WebSocket,
    _engine_manager: Arc<AsrEngineManager>,
    engine: String,
    _language: String,
) {
    use futures_util::StreamExt;

    // Validate engine
    if octopus_asr::config::resolve_engine_category(&engine).is_none() {
        let _ = socket
            .send(Message::Text(
                format!("{{\"error\": \"Unknown engine '{}'\"}}", engine).into(),
            ))
            .await;
        return;
    }

    let streaming_session = match octopus_asr::streaming_engine::StreamingSession::new(&engine) {
        Ok(sess) => sess,
        Err(e) => {
            let _ = socket
                .send(Message::Text(
                    format!("{{\"error\": \"Failed to create streaming session: {}\"}}", e).into(),
                ))
                .await;
            return;
        }
    };

    let vad_path = match octopus_asr::config::find_silero_vad() {
        Ok(p) => p,
        Err(e) => {
            let _ = socket
                .send(Message::Text(
                    format!("{{\"error\": \"VAD: {}\"}}", e).into(),
                ))
                .await;
            return;
        }
    };
    let mut vad = match octopus_asr::vad::SileroVad::new(&vad_path) {
        Ok(v) => v,
        Err(e) => {
            let _ = socket
                .send(Message::Text(
                    format!("{{\"error\": \"VAD init: {}\"}}", e).into(),
                ))
                .await;
            return;
        }
    };

    let mut silence_duration = 0.0f64;
    let mut flushed = false;

    while let Some(msg) = socket.next().await {
        match msg {
            Ok(Message::Binary(data)) => {
                // Expect f32 PCM little-endian chunks
                let chunk: Vec<f32> = data
                    .chunks_exact(4)
                    .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                    .collect();
                if chunk.is_empty() {
                    continue;
                }

                let was_silent = detect_silence_gap_local(&mut vad, &chunk, &mut silence_duration);
                if silence_duration == 0.0 {
                    flushed = false;
                }

                match streaming_session.accept_samples(&chunk, was_silent) {
                    Ok(Some(new_text)) => {
                        let _ = socket
                            .send(Message::Text(
                                format!(
                                    "{{\"text\": \"{}\", \"final\": false}}",
                                    new_text.replace('"', "\\\"")
                                )
                                .into(),
                            ))
                            .await;
                    }
                    Ok(None) => {}
                    Err(e) => {
                        let _ = socket
                            .send(Message::Text(
                                format!("{{\"error\": \"Streaming ASR error: {}\"}}", e).into(),
                            ))
                            .await;
                    }
                }

                // Silent flush (> 0.5s)
                if silence_duration >= 0.5 && !flushed {
                    match streaming_session.flush(true) {
                        Ok(Some(new_text)) => {
                            let _ = socket
                                .send(Message::Text(
                                    format!(
                                        "{{\"text\": \"{}\", \"final\": false}}",
                                        new_text.replace('"', "\\\"")
                                    )
                                    .into(),
                                ))
                                .await;
                            flushed = true;
                        }
                        _ => {}
                    }
                }
            }
            Ok(Message::Text(cmd)) => {
                if cmd == "flush" {
                    match streaming_session.finish() {
                        Ok(final_text) => {
                            let _ = socket
                                .send(Message::Text(
                                    format!(
                                        "{{\"text\": \"{}\", \"final\": true}}",
                                        final_text.replace('"', "\\\"")
                                    )
                                    .into(),
                                ))
                                .await;
                        }
                        Err(e) => {
                            let _ = socket
                                .send(Message::Text(
                                    format!("{{\"error\": \"Streaming ASR finish error: {}\"}}", e).into(),
                                ))
                                .await;
                        }
                    }
                    streaming_session.reset();
                    silence_duration = 0.0;
                    flushed = false;
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

    // 全局默认引擎：以 config.yaml.asr_engine 为准（DB name 精确匹配），
    // 空/匹配不到 → 回退兜底 zipformer-small-ctc（见 asr::config::resolve_active_engine）。
    let app_cfg = octopus_infra::config::load_config()?;
    let active_model = octopus_asr::config::resolve_active_engine(&app_cfg.asr_engine)?.name;

    let engine_manager = Arc::new(AsrEngineManager::new());
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
        .layer(CorsLayer::permissive())
        .with_state(state);

    let addr = format!("{}:{}", cli.host, cli.port);
    let listener = TcpListener::bind(&addr).await?;
    tracing::info!("octopus-server listening on {}", addr);
    axum::serve(listener, app).await?;

    Ok(())
}
