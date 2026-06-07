// Pollen Count plugin - powered by Open-Meteo / CAMS (no API key needed)
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

struct PollenCount;

impl Guest for PollenCount {
    fn call(tool: String, input_json: String) -> Result<String, String> {
        let input: Value = serde_json::from_str(&input_json)
            .map_err(|e| format!("invalid input JSON: {e}"))?;

        match tool.as_str() {
            "pollen_get" => pollen_get(&input),
            _ => Err(format!("unknown tool: {tool}")),
        }
    }
}

export!(PollenCount);

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

// ── Pollen ────────────────────────────────────────────────────────────────────

/// (API field name, display label). Order matches how results are presented.
const POLLEN_TYPES: &[(&str, &str)] = &[
    ("alder_pollen", "Alder"),
    ("birch_pollen", "Birch"),
    ("grass_pollen", "Grass"),
    ("mugwort_pollen", "Mugwort"),
    ("olive_pollen", "Olive"),
    ("ragweed_pollen", "Ragweed"),
];

/// Rates a concentration (grains/m³) using the clinically-relevant thresholds
/// CAMS/EAACI use for each pollen family.
fn pollen_rating(field: &str, value: f64) -> &'static str {
    let (low, moderate, high) = match field {
        "alder_pollen" | "birch_pollen" => (10.0, 100.0, 1000.0),
        "olive_pollen" => (10.0, 50.0, 200.0),
        _ => (5.0, 20.0, 50.0), // grass, mugwort, ragweed
    };
    if value <= 0.0 {
        "None"
    } else if value <= low {
        "Low"
    } else if value <= moderate {
        "Moderate"
    } else if value <= high {
        "High"
    } else {
        "Very High"
    }
}

fn format_level(field: &str, label: &str, value: f64) -> String {
    format!("{}: {} ({:.1} grains/m³)", label, pollen_rating(field, value), value)
}

#[derive(Deserialize, Default)]
struct PollenFields {
    alder_pollen: Option<f64>,
    birch_pollen: Option<f64>,
    grass_pollen: Option<f64>,
    mugwort_pollen: Option<f64>,
    olive_pollen: Option<f64>,
    ragweed_pollen: Option<f64>,
}

impl PollenFields {
    fn get(&self, field: &str) -> Option<f64> {
        match field {
            "alder_pollen" => self.alder_pollen,
            "birch_pollen" => self.birch_pollen,
            "grass_pollen" => self.grass_pollen,
            "mugwort_pollen" => self.mugwort_pollen,
            "olive_pollen" => self.olive_pollen,
            "ragweed_pollen" => self.ragweed_pollen,
            _ => None,
        }
    }
}

#[derive(Deserialize)]
struct CurrentPollen {
    #[serde(flatten)]
    fields: PollenFields,
}

#[derive(Deserialize, Default)]
struct HourlyPollen {
    time: Vec<String>,
    alder_pollen: Option<Vec<Option<f64>>>,
    birch_pollen: Option<Vec<Option<f64>>>,
    grass_pollen: Option<Vec<Option<f64>>>,
    mugwort_pollen: Option<Vec<Option<f64>>>,
    olive_pollen: Option<Vec<Option<f64>>>,
    ragweed_pollen: Option<Vec<Option<f64>>>,
}

impl HourlyPollen {
    fn series(&self, field: &str) -> Option<&Vec<Option<f64>>> {
        match field {
            "alder_pollen" => self.alder_pollen.as_ref(),
            "birch_pollen" => self.birch_pollen.as_ref(),
            "grass_pollen" => self.grass_pollen.as_ref(),
            "mugwort_pollen" => self.mugwort_pollen.as_ref(),
            "olive_pollen" => self.olive_pollen.as_ref(),
            "ragweed_pollen" => self.ragweed_pollen.as_ref(),
            _ => None,
        }
    }
}

#[derive(Deserialize)]
struct AirQualityResponse {
    current: Option<CurrentPollen>,
    #[serde(default)]
    hourly: HourlyPollen,
}

/// The date portion ("YYYY-MM-DD") of an Open-Meteo local timestamp like "2026-06-08T14:00".
fn date_of(timestamp: &str) -> &str {
    timestamp.get(..10).unwrap_or(timestamp)
}

fn pollen_get(input: &Value) -> Result<String, String> {
    let location = input
        .get("location")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or("location is required, e.g. 'Norfolk, UK' or 'Munich'")?;

    let days = input
        .get("days")
        .and_then(Value::as_u64)
        .unwrap_or(4)
        .clamp(1, 4);

    let geo = geocode(location)?;
    let place = place_name(&geo);

    let fields = POLLEN_TYPES.iter().map(|(f, _)| *f).collect::<Vec<_>>().join(",");
    let url = format!(
        "https://air-quality-api.open-meteo.com/v1/air-quality?latitude={}&longitude={}&current={}&hourly={}&timezone=auto&forecast_days={}",
        geo.latitude, geo.longitude, fields, fields, days
    );
    let body = http_get(&url)?;
    let aq: AirQualityResponse =
        serde_json::from_str(&body).map_err(|e| format!("failed to parse air quality response: {e}"))?;

    // Determine which pollen types actually have any data for this location/season —
    // CAMS pollen forecasts only cover Europe, and only during each plant's season.
    let available: Vec<&(&str, &str)> = POLLEN_TYPES
        .iter()
        .filter(|(field, _)| {
            let in_current = aq.current.as_ref().and_then(|c| c.fields.get(field)).is_some();
            let in_hourly = aq
                .hourly
                .series(field)
                .map(|series| series.iter().any(|v| v.is_some()))
                .unwrap_or(false);
            in_current || in_hourly
        })
        .collect();

    if available.is_empty() {
        return Ok(format!(
            "No pollen forecast data is available for {place}. CAMS pollen forecasts currently only cover Europe, and only during each plant's pollen season — try again during spring/summer, or for a European location.",
        ));
    }

    let mut out = String::new();
    out.push_str(&format!("Pollen forecast for {place}\n"));

    if let Some(current) = &aq.current {
        out.push_str("\nCurrent levels:\n");
        for (field, label) in &available {
            if let Some(value) = current.fields.get(field) {
                out.push_str(&format!("{}\n", format_level(field, label, value)));
            }
        }
    }

    out.push_str("\nDaily peak levels:\n");
    let n = aq.hourly.time.len();
    let mut current_date: Option<&str> = None;
    let mut peaks: Vec<(&str, &str, f64)> = Vec::new();

    let flush = |date: &str, peaks: &mut Vec<(&str, &str, f64)>, out: &mut String| {
        if peaks.is_empty() {
            return;
        }
        out.push_str(&format!("{date}: "));
        let line = peaks
            .iter()
            .map(|(field, label, value)| format!("{} {} ({:.1})", label, pollen_rating(field, *value), value))
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&line);
        out.push('\n');
        peaks.clear();
    };

    for i in 0..n {
        let date = date_of(&aq.hourly.time[i]);
        if current_date != Some(date) {
            if let Some(prev) = current_date {
                flush(prev, &mut peaks, &mut out);
            }
            current_date = Some(date);
        }
        for (field, label) in &available {
            if let Some(Some(value)) = aq.hourly.series(field).and_then(|series| series.get(i)) {
                match peaks.iter_mut().find(|(f, _, _)| f == field) {
                    Some(entry) if entry.2 < *value => entry.2 = *value,
                    Some(_) => {}
                    None => peaks.push((field, label, *value)),
                }
            }
        }
    }
    if let Some(date) = current_date {
        flush(date, &mut peaks, &mut out);
    }

    Ok(out)
}
