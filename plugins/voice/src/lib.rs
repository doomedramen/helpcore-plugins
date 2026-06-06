use serde::{Deserialize, Serialize};
use serde_json::Value;

wit_bindgen::generate!({
    inline: r#"
        package helpcore:plugin;

        interface host {
            http-request: func(request-json: string) -> result<string, string>;
            data-read: func(path: string) -> result<string, string>;
            data-write: func(path: string, content: string) -> result<_, string>;
            config-read: func(key: string) -> result<string, string>;
        }

        world plugin {
            import host;
            export call: func(tool: string, input-json: string) -> result<string, string>;
        }
    "#,
    world: "plugin",
});

use helpcore::plugin::host;

struct Voice;

impl Guest for Voice {
    fn call(tool: String, input_json: String) -> Result<String, String> {
        let input: Value = serde_json::from_str(&input_json)
            .map_err(|e| format!("invalid input JSON: {e}"))?;

        match tool.as_str() {
            "transcribe_audio" => transcribe_audio(&input),
            "synthesize_speech" => synthesize_speech(&input),
            _ => Err(format!("unknown tool: {tool}")),
        }
    }
}

export!(Voice);

fn url_encode(s: &str) -> String {
    let mut result = String::new();
    for byte in s.bytes() {
        match byte {
            b'0'..=b'9' | b'A'..=b'Z' | b'a'..=b'z' | b'-' | b'_' | b'.' | b'~' => {
                result.push(byte as char);
            }
            b' ' => result.push_str("%20"),
            _ => result.push_str(&format!("%{:02X}", byte)),
        }
    }
    result
}

#[derive(Serialize)]
struct HttpRequest<'a> {
    method: &'a str,
    url: String,
    headers: serde_json::Map<String, Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    body: Option<String>,
}

#[derive(Deserialize)]
struct HttpResponse {
    status: u16,
    body: String,
}

fn send_http(method: &str, url: &str, headers: serde_json::Map<String, Value>, body: Option<String>) -> Result<String, String> {
    let req = HttpRequest { method, url: url.to_string(), headers, body };
    let req_json = serde_json::to_string(&req).map_err(|e| e.to_string())?;
    let resp_json = host::http_request(&req_json)?;
    let resp: HttpResponse = serde_json::from_str(&resp_json)
        .map_err(|e| format!("failed to parse HTTP response: {e}"))?;
    if resp.status >= 400 {
        return Err(format!("API returned HTTP {}: {}", resp.status, resp.body));
    }
    Ok(resp.body)
}

fn json_headers(api_key: Option<&str>) -> serde_json::Map<String, Value> {
    let mut h = serde_json::Map::new();
    h.insert("Content-Type".into(), Value::String("application/json".into()));
    if let Some(key) = api_key {
        h.insert("Authorization".into(), Value::String(format!("Bearer {key}")));
    }
    h
}

fn transcribe_audio(input: &Value) -> Result<String, String> {
    let audio_url = input
        .get("audio_url")
        .and_then(Value::as_str)
        .ok_or("audio_url is required")?;

    let language = input.get("language").and_then(Value::as_str);

    let stt_url = host::config_read("stt_url")
        .map_err(|_| "STT URL not configured".to_string())?;
    let api_key = host::config_read("stt_api_key").ok();
    let model = host::config_read("stt_model").unwrap_or_else(|_| "whisper-1".into());

    let url_lower = stt_url.to_lowercase();
    let api_key = api_key.as_deref();

    let (method, url, headers, body): (&str, String, serde_json::Map<String, Value>, Option<String>) =
        if url_lower.contains("googleapis") {
            let mut config = serde_json::json!({
                "encoding": "LINEAR16",
                "model": model,
            });
            if let Some(lang) = language {
                config["languageCode"] = Value::String(lang.to_string());
            }
            (
                "POST",
                format!("{}/v1/speech:recognize", stt_url.trim_end_matches('/')),
                json_headers(api_key),
                Some(
                    serde_json::json!({
                        "audio": { "uri": audio_url },
                        "config": config,
                    })
                    .to_string(),
                ),
            )
        } else if url_lower.contains("deepgram") {
            let mut u = format!(
                "{}/v1/listen?smart_format=true&url={}",
                stt_url.trim_end_matches('/'),
                url_encode(audio_url)
            );
            if let Some(lang) = language {
                u.push_str(&format!("&language={}", url_encode(lang)));
            }
            let mut h = serde_json::Map::new();
            if let Some(key) = api_key {
                h.insert("Authorization".into(), Value::String(format!("Token {key}")));
            }
            ("POST", u, h, None)
        } else {
            let mut req_body = serde_json::json!({ "audio_url": audio_url });
            if let Some(lang) = language {
                req_body["language"] = Value::String(lang.to_string());
            }
            ("POST", stt_url, json_headers(api_key), Some(req_body.to_string()))
        };

    let resp_body = send_http(method, &url, headers, body)?;

    let v: Value = serde_json::from_str(&resp_body)
        .map_err(|_| "STT response was not valid JSON".to_string())?;

    let text = if url_lower.contains("googleapis") {
        v.pointer("/results/0/alternatives/0/transcript")
            .and_then(Value::as_str)
    } else if url_lower.contains("deepgram") {
        v.pointer("/results/channels/0/alternatives/0/transcript")
            .and_then(Value::as_str)
    } else {
        v.get("text")
            .or_else(|| v.get("transcript"))
            .and_then(Value::as_str)
    };

    text.map(|s| s.trim().to_string())
        .ok_or_else(|| "STT response missing transcription text".to_string())
}

fn synthesize_speech(input: &Value) -> Result<String, String> {
    let text = input
        .get("text")
        .and_then(Value::as_str)
        .ok_or("text is required")?;

    let tts_url = host::config_read("tts_url")
        .map_err(|_| "TTS URL not configured".to_string())?;
    let api_key = host::config_read("tts_api_key").ok();
    let model = host::config_read("tts_model").unwrap_or_else(|_| "tts-1".into());
    let voice = host::config_read("tts_voice").unwrap_or_else(|_| "alloy".into());

    let url_lower = tts_url.to_lowercase();
    let api_key = api_key.as_deref();

    if url_lower.contains("googleapis") {
        let body = serde_json::json!({
            "input": { "text": text },
            "voice": { "languageCode": "en-US", "name": voice },
            "audioConfig": { "audioEncoding": "MP3" },
        })
        .to_string();

        let resp_body = send_http(
            "POST",
            &format!("{}/v1/text:synthesize", tts_url.trim_end_matches('/')),
            json_headers(api_key),
            Some(body),
        )?;

        let v: Value = serde_json::from_str(&resp_body)
            .map_err(|_| "TTS response was not valid JSON".to_string())?;

        let b64 = v
            .get("audioContent")
            .and_then(Value::as_str)
            .ok_or("TTS response missing audioContent")?;

        Ok(format!("data:audio/mp3;base64,{b64}"))
    } else {
        let body = serde_json::json!({
            "model": model,
            "input": text,
            "voice": voice,
        })
        .to_string();

        let resp_body = send_http(
            "POST",
            &format!("{}/v1/audio/speech", tts_url.trim_end_matches('/')),
            json_headers(api_key),
            Some(body),
        )?;

        let v: Value = serde_json::from_str(&resp_body).map_err(|_| {
            "TTS returned binary audio instead of JSON. \
             This plugin requires a TTS provider that returns JSON with a base64-encoded \
             audio content field (e.g. Google Cloud TTS). \
             For OpenAI-compatible TTS providers that return raw binary audio, \
             use the voice-bridge instead."
        })?;

        let b64 = v
            .get("audioContent")
            .or_else(|| v.get("audio"))
            .and_then(Value::as_str)
            .ok_or("TTS response JSON does not contain 'audioContent' or 'audio' field")?;

        Ok(format!("data:audio/mpeg;base64,{b64}"))
    }
}
