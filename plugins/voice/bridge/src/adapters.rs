use anyhow::Context;
use bytes::Bytes;
use reqwest::Client;

// ── STT ────────────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub enum SttBackend {
    /// OpenAI-compatible POST /v1/audio/transcriptions → {"text": "..."}.
    /// Covers: speaches (ghcr.io/speaches-ai/speaches), real OpenAI.
    OpenAiCompat {
        url: String,
        api_key: Option<String>,
        model: String,
    },
}

impl SttBackend {
    pub fn from_env() -> anyhow::Result<Self> {
        let backend = std::env::var("STT_BACKEND").unwrap_or_else(|_| "openai_compat".into());
        match backend.as_str() {
            "openai_compat" => Ok(Self::OpenAiCompat {
                url: std::env::var("STT_URL")
                    .unwrap_or_else(|_| "http://localhost:8000".into()),
                api_key: std::env::var("STT_API_KEY").ok(),
                model: std::env::var("STT_MODEL")
                    .unwrap_or_else(|_| "Systran/faster-whisper-small".into()),
            }),
            other => anyhow::bail!("unknown STT_BACKEND: {other}"),
        }
    }

    pub async fn transcribe(&self, audio: Bytes) -> anyhow::Result<String> {
        match self {
            Self::OpenAiCompat { url, api_key, model } => {
                let part = reqwest::multipart::Part::bytes(audio.to_vec())
                    .file_name("audio.wav")
                    .mime_str("audio/wav")?;
                let form = reqwest::multipart::Form::new()
                    .part("file", part)
                    .text("model", model.clone());

                let mut req = Client::new()
                    .post(format!("{url}/v1/audio/transcriptions"))
                    .multipart(form);
                if let Some(key) = api_key {
                    req = req.bearer_auth(key);
                }

                let resp: serde_json::Value = req
                    .send()
                    .await
                    .context("STT request failed")?
                    .error_for_status()
                    .context("STT returned error")?
                    .json()
                    .await
                    .context("STT response was not JSON")?;

                resp.get("text")
                    .and_then(|v| v.as_str())
                    .map(|s| s.trim().to_string())
                    .context("STT response missing 'text' field")
            }
        }
    }
}

// ── TTS ────────────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub enum TtsBackend {
    /// OpenAI-compatible POST /v1/audio/speech → audio bytes.
    /// Covers: speaches (ghcr.io/speaches-ai/speaches), real OpenAI.
    OpenAiCompat {
        url: String,
        api_key: Option<String>,
        model: String,
        voice: String,
    },
    /// GET /?text=<encoded> → WAV.
    /// Image: artibex/piper-http:latest, port 5000.
    PiperHttp { url: String },
}

impl TtsBackend {
    pub fn from_env() -> anyhow::Result<Self> {
        let backend = std::env::var("TTS_BACKEND").unwrap_or_else(|_| "openai_compat".into());
        match backend.as_str() {
            "openai_compat" => Ok(Self::OpenAiCompat {
                url: std::env::var("TTS_URL")
                    .unwrap_or_else(|_| "http://localhost:8000".into()),
                api_key: std::env::var("TTS_API_KEY").ok(),
                model: std::env::var("TTS_MODEL").unwrap_or_else(|_| "piper".into()),
                voice: std::env::var("TTS_VOICE")
                    .unwrap_or_else(|_| "en_US-lessac-medium".into()),
            }),
            "piper_http" => Ok(Self::PiperHttp {
                url: std::env::var("TTS_URL")
                    .unwrap_or_else(|_| "http://localhost:5000".into()),
            }),
            other => anyhow::bail!("unknown TTS_BACKEND: {other}"),
        }
    }

    pub async fn synthesize(&self, text: &str) -> anyhow::Result<Bytes> {
        match self {
            Self::OpenAiCompat { url, api_key, model, voice } => {
                let mut req = Client::new()
                    .post(format!("{url}/v1/audio/speech"))
                    .json(&serde_json::json!({
                        "model": model,
                        "input": text,
                        "voice": voice,
                    }));
                if let Some(key) = api_key {
                    req = req.bearer_auth(key);
                }
                req.send()
                    .await
                    .context("TTS request failed")?
                    .error_for_status()
                    .context("TTS returned error")?
                    .bytes()
                    .await
                    .context("TTS response body failed")
            }
            Self::PiperHttp { url } => {
                let encoded = urlencoding::encode(text);
                Client::new()
                    .get(format!("{url}/?text={encoded}"))
                    .send()
                    .await
                    .context("piper-http request failed")?
                    .error_for_status()
                    .context("piper-http returned error")?
                    .bytes()
                    .await
                    .context("piper-http response body failed")
            }
        }
    }
}
