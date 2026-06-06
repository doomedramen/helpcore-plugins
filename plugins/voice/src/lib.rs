use serde::{Deserialize, Serialize};
use serde_json::Value;

wit_bindgen::generate!({
    inline: r#"
        package helpcore:plugin;

        interface host {
            http-request: func(request-json: string) -> result<string, string>;
            http-request-binary: func(request-json: string) -> result<string, string>;
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

const MAX_CHARS_PER_CHUNK: usize = 2000;

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
    #[serde(skip_serializing_if = "Option::is_none", rename = "body_base64")]
    body_base64: Option<String>,
}

#[derive(Deserialize)]
struct HttpResponse {
    status: u16,
    body: String,
}

fn send_http(req: &HttpRequest) -> Result<String, String> {
    let req_json =
        serde_json::to_string(req).map_err(|e| format!("failed to serialize request: {e}"))?;
    let resp_json = host::http_request(&req_json)?;
    let resp: HttpResponse = serde_json::from_str(&resp_json)
        .map_err(|e| format!("failed to parse HTTP response: {e}"))?;
    if resp.status >= 400 {
        return Err(format!("API returned HTTP {}: {}", resp.status, resp.body));
    }
    Ok(resp.body)
}

fn send_http_binary(req: &HttpRequest) -> Result<String, String> {
    let req_json =
        serde_json::to_string(req).map_err(|e| format!("failed to serialize request: {e}"))?;
    let resp_json = host::http_request_binary(&req_json)?;
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
    let stt_url = stt_url.trim_end_matches('/').to_string();
    let api_key = host::config_read("stt_api_key").ok();
    let model = host::config_read("stt_model").unwrap_or_else(|_| "whisper-1".into());

    let url_lower = stt_url.to_lowercase();
    let api_key = api_key.as_deref();

    let (method, url, headers, body): (&str, String, serde_json::Map<String, Value>, Option<String>) =
        if url_lower.contains("deepgram") {
            let mut u = format!(
                "{}/v1/listen?smart_format=true&model={}&url={}",
                stt_url,
                url_encode(&model),
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

    let resp_body = send_http(&HttpRequest { method, url, headers, body, body_base64: None })?;

    let v: Value = serde_json::from_str(&resp_body)
        .map_err(|_| "STT response was not valid JSON".to_string())?;

    let text = if url_lower.contains("deepgram") {
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

fn split_text(text: &str, max_chars: usize) -> Vec<String> {
    if text.len() <= max_chars {
        return vec![text.to_string()];
    }

    let mut chunks = Vec::new();
    let mut start = 0;

    while start < text.len() {
        if start + max_chars >= text.len() {
            chunks.push(text[start..].to_string());
            break;
        }

        let end = text[start..start + max_chars]
            .rfind(|c| c == '.' || c == '!' || c == '?' || c == '\n')
            .map(|pos| start + pos + 1)
            .or_else(|| {
                text[start..start + max_chars]
                    .rfind(|c: char| c.is_whitespace() && c != '\n')
                    .map(|pos| start + pos + 1)
            })
            .unwrap_or(start + max_chars);

        chunks.push(text[start..end].to_string());
        start = end;
    }

    chunks
}

const B64_CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_encode(data: &[u8]) -> String {
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;

        out.push(B64_CHARS[((triple >> 18) & 0x3F) as usize] as char);
        out.push(B64_CHARS[((triple >> 12) & 0x3F) as usize] as char);

        if chunk.len() > 1 {
            out.push(B64_CHARS[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }

        if chunk.len() > 2 {
            out.push(B64_CHARS[(triple & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

fn b64_char_value(c: u8) -> Option<u32> {
    match c {
        b'A'..=b'Z' => Some((c - b'A') as u32),
        b'a'..=b'z' => Some((c - b'a' + 26) as u32),
        b'0'..=b'9' => Some((c - b'0' + 52) as u32),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

fn base64_decode(input: &str) -> Result<Vec<u8>, String> {
    let trimmed = input.trim_end_matches('=');
    let mut out = Vec::with_capacity(trimmed.len() * 3 / 4);
    let bytes = trimmed.as_bytes();
    let mut i = 0;

    while i + 4 <= bytes.len() {
        let a = b64_char_value(bytes[i]).ok_or("invalid base64 character")?;
        let b = b64_char_value(bytes[i + 1]).ok_or("invalid base64 character")?;
        let c = b64_char_value(bytes[i + 2]).ok_or("invalid base64 character")?;
        let d = b64_char_value(bytes[i + 3]).ok_or("invalid base64 character")?;
        let triple = (a << 18) | (b << 12) | (c << 6) | d;
        out.push((triple >> 16) as u8);
        out.push(((triple >> 8) & 0xFF) as u8);
        out.push((triple & 0xFF) as u8);
        i += 4;
    }

    let remaining = bytes.len() - i;
    if remaining >= 2 {
        let a = b64_char_value(bytes[i]).ok_or("invalid base64 character")?;
        let b = b64_char_value(bytes[i + 1]).ok_or("invalid base64 character")?;
        if remaining == 2 {
            let triple = (a << 18) | (b << 12);
            out.push((triple >> 16) as u8);
        } else {
            let c = b64_char_value(bytes[i + 2]).ok_or("invalid base64 character")?;
            let triple = (a << 18) | (b << 12) | (c << 6);
            out.push((triple >> 16) as u8);
            out.push(((triple >> 8) & 0xFF) as u8);
        }
    }

    Ok(out)
}

fn concatenate_audio(parts: &[Vec<u8>]) -> Result<Vec<u8>, String> {
    if parts.is_empty() {
        return Err("no audio parts to concatenate".to_string());
    }
    if parts.len() == 1 {
        return Ok(parts[0].clone());
    }

    let is_wav = parts[0].starts_with(b"RIFF");

    if is_wav {
        let mut header = parts[0][..44].to_vec();
        let mut data = Vec::new();
        data.extend_from_slice(&parts[0][44..]);
        for chunk in &parts[1..] {
            if chunk.len() <= 44 {
                continue;
            }
            data.extend_from_slice(&chunk[44..]);
        }
        let data_size = data.len() as u32;
        header[4..8].copy_from_slice(&(data_size + 36).to_le_bytes());
        header[40..44].copy_from_slice(&data_size.to_le_bytes());
        let mut result = header;
        result.extend(data);
        Ok(result)
    } else {
        let mut result = Vec::new();
        for part in parts {
            result.extend_from_slice(part);
        }
        Ok(result)
    }
}

const MAX_RESULT_BYTES: usize = 1_000_000;

fn synthesize_chunk(
    tts_url: &str,
    api_key: Option<&str>,
    model: &str,
    voice: &str,
    text: &str,
) -> Result<Vec<u8>, String> {
    let body = serde_json::json!({
        "model": model,
        "input": text,
        "voice": voice,
    })
    .to_string();

    let resp_body = send_http_binary(&HttpRequest {
        method: "POST",
        url: format!("{}/v1/audio/speech", tts_url),
        headers: json_headers(api_key),
        body: Some(body),
        body_base64: None,
    })?;

    base64_decode(&resp_body)
}

fn synthesize_speech(input: &Value) -> Result<String, String> {
    let text = input
        .get("text")
        .and_then(Value::as_str)
        .ok_or("text is required")?;

    let tts_url = host::config_read("tts_url")
        .map_err(|_| "TTS URL not configured".to_string())?;
    let tts_url = tts_url.trim_end_matches('/').to_string();
    let api_key = host::config_read("tts_api_key").ok();
    let model = host::config_read("tts_model").unwrap_or_else(|_| "tts-1".into());
    let voice = host::config_read("tts_voice").unwrap_or_else(|_| "alloy".into());
    let api_key = api_key.as_deref();

    let chunks = split_text(text, MAX_CHARS_PER_CHUNK);

    let mut audio_parts: Vec<Vec<u8>> = Vec::new();
    let mut total_b64_len: usize = 0;
    let overhead = 24;

    for chunk in &chunks {
        let part = synthesize_chunk(&tts_url, api_key, &model, &voice, chunk)?;
        let b64_len = (part.len() + 2) / 3 * 4;
        if total_b64_len + b64_len + overhead > MAX_RESULT_BYTES {
            return Err(
                "Synthesized audio exceeds the 1 MiB result limit. \
                 Try requesting a shorter response."
                    .to_string(),
            );
        }
        total_b64_len += b64_len;
        audio_parts.push(part);
    }

    let combined = concatenate_audio(&audio_parts)?;
    let b64 = base64_encode(&combined);
    Ok(format!("data:audio/mpeg;base64,{b64}"))
}
