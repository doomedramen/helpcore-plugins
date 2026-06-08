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

struct Dictionary;

impl Guest for Dictionary {
    fn call(tool: String, input_json: String) -> Result<String, String> {
        let input: Value = serde_json::from_str(&input_json)
            .map_err(|e| format!("invalid input JSON: {e}"))?;
        match tool.as_str() {
            "define" => define(&input),
            _ => Err(format!("unknown tool: {tool}")),
        }
    }
}

export!(Dictionary);

fn get_str<'a>(input: &'a Value, key: &str) -> Result<&'a str, String> {
    input.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
        .ok_or_else(|| format!("{key} is required"))
}

#[derive(Serialize)]
struct HttpReq<'a> {
    method: &'a str,
    url: String,
    headers: serde_json::Map<String, Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    body: Option<String>,
}

#[derive(Deserialize)]
struct HttpResp {
    status: u16,
    body: String,
}

fn http_get(url: &str) -> Result<String, String> {
    let req = HttpReq {
        method: "GET", url: url.to_string(),
        headers: serde_json::Map::new(), body: None,
    };
    let req_json = serde_json::to_string(&req).map_err(|e| format!("serialize: {e}"))?;
    let resp_json = host::http_request(&req_json)?;
    let resp: HttpResp = serde_json::from_str(&resp_json).map_err(|e| format!("parse HTTP: {e}"))?;
    if resp.status >= 400 {
        return Err(format!("HTTP {}: {}", resp.status, resp.body));
    }
    Ok(resp.body)
}

fn define(input: &Value) -> Result<String, String> {
    let word = get_str(input, "word")?.to_lowercase();
    let url = format!("https://api.dictionaryapi.dev/api/v2/entries/en/{word}");
    let body = http_get(&url)?;

    let entries: Vec<Value> = serde_json::from_str(&body)
        .map_err(|_| format!("Word '{word}' not found. Check spelling or try a different word."))?;

    let entry = &entries[0];
    let word_display = entry.get("word").and_then(Value::as_str).unwrap_or(&word);

    let mut out = String::new();

    // Phonetics
    if let Some(phonetics) = entry.get("phonetics").and_then(Value::as_array) {
        for p in phonetics {
            if let Some(text) = p.get("text").and_then(Value::as_str) {
                out.push_str(&format!("Phonetic: {text}\n"));
                break;
            }
        }
    }

    // Meanings
    if let Some(meanings) = entry.get("meanings").and_then(Value::as_array) {
        for meaning in meanings {
            let pos = meaning.get("partOfSpeech").and_then(Value::as_str).unwrap_or("?");
            out.push_str(&format!("\n[{pos}]\n"));

            if let Some(defs) = meaning.get("definitions").and_then(Value::as_array) {
                for (i, def) in defs.iter().enumerate() {
                    if i >= 3 { break; }
                    let d = def.get("definition").and_then(Value::as_str).unwrap_or("");
                    out.push_str(&format!("  {}. {}\n", i + 1, d));
                    if let Some(ex) = def.get("example").and_then(Value::as_str) {
                        out.push_str(&format!("     Example: \"{}\"\n", ex));
                    }
                }
            }

            if let Some(syns) = meaning.get("synonyms").and_then(Value::as_array) {
                if !syns.is_empty() {
                    let syn_list: Vec<&str> = syns.iter().filter_map(Value::as_str).take(10).collect();
                    out.push_str(&format!("  Synonyms: {}\n", syn_list.join(", ")));
                }
            }
            if let Some(ants) = meaning.get("antonyms").and_then(Value::as_array) {
                if !ants.is_empty() {
                    let ant_list: Vec<&str> = ants.iter().filter_map(Value::as_str).take(5).collect();
                    out.push_str(&format!("  Antonyms: {}\n", ant_list.join(", ")));
                }
            }
        }
    }

    Ok(format!("{word_display}\n{out}"))
}
