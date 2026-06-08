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

struct Translation;

impl Guest for Translation {
    fn call(tool: String, input_json: String) -> Result<String, String> {
        let input: Value = serde_json::from_str(&input_json)
            .map_err(|e| format!("invalid input JSON: {e}"))?;

        match tool.as_str() {
            "translate" => translate(&input),
            "detect_language" => detect_language(&input),
            "list_languages" => list_languages(),
            _ => Err(format!("unknown tool: {tool}")),
        }
    }
}

export!(Translation);

fn get_str<'a>(input: &'a Value, key: &str) -> Result<&'a str, String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("{key} is required"))
}

fn get_str_opt<'a>(input: &'a Value, key: &str) -> Option<&'a str> {
    input.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
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

fn http_get(url: &str) -> Result<String, String> {
    let req = HttpRequest {
        method: "GET",
        url: url.to_string(),
        headers: serde_json::Map::new(),
        body: None,
    };
    let req_json =
        serde_json::to_string(&req).map_err(|e| format!("failed to serialize request: {e}"))?;
    let resp_json = host::http_request(&req_json)?;
    let resp: HttpResponse =
        serde_json::from_str(&resp_json).map_err(|e| format!("failed to parse HTTP response: {e}"))?;
    if resp.status >= 400 {
        return Err(format!("Request failed with HTTP {}: {}", resp.status, resp.body));
    }
    Ok(resp.body)
}

fn http_post_json(url: &str, body: &str) -> Result<String, String> {
    let mut headers = serde_json::Map::new();
    headers.insert(
        "Content-Type".to_string(),
        Value::String("application/json".to_string()),
    );
    let req = HttpRequest {
        method: "POST",
        url: url.to_string(),
        headers,
        body: Some(body.to_string()),
    };
    let req_json =
        serde_json::to_string(&req).map_err(|e| format!("failed to serialize request: {e}"))?;
    let resp_json = host::http_request(&req_json)?;
    let resp: HttpResponse =
        serde_json::from_str(&resp_json).map_err(|e| format!("failed to parse HTTP response: {e}"))?;
    if resp.status >= 400 {
        return Err(format!("Translation failed with HTTP {}: {}", resp.status, resp.body));
    }
    Ok(resp.body)
}

const TRANSLATE_BASE: &str = "https://translate.argosopentech.com";

fn translate(input: &Value) -> Result<String, String> {
    let text = get_str(input, "text")?;
    let target = get_str(input, "target")?;
    let source = get_str_opt(input, "source").unwrap_or("auto");

    let body_json = serde_json::json!({
        "q": text,
        "source": source,
        "target": target,
        "format": "text",
    });
    let body_str = serde_json::to_string(&body_json)
        .map_err(|e| format!("failed to serialize request body: {e}"))?;

    let resp = http_post_json(&format!("{TRANSLATE_BASE}/translate"), &body_str)?;
    let data: Value = serde_json::from_str(&resp)
        .map_err(|_| format!("Translation service returned an unexpected response. The service may be temporarily unavailable."))?;

    if let Some(error) = data.get("error").and_then(Value::as_str) {
        return Err(format!("translation error: {error}"));
    }

    let translated = data.get("translatedText")
        .and_then(Value::as_str)
        .ok_or("no translation in response")?;

    let detected = data.get("detectedLanguage")
        .and_then(|l| l.get("language"))
        .and_then(Value::as_str);

    let mut out = String::new();
    if let Some(d) = detected {
        out.push_str(&format!("[{} → {}] ", d, target));
    } else {
        out.push_str(&format!("[→ {}] ", target));
    }
    out.push_str(translated);

    Ok(out)
}

fn detect_language(input: &Value) -> Result<String, String> {
    let text = get_str(input, "text")?;

    let body_json = serde_json::json!({
        "q": text,
    });
    let body_str = serde_json::to_string(&body_json)
        .map_err(|e| format!("failed to serialize request body: {e}"))?;

    let resp = http_post_json(&format!("{TRANSLATE_BASE}/detect"), &body_str)?;
    let data: Value = serde_json::from_str(&resp)
        .map_err(|_| "Detection service returned an unexpected response.".to_string())?;

    let results = data.as_array().ok_or("unexpected detection response format")?;

    if results.is_empty() {
        return Ok("Could not detect language.".to_string());
    }

    let best = &results[0];
    let lang = best.get("language").and_then(Value::as_str).unwrap_or("unknown");
    let confidence = best.get("confidence").and_then(Value::as_f64).unwrap_or(0.0);

    let language_name = language_name(lang);
    Ok(format!("Detected: {language_name} ({lang}) — confidence: {:.0}%", confidence * 100.0))
}

fn list_languages() -> Result<String, String> {
    let resp = http_get(&format!("{TRANSLATE_BASE}/languages"))?;
    let langs: Vec<Value> = serde_json::from_str(&resp)
        .map_err(|_| "Failed to fetch language list.".to_string())?;

    let mut out = String::from("Available languages:\n");
    for lang in &langs {
        let code = lang.get("code").and_then(Value::as_str).unwrap_or("");
        let name = lang.get("name").and_then(Value::as_str).unwrap_or("");
        out.push_str(&format!("  {code} — {name}\n"));
    }

    Ok(out)
}

fn language_name(code: &str) -> &str {
    match code {
        "en" => "English", "es" => "Spanish", "fr" => "French",
        "de" => "German", "it" => "Italian", "pt" => "Portuguese",
        "nl" => "Dutch", "ru" => "Russian", "ja" => "Japanese",
        "zh" => "Chinese", "ko" => "Korean", "ar" => "Arabic",
        "hi" => "Hindi", "tr" => "Turkish", "pl" => "Polish",
        "uk" => "Ukrainian", "sv" => "Swedish", "da" => "Danish",
        "fi" => "Finnish", "no" => "Norwegian", "cs" => "Czech",
        "el" => "Greek", "he" => "Hebrew", "th" => "Thai",
        "vi" => "Vietnamese", "id" => "Indonesian", "ms" => "Malay",
        "fa" => "Persian", "ro" => "Romanian", "hu" => "Hungarian",
        "sk" => "Slovak", "bg" => "Bulgarian", "ca" => "Catalan",
        "lt" => "Lithuanian", "lv" => "Latvian", "et" => "Estonian",
        "sl" => "Slovenian", "hr" => "Croatian", "sr" => "Serbian",
        _ => code,
    }
}
