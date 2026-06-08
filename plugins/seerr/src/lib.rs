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
            secret-read: func(key: string) -> result<string, string>;
        }

        world plugin {
            import host;
            export call: func(tool: string, input-json: string) -> result<string, string>;
        }
    "#,
    world: "plugin",
});

use helpcore::plugin::host;

struct Seerr;

impl Guest for Seerr {
    fn call(tool: String, input_json: String) -> Result<String, String> {
        let input: Value =
            serde_json::from_str(&input_json).map_err(|e| format!("invalid input JSON: {e}"))?;

        match tool.as_str() {
            "seerr_search" => seerr_search(&input),
            "seerr_request" => seerr_request(&input),
            "seerr_trending" => seerr_trending(&input),
            "seerr_requests" => seerr_requests(&input),
            _ => Err(format!("unknown tool: {tool}")),
        }
    }
}

export!(Seerr);

// ── Config ────────────────────────────────────────────────────────────────────

struct Config {
    url: String,
    api_key: String,
}

fn load_config() -> Result<Config, String> {
    let url = host::config_read("url")
        .map_err(|_| "Seerr URL is not configured. Set it in the plugin settings.".to_string())?;
    let api_key = host::secret_read("api_key")
        .map_err(|_| "Seerr API key is not configured. Find it in Seerr → Settings → General → API Key and add it to your plugin settings.".to_string())?;
    Ok(Config {
        url: url.trim_end_matches('/').to_string(),
        api_key,
    })
}

// ── HTTP helpers ──────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct HttpRequest {
    method: String,
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

fn seerr_headers(api_key: &str) -> serde_json::Map<String, Value> {
    let mut headers = serde_json::Map::new();
    headers.insert("X-Api-Key".into(), Value::String(api_key.to_string()));
    headers.insert("Accept".into(), Value::String("application/json".into()));
    headers
}

fn http_get(config: &Config, path: &str) -> Result<(u16, String), String> {
    let req = HttpRequest {
        method: "GET".into(),
        url: format!("{}/api/v1{path}", config.url),
        headers: seerr_headers(&config.api_key),
        body: None,
    };
    let req_json = serde_json::to_string(&req).map_err(|e| format!("serialize: {e}"))?;
    let resp_json = host::http_request(&req_json)?;
    let resp: HttpResponse =
        serde_json::from_str(&resp_json).map_err(|e| format!("parse HTTP: {e}"))?;
    Ok((resp.status, resp.body))
}

fn http_post_json(config: &Config, path: &str, body: Value) -> Result<(u16, String), String> {
    let mut headers = seerr_headers(&config.api_key);
    headers.insert(
        "Content-Type".into(),
        Value::String("application/json".into()),
    );
    let req = HttpRequest {
        method: "POST".into(),
        url: format!("{}/api/v1{path}", config.url),
        headers,
        body: Some(serde_json::to_string(&body).map_err(|e| format!("serialize: {e}"))?),
    };
    let req_json = serde_json::to_string(&req).map_err(|e| format!("serialize: {e}"))?;
    let resp_json = host::http_request(&req_json)?;
    let resp: HttpResponse =
        serde_json::from_str(&resp_json).map_err(|e| format!("parse HTTP: {e}"))?;
    Ok((resp.status, resp.body))
}

fn seerr_get(config: &Config, path: &str) -> Result<Value, String> {
    let (status, body) = http_get(config, path)?;
    if status == 401 || status == 403 {
        return Err("Unauthorized — check your Seerr API key in plugin settings.".into());
    }
    if status >= 400 {
        return Err(format!(
            "Seerr returned HTTP {status}: {}",
            truncate(&body, 300)
        ));
    }
    serde_json::from_str(&body).map_err(|e| format!("parse response: {e}"))
}

fn seerr_post(config: &Config, path: &str, body: Value) -> Result<Value, String> {
    let (status, resp_body) = http_post_json(config, path, body)?;
    if status == 401 || status == 403 {
        return Err("Unauthorized — check your Seerr API key in plugin settings.".into());
    }
    if status >= 400 {
        return Err(format!(
            "Seerr returned HTTP {status}: {}",
            truncate(&resp_body, 300)
        ));
    }
    serde_json::from_str(&resp_body).map_err(|e| format!("parse response: {e}"))
}

fn truncate(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max])
    }
}

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

// ── Shared helpers ────────────────────────────────────────────────────────────

fn result_title(item: &Value) -> &str {
    item.get("title")
        .or_else(|| item.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("Unknown")
}

fn result_type(item: &Value) -> &str {
    item.get("mediaType").and_then(Value::as_str).unwrap_or("?")
}

fn result_year(item: &Value) -> String {
    item.get("releaseDate")
        .or_else(|| item.get("firstAirDate"))
        .and_then(Value::as_str)
        .and_then(|d| d.get(..4))
        .unwrap_or("?")
        .to_string()
}

fn result_tmdb_id(item: &Value) -> u64 {
    item.get("id").and_then(Value::as_u64).unwrap_or(0)
}

fn result_overview(item: &Value) -> String {
    truncate(
        item.get("overview").and_then(Value::as_str).unwrap_or(""),
        280,
    )
}

fn result_poster(item: &Value) -> &str {
    item.get("posterPath").and_then(Value::as_str).unwrap_or("")
}

fn format_item(item: &Value) -> String {
    let title = result_title(item);
    let year = result_year(item);
    let media_type = result_type(item);
    let tmdb_id = result_tmdb_id(item);
    let overview = result_overview(item);
    let poster = result_poster(item);

    let type_label = match media_type {
        "movie" => "Movie",
        "tv" => "TV Show",
        "person" => "Person",
        other => other,
    };

    let mut out = format!("**{title}** ({year}) [{type_label}] [TMDB: {tmdb_id}]");
    if !overview.is_empty() {
        out.push_str(&format!("\n  {overview}"));
    }
    if !poster.is_empty() {
        out.push_str(&format!("\n  Poster: {poster}"));
    }
    out
}

fn map_status(status_val: &Value) -> &str {
    match status_val.as_u64().unwrap_or(0) {
        1 => "Pending",
        2 => "Approved",
        3 => "Declined",
        _ => "Unknown",
    }
}

// ── Tools ─────────────────────────────────────────────────────────────────────

fn seerr_search(input: &Value) -> Result<String, String> {
    let config = load_config()?;
    let query = input
        .get("query")
        .and_then(Value::as_str)
        .ok_or("query is required")?;
    let query = query.trim();
    if query.is_empty() {
        return Err("query must not be empty".to_string());
    }

    let path = format!("/search?query={}", url_encode(query));
    let data = seerr_get(&config, &path)?;

    let results = data["results"]
        .as_array()
        .ok_or_else(|| format!("No results found for '{}'.", query))?;

    if results.is_empty() {
        return Ok(format!("No results found for '{}'.", query));
    }

    let mut out = String::new();
    let mut count: usize = 0;
    let limit: usize = 10;

    for item in results.iter() {
        if count >= limit {
            break;
        }
        let title = result_title(item);
        if title == "Unknown" {
            continue;
        }
        if count > 0 {
            out.push_str("\n\n");
        }
        out.push_str(&format_item(item));
        count += 1;
    }

    if count == 0 {
        return Ok(format!("No results found for '{}'.", query));
    }

    Ok(out)
}

fn seerr_request(input: &Value) -> Result<String, String> {
    let config = load_config()?;
    let media_type = input
        .get("media_type")
        .and_then(Value::as_str)
        .ok_or("media_type is required (movie or tv)")?;
    let tmdb_id = input
        .get("tmdb_id")
        .and_then(Value::as_u64)
        .ok_or("tmdb_id is required")?;
    let seasons_raw = input.get("seasons");

    let media_type = media_type.to_lowercase();
    let media_type = match media_type.as_str() {
        "movie" => "movie",
        "tv" => "tv",
        _ => return Err("media_type must be 'movie' or 'tv'".to_string()),
    };

    let mut body = serde_json::json!({
        "mediaType": media_type,
        "mediaId": tmdb_id,
    });

    if media_type == "tv" {
        let seasons_val: Value = match seasons_raw {
            Some(Value::String(s)) if s.trim() == "all" => Value::String("all".into()),
            Some(Value::String(s)) => {
                let nums: Result<Vec<Value>, _> = s
                    .split(',')
                    .map(|p| p.trim().parse::<u64>().map(Value::from))
                    .collect();
                Value::Array(nums.map_err(|_| {
                    "seasons must be 'all' or comma-separated numbers like '1,2,3'".to_string()
                })?)
            }
            None => Value::String("all".into()),
            _ => {
                return Err(
                    "seasons must be 'all' or comma-separated numbers like '1,2,3'".to_string(),
                )
            }
        };
        body.as_object_mut()
            .unwrap()
            .insert("seasons".into(), seasons_val);
    }

    let resp = seerr_post(&config, "/request", body)?;

    let media = resp.get("media").unwrap_or(&resp);
    let title = result_title(media);

    let media_label = if media_type == "movie" {
        "Movie"
    } else {
        "TV Show"
    };
    Ok(format!("Request submitted: **{title}** ({media_label}) [TMDB: {tmdb_id}]. Check the status with seerr_requests."))
}

fn seerr_trending(input: &Value) -> Result<String, String> {
    let config = load_config()?;
    let count = input
        .get("count")
        .and_then(Value::as_u64)
        .unwrap_or(10)
        .clamp(1, 20) as usize;

    let data = seerr_get(&config, "/discover/trending")?;

    let results = data["results"]
        .as_array()
        .ok_or("no trending results returned")?;

    if results.is_empty() {
        return Ok("No trending items found.".to_string());
    }

    let mut out = String::new();
    let mut shown: usize = 0;

    for item in results.iter() {
        if shown >= count {
            break;
        }
        let title = result_title(item);
        if title == "Unknown" {
            continue;
        }
        if shown > 0 {
            out.push_str("\n\n");
        }
        out.push_str(&format_item(item));
        shown += 1;
    }

    if shown == 0 {
        return Ok("No trending items found.".to_string());
    }

    Ok(out)
}

fn seerr_requests(input: &Value) -> Result<String, String> {
    let config = load_config()?;
    let count = input
        .get("count")
        .and_then(Value::as_u64)
        .unwrap_or(10)
        .clamp(1, 50) as usize;

    let path = format!("/request?take={count}&sort=created&order=desc");
    let data = seerr_get(&config, &path)?;

    let results = data["results"].as_array().ok_or("no requests found")?;

    if results.is_empty() {
        return Ok("No requests found.".to_string());
    }

    let mut out = String::new();
    let mut shown: usize = 0;

    for req in results.iter() {
        if shown >= count {
            break;
        }

        let media = req.get("media").unwrap_or(req);
        let title = result_title(media);
        let media_type = result_type(media);
        let status = map_status(&req["status"]);
        let requested_by = req
            .get("requestedBy")
            .and_then(|r| {
                r.get("username")
                    .or_else(|| r.get("plexUsername"))
                    .or_else(|| r.get("jellyfinUsername"))
            })
            .and_then(Value::as_str)
            .unwrap_or("?");
        let created = req.get("createdAt").and_then(Value::as_str).unwrap_or("?");
        let date = created.get(..10).unwrap_or(created);

        let type_label = match media_type {
            "movie" => "Movie",
            "tv" => "TV Show",
            other => other,
        };

        if shown > 0 {
            out.push('\n');
        }
        out.push_str(&format!(
            "[{status}] **{title}** ({type_label}) — requested by {requested_by} on {date}"
        ));

        let seasons = req.get("seasons");
        if let Some(arr) = seasons.and_then(Value::as_array) {
            let s: Vec<String> = arr
                .iter()
                .filter_map(Value::as_str)
                .map(|s| s.to_string())
                .collect();
            if !s.is_empty() {
                out.push_str(&format!("\n  Seasons: {}", s.join(", ")));
            }
        }

        shown += 1;
    }

    if shown == 0 {
        return Ok("No requests found.".to_string());
    }

    Ok(out)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_encode_basic() {
        assert_eq!(url_encode("hello"), "hello");
        assert_eq!(url_encode("hello world"), "hello%20world");
        assert_eq!(url_encode("dune"), "dune");
        assert_eq!(url_encode("the matrix"), "the%20matrix");
    }

    #[test]
    fn url_encode_special() {
        assert_eq!(
            url_encode("star wars: a new hope"),
            "star%20wars%3A%20a%20new%20hope"
        );
    }

    #[test]
    fn truncate_short() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello world", 11), "hello world");
    }

    #[test]
    fn truncate_long() {
        assert_eq!(truncate("hello world this is long", 10), "hello worl…");
    }

    #[test]
    fn map_status_values() {
        assert_eq!(map_status(&Value::from(1)), "Pending");
        assert_eq!(map_status(&Value::from(2)), "Approved");
        assert_eq!(map_status(&Value::from(3)), "Declined");
        assert_eq!(map_status(&Value::from(99)), "Unknown");
    }

    #[test]
    fn result_title_returns_name_fallback() {
        let item = serde_json::json!({"name": "Breaking Bad"});
        assert_eq!(result_title(&item), "Breaking Bad");
    }

    #[test]
    fn result_title_prefers_title() {
        let item = serde_json::json!({"title": "Inception", "name": "Inception"});
        assert_eq!(result_title(&item), "Inception");
    }

    #[test]
    fn result_year_from_release_date() {
        let item = serde_json::json!({"releaseDate": "2024-03-15"});
        assert_eq!(result_year(&item), "2024");
    }

    #[test]
    fn result_year_from_first_air_date() {
        let item = serde_json::json!({"firstAirDate": "2022-01-10"});
        assert_eq!(result_year(&item), "2022");
    }

    #[test]
    fn format_item_basic() {
        let item = serde_json::json!({
            "id": 123,
            "title": "Dune",
            "mediaType": "movie",
            "releaseDate": "2021-10-22",
            "overview": "A mythic and emotionally charged hero's journey.",
            "posterPath": "/dune.jpg"
        });
        let result = format_item(&item);
        assert!(result.contains("**Dune**"));
        assert!(result.contains("(2021)"));
        assert!(result.contains("[Movie]"));
        assert!(result.contains("[TMDB: 123]"));
        assert!(result.contains("hero's journey"));
        assert!(result.contains("Poster: /dune.jpg"));
    }
}
