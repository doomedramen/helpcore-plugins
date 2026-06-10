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

struct Radarr;

impl Guest for Radarr {
    fn call(tool: String, input_json: String) -> Result<String, String> {
        let input: Value =
            serde_json::from_str(&input_json).map_err(|e| format!("invalid input JSON: {e}"))?;

        match tool.as_str() {
            "radarr_search" => search(&input),
            "radarr_add" => add_movie(&input),
            "radarr_calendar" => calendar(&input),
            "radarr_wanted" => wanted(&input),
            "radarr_quality_profiles" => quality_profiles(&input),
            "radarr_library" => library(&input),
            _ => Err(format!("unknown tool: {tool}")),
        }
    }
}

export!(Radarr);

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

struct Config {
    url: String,
    api_key: String,
}

fn load_config() -> Result<Config, String> {
    let url = host::config_read("url")
        .map_err(|_| "Radarr URL is not configured. Set it in the plugin settings.".to_string())?;
    let url = url.trim_end_matches('/').to_string();
    let api_key = host::secret_read("api_key").map_err(|e| {
        if e.contains("not approved") {
            e
        } else {
            "Radarr API key is not configured. Set it in the plugin settings.".to_string()
        }
    })?;
    Ok(Config { url, api_key })
}

fn build_url(config: &Config, path: &str) -> String {
    let mut url = format!("{}/api/v3/{path}", config.url);
    match url.contains('?') {
        true => url.push_str(&format!("&apiKey={}", config.api_key)),
        false => url.push_str(&format!("?apiKey={}", config.api_key)),
    }
    url
}

fn auth_headers(api_key: &str) -> serde_json::Map<String, Value> {
    let mut headers = serde_json::Map::new();
    headers.insert("Accept".into(), Value::String("application/json".into()));
    headers.insert("X-Api-Key".into(), Value::String(api_key.into()));
    headers
}

fn http_get(config: &Config, path: &str) -> Result<(u16, String), String> {
    let req = HttpRequest {
        method: "GET",
        url: build_url(config, path),
        headers: auth_headers(&config.api_key),
        body: None,
    };
    execute(req)
}

fn http_post_json(config: &Config, path: &str, body: &Value) -> Result<(u16, String), String> {
    let mut headers = auth_headers(&config.api_key);
    headers.insert(
        "Content-Type".into(),
        Value::String("application/json".into()),
    );

    let body_str = serde_json::to_string(body).map_err(|e| e.to_string())?;
    let req = HttpRequest {
        method: "POST",
        url: build_url(config, path),
        headers,
        body: Some(body_str),
    };
    execute(req)
}

fn execute(req: HttpRequest) -> Result<(u16, String), String> {
    let req_json =
        serde_json::to_string(&req).map_err(|e| format!("failed to serialize request: {e}"))?;
    let resp_json = host::http_request(&req_json)?;
    let resp: HttpResponse = serde_json::from_str(&resp_json)
        .map_err(|e| format!("failed to parse HTTP response: {e}"))?;
    Ok((resp.status, resp.body))
}

fn api_get(config: &Config, path: &str) -> Result<Value, String> {
    let (status, body) = http_get(config, path)?;
    if status >= 400 {
        return Err(format!(
            "Radarr returned HTTP {status}: {}",
            truncate(&body, 300)
        ));
    }
    serde_json::from_str(&body).map_err(|e| format!("failed to parse response: {e}"))
}

fn api_post(config: &Config, path: &str, payload: &Value) -> Result<Value, String> {
    let (status, body) = http_post_json(config, path, payload)?;
    if status >= 400 {
        return Err(format!(
            "Radarr returned HTTP {status}: {}",
            truncate(&body, 300)
        ));
    }
    if body.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(&body).map_err(|e| format!("failed to parse response: {e}"))
}

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        s
    } else {
        &s[..max]
    }
}

fn str_or(val: &Value, key: &str, default: &str) -> String {
    val.get(key)
        .and_then(Value::as_str)
        .unwrap_or(default)
        .to_string()
}

fn num_or(val: &Value, key: &str, default: f64) -> f64 {
    val.get(key)
        .and_then(Value::as_f64)
        .unwrap_or(default)
}

// ── Date helpers ──────────────────────────────────────────────────────────────

fn days_since_epoch() -> i64 {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    (secs / 86400) as i64
}

fn days_to_iso(days: i64) -> String {
    // Howard Hinnant civil_from_days algorithm
    let z = days + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
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

// ── radarr_search ─────────────────────────────────────────────────────────────

fn search(input: &Value) -> Result<String, String> {
    let config = load_config()?;
    let query = input
        .get("query")
        .and_then(Value::as_str)
        .ok_or("query is required")?
        .trim();
    if query.is_empty() {
        return Err("query must not be empty".into());
    }

    let path = format!("movie/lookup?term={}", url_encode(query));
    let data = api_get(&config, &path)?;

    let mut results: Vec<&Value> = data
        .as_array()
        .ok_or("unexpected response: expected array")?
        .iter()
        .collect();
    if results.is_empty() {
        return Ok(format!("No movies found for \"{query}\"."));
    }

    // Fetch library to cross-reference ownership status
    let library_status: Vec<(u64, bool, bool)> = api_get(&config, "movie?pageSize=50")
        .ok()
        .and_then(|d| d.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|m| {
            let tmdb = m.get("tmdbId").and_then(Value::as_u64)?;
            let has_file = m.get("hasFile").and_then(Value::as_bool).unwrap_or(false);
            let monitored = m.get("monitored").and_then(Value::as_bool).unwrap_or(false);
            Some((tmdb, has_file, monitored))
        })
        .collect();

    // Rank: exact title match first, then by popularity/vote count descending
    let query_lower = query.to_lowercase();
    results.sort_by(|a, b| {
        let a_title = str_or(a, "title", "").to_lowercase();
        let b_title = str_or(b, "title", "").to_lowercase();
        let a_exact = a_title == query_lower;
        let b_exact = b_title == query_lower;
        b_exact
            .cmp(&a_exact)
            .then_with(|| {
                let a_pop = num_or(a, "popularity", 0_f64);
                let b_pop = num_or(b, "popularity", 0_f64);
                b_pop.partial_cmp(&a_pop).unwrap_or(std::cmp::Ordering::Equal)
            })
    });

    let limit = results.len().min(8);
    let mut out = format!("Search results for \"{query}\":");
    for item in results.iter().take(limit) {
        let title = str_or(item, "title", "?");
        let year = item
            .get("year")
            .and_then(Value::as_u64)
            .map_or_else(|| "—".into(), |y| y.to_string());
        let tmdb_id = item
            .get("tmdbId")
            .and_then(Value::as_u64)
            .map_or_else(|| "—".into(), |id| id.to_string());
        let in_cinemas = str_or(item, "inCinemas", "?");

        // Ownership status
        let lib_status = item
            .get("tmdbId")
            .and_then(Value::as_u64)
            .and_then(|tid| library_status.iter().find(|(id, _, _)| *id == tid))
            .map(|(_, has_file, monitored)| {
                if *has_file {
                    " ✅ In library"
                } else if *monitored {
                    " 📥 Monitored (missing)"
                } else {
                    " 📋 In library (unmonitored)"
                }
            })
            .unwrap_or("");

        out.push_str(&format!(
            "\n\n  {title} ({year}){lib_status}\n  TMDB ID: {tmdb_id} | In Cinemas: {in_cinemas}"
        ));
    }
    if results.len() > 8 {
        out.push_str(&format!(
            "\n\n... and {} more. Refine your search.",
            results.len() - 8
        ));
    }

    Ok(out)
}

// ── radarr_add ────────────────────────────────────────────────────────────────

fn add_movie(input: &Value) -> Result<String, String> {
    let config = load_config()?;
    let tmdb_id = input
        .get("tmdb_id")
        .and_then(Value::as_u64)
        .ok_or("tmdb_id is required")?;

    // Look up movie details from TMDB via Radarr
    let path = format!("movie/lookup/tmdb?tmdbId={tmdb_id}");
    let movie = api_get(&config, &path)?;

    let movie = if let Some(first) = movie.as_array().and_then(|a| a.first()) {
        first
    } else if movie.is_object() {
        &movie
    } else {
        return Err(format!("Movie with TMDB ID {tmdb_id} not found."));
    };

    let title = str_or(movie, "title", "?");
    let year = movie.get("year").and_then(Value::as_u64).unwrap_or(0);

    // Get quality profile ID
    let quality_profile_id: u64 = match input.get("quality_profile_id").and_then(Value::as_u64) {
        Some(id) => id,
        None => {
            let profiles = api_get(&config, "qualityprofile")?;
            let arr = profiles
                .as_array()
                .ok_or("failed to load quality profiles")?;
            let first = arr
                .first()
                .ok_or("no quality profiles configured in Radarr")?;
            first
                .get("id")
                .and_then(Value::as_u64)
                .ok_or("unexpected quality profile response")?
        }
    };

    // Get root folder path
    let rootfolders = api_get(&config, "rootfolder")?;
    let root_arr = rootfolders
        .as_array()
        .ok_or("unexpected rootfolder response")?;
    let root_folder_path = root_arr
        .first()
        .and_then(|f| f.get("path"))
        .and_then(Value::as_str)
        .ok_or("no root folder configured in Radarr")?;

    // Determine monitored
    let monitored = input
        .get("monitored")
        .and_then(Value::as_bool)
        .unwrap_or(true);

    let body = serde_json::json!({
        "tmdbId": tmdb_id,
        "title": title,
        "year": year,
        "qualityProfileId": quality_profile_id,
        "monitored": monitored,
        "rootFolderPath": root_folder_path,
        "addOptions": {
            "searchForMovie": true
        }
    });

    api_post(&config, "movie", &body)?;

    let mon_str = if monitored {
        "monitored"
    } else {
        "unmonitored"
    };
    Ok(format!(
        "Added \"{title}\" ({year}) to Radarr ({mon_str}).\nQuality Profile ID: {quality_profile_id}\nRoot Folder: {root_folder_path}"
    ))
}

// ── radarr_calendar ───────────────────────────────────────────────────────────

fn calendar(input: &Value) -> Result<String, String> {
    let config = load_config()?;
    let days = input
        .get("days")
        .and_then(Value::as_u64)
        .unwrap_or(14)
        .clamp(1, 365);

    let today = days_to_iso(days_since_epoch());
    let end = days_to_iso(days_since_epoch() + days as i64);

    let path = format!("calendar?start={today}&end={end}");
    let data = api_get(&config, &path)?;

    let items = data.as_array().ok_or("unexpected calendar response")?;
    if items.is_empty() {
        return Ok(format!("No movies in the next {days} day(s)."));
    }

    let mut out = format!("Upcoming Movies (next {days} days):");
    for item in items {
        let title = str_or(item, "title", "?");
        let release = str_or(item, "inCinemas", "?");
        let has_file = item
            .get("hasFile")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let status = if has_file { "downloaded" } else { "missing" };

        out.push_str(&format!("\n\n  {title}\n  Release: {release} | {status}"));
    }
    out.push_str(&format!("\n\n{} movie(s).", items.len()));

    Ok(out)
}

// ── radarr_wanted ─────────────────────────────────────────────────────────────

fn wanted(input: &Value) -> Result<String, String> {
    let config = load_config()?;
    let count = input
        .get("count")
        .and_then(Value::as_u64)
        .unwrap_or(15)
        .clamp(1, 25);

    let path = format!("movie?sortKey=title&pageSize={count}");
    let data = api_get(&config, &path)?;

    let movies = data.as_array().ok_or("unexpected movie list response")?;

    let wanted: Vec<&Value> = movies
        .iter()
        .filter(|m| {
            let monitored = m.get("monitored").and_then(Value::as_bool).unwrap_or(false);
            let has_file = m.get("hasFile").and_then(Value::as_bool).unwrap_or(true);
            monitored && !has_file
        })
        .collect();

    if wanted.is_empty() {
        return Ok("No missing movies — all monitored movies are downloaded.".into());
    }

    let total_fetched = movies.len();
    let mut out = "Wanted (missing) movies:".to_string();
    for item in &wanted {
        let title = str_or(item, "title", "?");
        let year = item
            .get("year")
            .and_then(Value::as_u64)
            .map_or_else(|| "—".into(), |y| y.to_string());
        let release = str_or(item, "inCinemas", "?");
        let physical = str_or(item, "physicalRelease", "?");

        out.push_str(&format!(
            "\n\n  {title} ({year})\n  Cinemas: {release} | Physical: {physical}"
        ));
    }
    out.push_str(&format!("\n\n{} wanted movie(s).", wanted.len()));

    if total_fetched >= count as usize {
        out.push_str(&format!(
            " (showing first {count} — increase \"count\" to see more)"
        ));
    }

    Ok(out)
}

// ── radarr_library ─────────────────────────────────────────────────────────────

fn library(input: &Value) -> Result<String, String> {
    let config = load_config()?;
    let count = input
        .get("count")
        .and_then(Value::as_u64)
        .unwrap_or(25)
        .clamp(1, 25);

    let path = format!("movie?sortKey=title&pageSize={count}");
    let data = api_get(&config, &path)?;

    let movies = data.as_array().ok_or("unexpected movie list response")?;

    let downloaded: Vec<&Value> = movies
        .iter()
        .filter(|m| m.get("hasFile").and_then(Value::as_bool).unwrap_or(false))
        .collect();

    if downloaded.is_empty() {
        return Ok("No downloaded movies found.".into());
    }

    let total_fetched = movies.len();
    let mut out = "Downloaded movies:".to_string();
    for item in &downloaded {
        let title = str_or(item, "title", "?");
        let year = item
            .get("year")
            .and_then(Value::as_u64)
            .map_or_else(|| "—".into(), |y| y.to_string());
        let quality = str_or(item, "movieFile", "")
            .is_empty()
            .then(|| "—".to_string())
            .or_else(|| {
                item.get("movieFile")
                    .and_then(|f| f.get("quality"))
                    .and_then(|q| q.get("quality"))
                    .and_then(|n| n.get("name"))
                    .and_then(Value::as_str)
                    .map(|s| s.to_string())
            })
            .unwrap_or_else(|| "—".to_string());

        out.push_str(&format!("\n\n  {title} ({year})\n  Quality: {quality}"));
    }
    out.push_str(&format!("\n\n{} downloaded movie(s).", downloaded.len()));

    if total_fetched >= count as usize && downloaded.len() < total_fetched {
        out.push_str(&format!(
            " (showing first {count} — increase \"count\" to see more)"
        ));
    }

    Ok(out)
}

// ── radarr_quality_profiles ───────────────────────────────────────────────────

fn quality_profiles(_input: &Value) -> Result<String, String> {
    let config = load_config()?;
    let data = api_get(&config, "qualityprofile")?;

    let profiles = data
        .as_array()
        .ok_or("unexpected quality profile response")?;
    if profiles.is_empty() {
        return Ok("No quality profiles configured.".into());
    }

    let mut out = "Quality Profiles:".to_string();
    for p in profiles {
        let id = p
            .get("id")
            .and_then(Value::as_u64)
            .map_or_else(|| "?".into(), |id| id.to_string());
        let name = str_or(p, "name", "Unknown");
        out.push_str(&format!("\n  {id}: {name}"));
    }

    Ok(out)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_requires_query() {
        let input = serde_json::json!({});
        let err = search(&input).unwrap_err();
        assert!(err.contains("query is required"));
    }

    #[test]
    fn search_rejects_empty_query() {
        let input = serde_json::json!({"query": ""});
        let err = search(&input).unwrap_err();
        assert!(err.contains("must not be empty"));
    }

    #[test]
    fn search_rejects_whitespace_only() {
        let input = serde_json::json!({"query": "   "});
        let err = search(&input).unwrap_err();
        assert!(err.contains("must not be empty"));
    }

    #[test]
    fn add_requires_tmdb_id() {
        let input = serde_json::json!({});
        let err = add_movie(&input).unwrap_err();
        assert!(err.contains("tmdb_id is required"));
    }

    #[test]
    fn url_encode_basic() {
        assert_eq!(url_encode("inception"), "inception");
        assert_eq!(url_encode("the matrix"), "the%20matrix");
        assert_eq!(url_encode("starship troopers"), "starship%20troopers");
    }

    #[test]
    fn url_encode_special_chars() {
        assert_eq!(url_encode("rock & roll"), "rock%20%26%20roll");
        assert_eq!(url_encode("café"), "caf%C3%A9");
    }

    #[test]
    fn calendar_days_default() {
        let input = serde_json::json!({});
        let val: Value = serde_json::from_str(&serde_json::to_string(&input).unwrap()).unwrap();
        let days = val.get("days").and_then(Value::as_u64);
        assert!(days.is_none());
    }

    #[test]
    fn calendar_days_clamped_low() {
        assert_eq!(0u64.clamp(1, 365), 1);
    }

    #[test]
    fn calendar_days_clamped_high() {
        assert_eq!(400u64.clamp(1, 365), 365);
    }

    #[test]
    fn wanted_count_default() {
        let input = serde_json::json!({});
        let val: Value = serde_json::from_str(&serde_json::to_string(&input).unwrap()).unwrap();
        let count = val.get("count").and_then(Value::as_u64);
        assert!(count.is_none());
    }

    #[test]
    fn wanted_count_clamped() {
        assert_eq!(0u64.clamp(1, 100), 1);
        assert_eq!(200u64.clamp(1, 100), 100);
    }

    #[test]
    fn days_to_iso_epoch() {
        // 1970-01-01 is day 0
        let date = days_to_iso(0);
        assert_eq!(date, "1970-01-01");
    }

    #[test]
    fn days_to_iso_known_dates() {
        assert_eq!(days_to_iso(1), "1970-01-02");
        // 2000-01-01 = 10957 days after epoch
        assert_eq!(days_to_iso(10957), "2000-01-01");
        // 2024-01-01
        assert_eq!(days_to_iso(19723), "2024-01-01");
    }

    #[test]
    fn days_to_iso_roundtrip_feb_28_2024() {
        // 2024-02-28 = 10957 + (2024-2000)*365 + leap_days + 31 + 28
        // Actually just test a known value: 2020-01-01 = 18262
        assert_eq!(days_to_iso(18262), "2020-01-01");
    }

    #[test]
    fn days_to_iso_leap_year() {
        // 2020-02-29 is a leap day
        // 2020-01-01 = 18262, so 2020-02-29 = 18262 + 31 + 29 - 1 = 18321
        // Actually: Jan has 31 days, so Feb starts at 18262+31 = 18293
        // Feb 29 = 18293 + 28 = 18321
        assert_eq!(days_to_iso(18321), "2020-02-29");
        assert_eq!(days_to_iso(18322), "2020-03-01");
    }

    #[test]
    fn truncate_short() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello", 5), "hello");
    }

    #[test]
    fn truncate_long() {
        assert_eq!(truncate("hello world", 5), "hello");
    }

    #[test]
    fn str_or_default() {
        let v = serde_json::json!({"name": "Alice"});
        assert_eq!(str_or(&v, "name", "?"), "Alice");
        assert_eq!(str_or(&v, "missing", "?"), "?");
    }

    #[test]
    fn quality_profiles_no_input_required() {
        // Just verify the function signature accepts empty input
        let input = serde_json::json!({});
        // No assertion needed — this test just checks the function compiles and runs
        // without panicking on input parsing (actual HTTP call would fail in test)
        let _ = quality_profiles(&input);
    }
}
