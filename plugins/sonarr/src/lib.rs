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

struct Sonarr;

impl Guest for Sonarr {
    fn call(tool: String, input_json: String) -> Result<String, String> {
        let input: Value =
            serde_json::from_str(&input_json).map_err(|e| format!("invalid input JSON: {e}"))?;

        match tool.as_str() {
            "sonarr_search" => search(&input),
            "sonarr_add" => add(&input),
            "sonarr_calendar" => calendar(&input),
            "sonarr_wanted" => wanted(&input),
            _ => Err(format!("unknown tool: {tool}")),
        }
    }
}

export!(Sonarr);

// ── Config ────────────────────────────────────────────────────────────────────

struct Config {
    base_url: String,
    api_key: String,
}

fn load_config() -> Result<Config, String> {
    let url = host::config_read("url")
        .map_err(|_| "Sonarr URL is not configured. Set it in the plugin settings.".to_string())?;
    let base_url = url.trim_end_matches('/').to_string();
    let api_key = host::secret_read("api_key").map_err(|e| {
        if e.contains("not approved") {
            e
        } else {
            "Sonarr API key is not configured. Set it in the plugin settings.".to_string()
        }
    })?;
    Ok(Config { base_url, api_key })
}

// ── URL encoding ──────────────────────────────────────────────────────────────

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

// ── Date helpers ──────────────────────────────────────────────────────────────

fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            let leap = (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0);
            if leap {
                29
            } else {
                28
            }
        }
        _ => 30,
    }
}

fn add_days(date: &str, days: u32) -> String {
    let parts: Vec<&str> = date.split('-').collect();
    if parts.len() != 3 {
        return date.to_string();
    }
    let mut y: u32 = parts[0].parse().unwrap_or(2026);
    let mut m: u32 = parts[1].parse().unwrap_or(6);
    let mut d: u32 = parts[2].parse().unwrap_or(8);
    d += days;
    while d > days_in_month(y, m) {
        d -= days_in_month(y, m);
        m += 1;
        if m > 12 {
            m = 1;
            y += 1;
        }
    }
    format!("{y:04}-{m:02}-{d:02}")
}

fn today() -> String {
    "2026-06-08".to_string()
}

// ── HTTP helpers ──────────────────────────────────────────────────────────────

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

fn build_url(config: &Config, path: &str) -> String {
    let mut url = format!("{}/api/v3{path}", config.base_url);
    match url.contains('?') {
        true => url.push_str(&format!("&apikey={}", config.api_key)),
        false => url.push_str(&format!("?apikey={}", config.api_key)),
    }
    url
}

fn http_get(config: &Config, path: &str) -> Result<(u16, String), String> {
    let url = build_url(config, path);
    let mut headers = serde_json::Map::new();
    headers.insert("Accept".into(), Value::String("application/json".into()));

    let req = HttpRequest {
        method: "GET",
        url,
        headers,
        body: None,
    };
    let req_json = serde_json::to_string(&req).map_err(|e| e.to_string())?;
    let resp_json =
        host::http_request(&req_json).map_err(|e| format!("HTTP request failed: {e}"))?;
    let resp: HttpResponse = serde_json::from_str(&resp_json)
        .map_err(|e| format!("failed to parse HTTP response: {e}"))?;
    Ok((resp.status, resp.body))
}

fn http_post_json(config: &Config, path: &str, body: &Value) -> Result<(u16, String), String> {
    let url = build_url(config, path);
    let mut headers = serde_json::Map::new();
    headers.insert("Accept".into(), Value::String("application/json".into()));
    headers.insert(
        "Content-Type".into(),
        Value::String("application/json".into()),
    );

    let body_str = serde_json::to_string(body).map_err(|e| e.to_string())?;
    let req = HttpRequest {
        method: "POST",
        url,
        headers,
        body: Some(body_str),
    };
    let req_json = serde_json::to_string(&req).map_err(|e| e.to_string())?;
    let resp_json =
        host::http_request(&req_json).map_err(|e| format!("HTTP request failed: {e}"))?;
    let resp: HttpResponse = serde_json::from_str(&resp_json)
        .map_err(|e| format!("failed to parse HTTP response: {e}"))?;
    Ok((resp.status, resp.body))
}

fn api_key_suffix(config: &Config) -> String {
    format!("?apikey={}", url_encode(&config.api_key))
}

// ── Tools ─────────────────────────────────────────────────────────────────────

// ── sonarr_search ─────────────────────────────────────────────────────────────

fn search(input: &Value) -> Result<String, String> {
    let config = load_config()?;
    let query = input
        .get("query")
        .and_then(Value::as_str)
        .ok_or("query is required")?;
    let suffix = api_key_suffix(&config);
    let (status, body) = http_get(
        &config,
        &format!("/series/lookup?term={}{}", url_encode(query), suffix),
    )?;

    if status >= 400 {
        return Err(format!(
            "Sonarr returned HTTP {status}: {}",
            truncate(&body, 300)
        ));
    }

    let results: Vec<Value> =
        serde_json::from_str(&body).map_err(|e| format!("failed to parse response: {e}"))?;

    if results.is_empty() {
        return Ok(format!("No shows found for '{query}'."));
    }

    let limit = 8;
    let mut out = format!("Sonarr search: {query}\n");
    for (i, show) in results.iter().take(limit).enumerate() {
        let title = show.get("title").and_then(Value::as_str).unwrap_or("?");
        let year = show
            .get("year")
            .and_then(Value::as_u64)
            .map(|y| y.to_string())
            .unwrap_or_else(|| "?".into());
        let tvdb_id = show.get("tvdbId").and_then(Value::as_u64).unwrap_or(0);
        let overview = show.get("overview").and_then(Value::as_str).unwrap_or("");
        let network = show.get("network").and_then(Value::as_str).unwrap_or("?");
        let status = show.get("status").and_then(Value::as_str).unwrap_or("?");
        let season_count = show.get("seasonCount").and_then(Value::as_u64).unwrap_or(0);

        let overview_short: String = if overview.len() > 120 {
            format!("{}…", &overview[..120])
        } else {
            overview.to_string()
        };

        out.push_str(&format!(
            "\n  {}. {} ({})  [tvdbId: {tvdb_id}]",
            i + 1,
            title,
            year
        ));
        out.push_str(&format!(
            "\n     Network: {network}  Status: {status}  Seasons: {season_count}"
        ));
        if !overview_short.is_empty() {
            out.push_str(&format!("\n     {}", overview_short));
        }
    }
    out.push_str(&format!("\n\n{} total results.", results.len().min(limit)));
    Ok(out)
}

// ── sonarr_add ────────────────────────────────────────────────────────────────

fn add(input: &Value) -> Result<String, String> {
    let config = load_config()?;
    let tvdb_id = input
        .get("tvdb_id")
        .and_then(Value::as_u64)
        .ok_or("tvdb_id is required")?;
    let title = input
        .get("title")
        .and_then(Value::as_str)
        .ok_or("title is required")?;
    let monitored = input
        .get("monitored")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let suffix = api_key_suffix(&config);

    // Lookup full series details
    let (status, lookup_body) = http_get(
        &config,
        &format!("/series/lookup?term=tvdb:{tvdb_id}{}", suffix),
    )?;
    if status >= 400 {
        return Err(format!(
            "Series lookup failed (HTTP {status}): {}",
            truncate(&lookup_body, 300)
        ));
    }
    let results: Vec<Value> = serde_json::from_str(&lookup_body)
        .map_err(|e| format!("failed to parse lookup response: {e}"))?;
    if results.is_empty() {
        return Err(format!("No series found with tvdbId: {tvdb_id}"));
    }
    let series = &results[0];
    let series_title = series.get("title").and_then(Value::as_str).unwrap_or(title);

    // Get root folder
    let (rf_status, rf_body) =
        http_get(&config, &format!("/rootfolder{}", api_key_suffix(&config)))?;
    if rf_status >= 400 {
        return Err(format!("Failed to get root folders (HTTP {rf_status})"));
    }
    let root_folders: Vec<Value> =
        serde_json::from_str(&rf_body).map_err(|e| format!("failed to parse root folders: {e}"))?;
    let root_folder = root_folders
        .first()
        .and_then(|f| f.get("path").and_then(Value::as_str))
        .ok_or("No root folder configured. Set one in Sonarr → Settings → Media Management.")?;

    // Get quality profile (from input or first available)
    let quality_profile_id =
        if let Some(qp_id) = input.get("quality_profile_id").and_then(Value::as_u64) {
            qp_id
        } else {
            let (qp_status, qp_body) = http_get(
                &config,
                &format!("/qualityprofile{}", api_key_suffix(&config)),
            )?;
            if qp_status >= 400 {
                return Err(format!("Failed to get quality profiles (HTTP {qp_status})"));
            }
            let profiles: Vec<Value> = serde_json::from_str(&qp_body)
                .map_err(|e| format!("failed to parse quality profiles: {e}"))?;
            profiles
                .first()
                .and_then(|p| p.get("id").and_then(Value::as_u64))
                .ok_or("No quality profile found in Sonarr.")?
        };

    // Build add body
    let add_options = serde_json::json!({ "searchForMissingEpisodes": true });
    let body = serde_json::json!({
        "tvdbId": tvdb_id,
        "title": series_title,
        "qualityProfileId": quality_profile_id,
        "monitored": monitored,
        "rootFolderPath": root_folder,
        "seasonFolder": true,
        "addOptions": add_options,
    });

    let (add_status, add_body) = http_post_json(
        &config,
        &format!("/series{}", api_key_suffix(&config)),
        &body,
    )?;
    if add_status >= 400 {
        let msg = match add_status {
            400 => {
                let errs: Vec<String> = serde_json::from_str::<Vec<Value>>(&add_body)
                    .ok()
                    .map(|v| {
                        v.iter()
                            .filter_map(|e| {
                                e.get("errorMessage")
                                    .and_then(Value::as_str)
                                    .map(String::from)
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                if errs.is_empty() {
                    format!("Bad request: {}", truncate(&add_body, 200))
                } else {
                    errs.join("; ")
                }
            }
            409 => "This series may already be added to Sonarr.".to_string(),
            _ => format!("HTTP {add_status}: {}", truncate(&add_body, 200)),
        };
        return Err(format!("Failed to add '{series_title}': {msg}"));
    }

    Ok(format!(
        "Added '{series_title}' to Sonarr.\n  TVDB ID: {tvdb_id}\n  Quality Profile ID: {quality_profile_id}\n  Root Folder: {root_folder}\n  Monitored: {monitored}"
    ))
}

// ── sonarr_calendar ───────────────────────────────────────────────────────────

fn calendar(input: &Value) -> Result<String, String> {
    let config = load_config()?;
    let days = input.get("days").and_then(Value::as_u64).unwrap_or(14);
    let start = today();
    let end = add_days(&start, days as u32);
    let suffix = api_key_suffix(&config);

    let (status, body) = http_get(
        &config,
        &format!("/calendar?start={start}&end={end}{suffix}"),
    )?;
    if status >= 400 {
        return Err(format!(
            "Calendar lookup failed (HTTP {status}): {}",
            truncate(&body, 300)
        ));
    }

    let episodes: Vec<Value> = serde_json::from_str(&body)
        .map_err(|e| format!("failed to parse calendar response: {e}"))?;

    if episodes.is_empty() {
        return Ok(format!("No episodes in the next {days} days."));
    }

    let mut out = format!("Sonarr Calendar — next {days} days:\n");
    for ep in &episodes {
        let series_title = ep
            .get("series")
            .and_then(|s| s.get("title"))
            .and_then(Value::as_str)
            .unwrap_or("?");
        let ep_title = ep.get("title").and_then(Value::as_str).unwrap_or("?");
        let season = ep.get("seasonNumber").and_then(Value::as_u64).unwrap_or(0);
        let episode = ep.get("episodeNumber").and_then(Value::as_u64).unwrap_or(0);
        let air_date = ep.get("airDate").and_then(Value::as_str).unwrap_or("?");
        let has_file = ep.get("hasFile").and_then(Value::as_bool).unwrap_or(false);
        let downloaded = if has_file { " [downloaded]" } else { "" };

        out.push_str(&format!(
            "\n  {series_title} — {ep_title}\n    S{season:02}E{episode:02}  Airs: {air_date}{downloaded}"
        ));
    }
    out.push_str(&format!("\n\n{} episode(s).", episodes.len()));
    Ok(out)
}

// ── sonarr_wanted ─────────────────────────────────────────────────────────────

fn wanted(input: &Value) -> Result<String, String> {
    let config = load_config()?;
    let count = input.get("count").and_then(Value::as_u64).unwrap_or(20);
    let suffix = api_key_suffix(&config);

    let (status, body) = http_get(
        &config,
        &format!("/wanted/missing?sortKey=airDateUtc&pageSize={count}{suffix}"),
    )?;
    if status >= 400 {
        return Err(format!(
            "Wanted lookup failed (HTTP {status}): {}",
            truncate(&body, 300)
        ));
    }

    let result: Value =
        serde_json::from_str(&body).map_err(|e| format!("failed to parse wanted response: {e}"))?;

    let episodes = result
        .get("records")
        .and_then(Value::as_array)
        .ok_or("unexpected response format")?;

    if episodes.is_empty() {
        return Ok("All episodes are downloaded. Nothing wanted!".to_string());
    }

    let mut out = format!("Sonarr Wanted — {} missing episode(s):\n", episodes.len());
    for ep in episodes {
        let series_title = ep
            .get("series")
            .and_then(|s| s.get("title"))
            .and_then(Value::as_str)
            .unwrap_or("?");
        let ep_title = ep.get("title").and_then(Value::as_str).unwrap_or("?");
        let season = ep.get("seasonNumber").and_then(Value::as_u64).unwrap_or(0);
        let episode = ep.get("episodeNumber").and_then(Value::as_u64).unwrap_or(0);
        let air_date = ep.get("airDate").and_then(Value::as_str).unwrap_or("?");

        out.push_str(&format!(
            "\n  {series_title} — {ep_title}\n    S{season:02}E{episode:02}  Airs: {air_date}"
        ));
    }
    Ok(out)
}

// ── Utility ───────────────────────────────────────────────────────────────────

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        s
    } else {
        &s[..max]
    }
}
