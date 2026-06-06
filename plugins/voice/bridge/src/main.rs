mod adapters;
mod core;

use anyhow::Context;
use axum::{
    Router,
    extract::{Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::IntoResponse,
    routing::{get, post},
};
use serde::Deserialize;
use std::sync::Arc;

use adapters::{SttBackend, TtsBackend};

#[derive(Clone)]
struct AppState {
    helpcore_url: String,
    stt: SttBackend,
    tts: TtsBackend,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8765);

    let helpcore_url = std::env::var("HELPCORE_URL")
        .context("HELPCORE_URL env var is required")?
        .trim_end_matches('/')
        .to_string();

    let stt = SttBackend::from_env()?;
    let tts = TtsBackend::from_env()?;

    let state = Arc::new(AppState { helpcore_url, stt, tts });

    let app = Router::new()
        .route("/health", get(health))
        .route("/voice", post(voice))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}"))
        .await
        .context("failed to bind")?;
    tracing::info!("voice bridge listening on {}", listener.local_addr()?);

    axum::serve(listener, app).await?;
    Ok(())
}

async fn health() -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({ "status": "ok" }))
}

#[derive(Deserialize)]
struct VoiceQuery {
    conversation_id: Option<String>,
}

async fn voice(
    State(state): State<Arc<AppState>>,
    Query(query): Query<VoiceQuery>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .ok_or_else(|| (StatusCode::UNAUTHORIZED, "missing bearer token".into()))?
        .to_string();

    let text = state
        .stt
        .transcribe(body)
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("STT: {e}")))?;

    tracing::debug!(text = %text, "transcribed");

    if text.trim().is_empty() {
        return Err((StatusCode::UNPROCESSABLE_ENTITY, "no speech detected".into()));
    }

    let (reply, conv_id) = core::chat(
        &state.helpcore_url,
        &token,
        &text,
        query.conversation_id.as_deref(),
    )
    .await
    .map_err(|e| (StatusCode::BAD_GATEWAY, format!("chat: {e}")))?;

    tracing::debug!(reply = %reply, "ai reply");

    let audio = state
        .tts
        .synthesize(&reply)
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("TTS: {e}")))?;

    let mut resp_headers = HeaderMap::new();
    resp_headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("audio/wav"));
    if let Some(id) = conv_id {
        if let Ok(val) = HeaderValue::from_str(&id) {
            resp_headers.insert("x-conversation-id", val);
        }
    }

    Ok((resp_headers, audio))
}
