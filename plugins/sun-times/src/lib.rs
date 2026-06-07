// Sun Times plugin - geocoding via Open-Meteo, sun data via sunrise-sunset.org (no API key needed)
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

struct SunTimes;

impl Guest for SunTimes {
    fn call(tool: String, input_json: String) -> Result<String, String> {
        let input: Value = serde_json::from_str(&input_json)
            .map_err(|e| format!("invalid input JSON: {e}"))?;

        match tool.as_str() {
            "sun_times" => sun_times(&input),
            _ => Err(format!("unknown tool: {tool}")),
        }
    }
}

export!(SunTimes);

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

// ── Geocoding (mirrors the approach used by the weather plugin) ──────────────

#[derive(Deserialize)]
struct GeoResult {
    name: String,
    latitude: f64,
    longitude: f64,
    country: Option<String>,
    country_code: Option<String>,
    admin1: Option<String>,
    timezone: Option<String>,
}

#[derive(Deserialize)]
struct GeoResponse {
    results: Option<Vec<GeoResult>>,
}

fn parse_location_hint(input: &str) -> (&str, Option<&str>) {
    let trimmed = input.trim();
    if let Some(comma_pos) = trimmed.find(',') {
        let name = trimmed[..comma_pos].trim();
        let hint = trimmed[comma_pos + 1..].trim();
        (name, Some(hint))
    } else {
        (trimmed, None)
    }
}

fn country_matches(result: &GeoResult, country_hint: &str) -> bool {
    let hint = country_hint.trim().to_lowercase();
    let hint = match hint.as_str() {
        "uk" => "united kingdom",
        "usa" => "united states",
        h => h,
    };
    if let Some(ref c) = result.country {
        if c.to_lowercase() == hint {
            return true;
        }
    }
    if let Some(ref code) = result.country_code {
        if code.to_lowercase() == hint {
            return true;
        }
    }
    false
}

fn geocode(location: &str) -> Result<GeoResult, String> {
    let (search_name, country_hint) = parse_location_hint(location);

    let geo_url = format!(
        "https://geocoding-api.open-meteo.com/v1/search?name={}&count=10&language=en&format=json",
        url_encode(search_name)
    );
    let geo_body = http_get(&geo_url)?;
    let geo: GeoResponse = serde_json::from_str(&geo_body)
        .map_err(|e| format!("failed to parse geocoding response: {e}"))?;

    let mut results = geo.results.filter(|r| !r.is_empty()).ok_or_else(|| {
        format!(
            "Location '{location}' not found. Try a more specific name, e.g. 'Norwich, UK' or 'Berlin, Germany'."
        )
    })?;

    let result = match country_hint {
        Some(hint) => {
            let pos = results.iter().position(|r| country_matches(r, hint));
            match pos {
                Some(i) => results.swap_remove(i),
                None => results.swap_remove(0),
            }
        }
        None => results.swap_remove(0),
    };

    Ok(result)
}

fn place_name(result: &GeoResult) -> String {
    [result.name.as_str(), result.admin1.as_deref().unwrap_or(""), result.country.as_deref().unwrap_or("")]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

// ── Date handling ─────────────────────────────────────────────────────────────

fn normalize_date(input: Option<&str>) -> Result<String, String> {
    let raw = input.map(str::trim).filter(|s| !s.is_empty()).unwrap_or("today");
    let lower = raw.to_lowercase();
    if lower == "today" || lower == "tomorrow" {
        return Ok(lower);
    }

    // Strict YYYY-MM-DD check.
    let bytes = raw.as_bytes();
    let valid = raw.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && bytes[5..7].iter().all(u8::is_ascii_digit)
        && bytes[8..10].iter().all(u8::is_ascii_digit);
    if !valid {
        return Err(format!(
            "'{raw}' doesn't look like a date — use 'today', 'tomorrow', or 'YYYY-MM-DD' (e.g. '2026-12-21')."
        ));
    }
    Ok(raw.to_string())
}

// ── Sunrise/sunset ────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct SunResponseEnvelope {
    status: String,
    results: Option<SunResults>,
}

#[derive(Deserialize)]
struct SunResults {
    sunrise: String,
    sunset: String,
    solar_noon: String,
    day_length: u64,
    civil_twilight_begin: String,
    civil_twilight_end: String,
}

/// Extract the "HH:MM" portion from an ISO-8601 timestamp like "2026-06-07T04:42:48+01:00".
fn time_of_day(iso: &str) -> &str {
    match iso.find('T') {
        Some(t) if iso.len() >= t + 6 => &iso[t + 1..t + 6],
        _ => iso,
    }
}

fn format_duration(total_seconds: u64) -> String {
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    if hours == 0 {
        format!("{minutes}m")
    } else if minutes == 0 {
        format!("{hours}h")
    } else {
        format!("{hours}h {minutes}m")
    }
}

fn sun_times(input: &Value) -> Result<String, String> {
    let location = input
        .get("location")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or("location is required, e.g. 'Lisbon' or 'Norfolk, UK'")?;

    let date = normalize_date(input.get("date").and_then(Value::as_str))?;

    let geo = geocode(location)?;
    let place = place_name(&geo);
    let tzid = geo.timezone.clone().unwrap_or_else(|| "UTC".to_string());

    let url = format!(
        "https://api.sunrise-sunset.org/json?lat={}&lng={}&date={}&formatted=0&tzid={}",
        geo.latitude,
        geo.longitude,
        url_encode(&date),
        url_encode(&tzid)
    );
    let body = http_get(&url)?;
    let envelope: SunResponseEnvelope =
        serde_json::from_str(&body).map_err(|e| format!("failed to parse sunrise-sunset response: {e}"))?;

    if envelope.status != "OK" {
        return Err(format!(
            "Sunrise/sunset lookup for {place} on {date} failed (status: {})",
            envelope.status
        ));
    }
    let r = envelope.results.ok_or("Sunrise/sunset service returned no data")?;

    let mut out = String::new();
    out.push_str(&format!("Sun times for {place} on {date} (local time, {tzid}):\n"));

    if r.day_length == 0 && r.sunrise.starts_with("1970-01-01") {
        out.push_str(
            "The sun does not rise or set there on this date — this location is experiencing either continuous daylight (midnight sun) or continuous darkness (polar night) at this time of year.\n",
        );
        out.push_str(&format!("Solar noon: {}\n", time_of_day(&r.solar_noon)));
    } else {
        out.push_str(&format!("Sunrise: {}\n", time_of_day(&r.sunrise)));
        out.push_str(&format!("Sunset: {}\n", time_of_day(&r.sunset)));
        out.push_str(&format!("Solar noon: {}\n", time_of_day(&r.solar_noon)));
        out.push_str(&format!("Day length: {}\n", format_duration(r.day_length)));
        out.push_str(&format!(
            "Civil twilight: begins {} (before sunrise), ends {} (after sunset)\n",
            time_of_day(&r.civil_twilight_begin),
            time_of_day(&r.civil_twilight_end)
        ));
    }

    Ok(out)
}
