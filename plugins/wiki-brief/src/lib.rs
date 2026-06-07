// Wiki Brief plugin - powered by the Wikipedia REST API (no API key needed)
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

struct WikiBrief;

impl Guest for WikiBrief {
    fn call(tool: String, input_json: String) -> Result<String, String> {
        let input: Value = serde_json::from_str(&input_json)
            .map_err(|e| format!("invalid input JSON: {e}"))?;

        match tool.as_str() {
            "wiki_summary" => wiki_summary(&input),
            _ => Err(format!("unknown tool: {tool}")),
        }
    }
}

export!(WikiBrief);

fn url_encode(s: &str) -> String {
    let mut result = String::new();
    for byte in s.bytes() {
        match byte {
            b'0'..=b'9' | b'A'..=b'Z' | b'a'..=b'z' | b'-' | b'_' | b'.' | b'~' => {
                result.push(byte as char);
            }
            b' ' => result.push('_'),
            _ => result.push_str(&format!("%{:02X}", byte)),
        }
    }
    result
}

fn url_encode_query(s: &str) -> String {
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

fn http_get(url: &str) -> Result<(u16, String), String> {
    let mut headers = serde_json::Map::new();
    headers.insert("Accept".into(), Value::String("application/json".into()));
    headers.insert(
        "User-Agent".into(),
        Value::String("helpcore-wiki-brief-plugin/0.1 (https://github.com/doomedramen/helpcore)".into()),
    );

    let req = HttpRequest {
        method: "GET",
        url: url.to_string(),
        headers,
        body: None,
    };
    let req_json =
        serde_json::to_string(&req).map_err(|e| format!("failed to serialize request: {e}"))?;
    let resp_json = host::http_request(&req_json)?;
    let resp: HttpResponse =
        serde_json::from_str(&resp_json).map_err(|e| format!("failed to parse HTTP response: {e}"))?;
    Ok((resp.status, resp.body))
}

#[derive(Deserialize)]
struct Summary {
    #[serde(rename = "type")]
    page_type: Option<String>,
    title: String,
    description: Option<String>,
    extract: Option<String>,
    content_urls: Option<ContentUrls>,
}

#[derive(Deserialize)]
struct ContentUrls {
    desktop: Option<DesktopUrls>,
}

#[derive(Deserialize)]
struct DesktopUrls {
    page: Option<String>,
}

fn fetch_summary(title: &str) -> Result<Option<Summary>, String> {
    let url = format!(
        "https://en.wikipedia.org/api/rest_v1/page/summary/{}",
        url_encode(title)
    );
    let (status, body) = http_get(&url)?;
    if status == 404 {
        return Ok(None);
    }
    if status >= 400 {
        return Err(format!("Wikipedia returned HTTP {status}: {body}"));
    }
    let summary: Summary =
        serde_json::from_str(&body).map_err(|e| format!("failed to parse Wikipedia response: {e}"))?;
    Ok(Some(summary))
}

/// opensearch returns: ["query", ["title1", "title2", ...], [...descriptions], [...urls]]
fn search_best_title(query: &str) -> Result<Option<String>, String> {
    let url = format!(
        "https://en.wikipedia.org/w/api.php?action=opensearch&search={}&limit=1&namespace=0&format=json",
        url_encode_query(query)
    );
    let (status, body) = http_get(&url)?;
    if status >= 400 {
        return Err(format!("Wikipedia search returned HTTP {status}: {body}"));
    }
    let parsed: Value =
        serde_json::from_str(&body).map_err(|e| format!("failed to parse Wikipedia search response: {e}"))?;
    let title = parsed
        .as_array()
        .and_then(|arr| arr.get(1))
        .and_then(Value::as_array)
        .and_then(|titles| titles.first())
        .and_then(Value::as_str)
        .map(str::to_string);
    Ok(title)
}

fn render_summary(summary: &Summary, note: Option<&str>) -> String {
    let mut out = String::new();
    out.push_str(&summary.title);
    if let Some(ref desc) = summary.description {
        out.push_str(&format!(" — {desc}"));
    }
    out.push('\n');

    if summary.page_type.as_deref() == Some("disambiguation") {
        out.push_str("\n(This is a disambiguation page — the topic could refer to several things. Consider asking the user to be more specific.)\n");
    }

    if let Some(ref extract) = summary.extract {
        out.push('\n');
        out.push_str(extract);
        out.push('\n');
    }

    if let Some(ref urls) = summary.content_urls {
        if let Some(ref desktop) = urls.desktop {
            if let Some(ref page) = desktop.page {
                out.push_str(&format!("\nFull article: {page}"));
            }
        }
    }

    if let Some(n) = note {
        out.push_str(&format!("\n\n{n}"));
    }

    out
}

fn wiki_summary(input: &Value) -> Result<String, String> {
    let topic = input
        .get("topic")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or("topic is required, e.g. 'Ada Lovelace'")?;

    if let Some(summary) = fetch_summary(topic)? {
        return Ok(render_summary(&summary, None));
    }

    // No exact match — search for the closest article and retry.
    match search_best_title(topic)? {
        Some(best) if best.to_lowercase() != topic.to_lowercase() => {
            match fetch_summary(&best)? {
                Some(summary) => Ok(render_summary(
                    &summary,
                    Some(&format!("(No exact article named '{topic}' — showing the closest match, '{best}'.)")),
                )),
                None => Err(format!(
                    "No Wikipedia article found for '{topic}' (closest match '{best}' could not be loaded either)."
                )),
            }
        }
        _ => Err(format!(
            "No Wikipedia article found for '{topic}'. Try a more specific or differently-spelled name."
        )),
    }
}
