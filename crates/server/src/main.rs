use axum::{
    extract::{ws::Message, Query, State, WebSocketUpgrade},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tower_http::cors::CorsLayer;

// ── Shared state ──

#[derive(Clone)]
struct AppState {
    asr_engine: String, // "whisper", "sensevoice", or "paraformer-streaming"
}

// ── API types ──

#[derive(Deserialize)]
struct TranscribeQuery {
    engine: Option<String>, // "whisper", "sensevoice", or "paraformer-streaming" (default: sensevoice)
    language: Option<String>, // "auto" (default), "zh", "en", ...
}

#[derive(Serialize)]
struct TranscribeResponse {
    text: String,
    duration_ms: u64,
    rtf: f64,
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
        asr_engine: state.asr_engine.clone(),
        vad_model: vad_path,
    })
}

async fn transcribe(Query(query): Query<TranscribeQuery>, body: bytes::Bytes) -> impl IntoResponse {
    let engine = query.engine.as_deref().unwrap_or("sensevoice");
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
            Json(TranscribeResponse {
                text: "No audio data".into(),
                duration_ms: 0,
                rtf: 0.0,
            }),
        );
    }

    let duration_ms = (samples.len() as f64 / 16.0) as u64; // 16kHz → ms
    let start = std::time::Instant::now();

    let text = {
        let category = octopus_asr::config::resolve_engine_category(engine);
        match category {
            Some(octopus_asr::config::EngineCategory::Whisper) => {
                octopus_asr::whisper::transcribe(&samples, language)
            }
            Some(octopus_asr::config::EngineCategory::Paraformer) => {
                octopus_asr::paraformer::transcribe(&samples, language)
            }
            Some(octopus_asr::config::EngineCategory::Qwen3Asr) => {
                octopus_asr::qwen3_asr::transcribe(&samples, language)
            }
            Some(octopus_asr::config::EngineCategory::Zipformer) => {
                octopus_asr::zipformer::transcribe(&samples, language)
            }
            Some(octopus_asr::config::EngineCategory::SenseVoice) | None => {
                octopus_asr::sensevoice::transcribe(&samples, language)
            }
        }
    };

    let elapsed = start.elapsed();
    let rtf = duration_ms as f64 / elapsed.as_millis() as f64;

    match text {
        Ok(t) => (
            axum::http::StatusCode::OK,
            Json(TranscribeResponse {
                text: t,
                duration_ms,
                rtf,
            }),
        ),
        Err(e) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(TranscribeResponse {
                text: format!("Error: {}", e),
                duration_ms,
                rtf,
            }),
        ),
    }
}

async fn ws_stream(ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.on_upgrade(handle_ws)
}

async fn handle_ws(mut socket: axum::extract::ws::WebSocket) {
    use futures_util::StreamExt;

    let mut audio_buffer: Vec<f32> = Vec::new();
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

    while let Some(msg) = socket.next().await {
        match msg {
            Ok(Message::Binary(data)) => {
                // Expect f32 PCM little-endian chunks
                let chunk: Vec<f32> = data
                    .chunks_exact(4)
                    .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                    .collect();
                audio_buffer.extend_from_slice(&chunk);

                // When we have enough audio (~1s = 16000 samples), run VAD + ASR
                if audio_buffer.len() >= 16000 {
                    let speech =
                        octopus_asr::audio::filter_speech(&audio_buffer, &mut vad, 480, 0.5);
                    if !speech.is_empty() {
                        let text = octopus_asr::sensevoice::transcribe(&speech, "auto")
                            .unwrap_or_else(|e| format!("[error: {}]", e));
                        let _ = socket
                            .send(Message::Text(
                                format!(
                                    "{{\"text\": \"{}\", \"final\": true}}",
                                    text.replace('"', "\\\"")
                                )
                                .into(),
                            ))
                            .await;
                    }
                    audio_buffer.clear();
                    vad.reset();
                }
            }
            Ok(Message::Text(cmd)) => {
                if cmd == "flush" && !audio_buffer.is_empty() {
                    let text = octopus_asr::sensevoice::transcribe(&audio_buffer, "auto")
                        .unwrap_or_else(|e| format!("[error: {}]", e));
                    let _ = socket
                        .send(Message::Text(
                            format!(
                                "{{\"text\": \"{}\", \"final\": true}}",
                                text.replace('"', "\\\"")
                            )
                            .into(),
                        ))
                        .await;
                    audio_buffer.clear();
                    vad.reset();
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

    let config = octopus_asr::config::load_config()?;
    let asr_engine = config.asr.active.clone();

    let state = AppState { asr_engine };

    let app = Router::new()
        .route("/health", get(health))
        .route("/models", get(models))
        .route("/transcribe", post(transcribe))
        .route("/ws/stream", get(ws_stream))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let addr = "0.0.0.0:3000";
    let listener = TcpListener::bind(addr).await?;
    tracing::info!("octopus-server listening on {}", addr);
    axum::serve(listener, app).await?;

    Ok(())
}
