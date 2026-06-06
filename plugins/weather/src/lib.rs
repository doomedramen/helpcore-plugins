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

struct Weather;

impl Guest for Weather {
    fn call(tool: String, input_json: String) -> Result<String, String> {
        let input: Value = serde_json::from_str(&input_json)
            .map_err(|e| format!("invalid input JSON: {e}"))?;

        match tool.as_str() {
            "weather_get" => weather_get(&input),
            _ => Err(format!("unknown tool: {tool}")),
        }
    }
}

export!(Weather);

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
        return Err(format!("Weather API returned HTTP {}: {}", resp.status, resp.body));
    }
    Ok(resp.body)
}

#[derive(Deserialize)]
struct GeoResult {
    name: String,
    latitude: f64,
    longitude: f64,
    country: Option<String>,
    admin1: Option<String>,
}

#[derive(Deserialize)]
struct GeoResponse {
    results: Option<Vec<GeoResult>>,
}

#[derive(Deserialize)]
struct CurrentWeather {
    temperature: f64,
    weathercode: u16,
    windspeed: f64,
}

#[derive(Deserialize)]
struct DailyWeather {
    time: Vec<String>,
    temperature_2m_max: Vec<f64>,
    temperature_2m_min: Vec<f64>,
    weathercode: Vec<u16>,
    precipitation_sum: Vec<f64>,
    wind_speed_10m_max: Vec<f64>,
}

#[derive(Deserialize)]
struct ForecastResponse {
    current_weather: Option<CurrentWeather>,
    daily: DailyWeather,
}

fn wmo_description(code: u16) -> &'static str {
    match code {
        0 => "Clear sky",
        1 => "Mainly clear",
        2 => "Partly cloudy",
        3 => "Overcast",
        45 | 48 => "Foggy",
        51 => "Light drizzle",
        53 => "Moderate drizzle",
        55 => "Dense drizzle",
        56 | 57 => "Freezing drizzle",
        61 => "Slight rain",
        63 => "Moderate rain",
        65 => "Heavy rain",
        66 | 67 => "Freezing rain",
        71 => "Slight snow",
        73 => "Moderate snow",
        75 => "Heavy snow",
        77 => "Snow grains",
        80 => "Slight rain showers",
        81 => "Moderate rain showers",
        82 => "Violent rain showers",
        85 => "Slight snow showers",
        86 => "Heavy snow showers",
        95 => "Thunderstorm",
        96 | 99 => "Thunderstorm with hail",
        _ => "Unknown",
    }
}

fn weather_get(input: &Value) -> Result<String, String> {
    let location = input
        .get("location")
        .and_then(Value::as_str)
        .ok_or("location is required")?;

    let days = input
        .get("days")
        .and_then(Value::as_u64)
        .unwrap_or(3)
        .min(7)
        .max(1) as u8;

    let geo_url = format!(
        "https://geocoding-api.open-meteo.com/v1/search?name={}&count=1&language=en&format=json",
        url_encode(location)
    );
    let geo_body = http_get(&geo_url)?;
    let geo: GeoResponse =
        serde_json::from_str(&geo_body).map_err(|e| format!("failed to parse geocoding response: {e}"))?;

    let result = geo
        .results
        .and_then(|mut r| r.pop())
        .ok_or_else(|| {
            format!(
                "Location '{}' not found. Try a more specific name, e.g. 'Norfolk, UK' or 'Berlin, Germany'.",
                location
            )
        })?;

    let place_parts: Vec<&str> = [Some(result.name.as_str()), result.admin1.as_deref(), result.country.as_deref()]
        .into_iter()
        .flatten()
        .collect();
    let place_name = place_parts.join(", ");

    let forecast_url = format!(
        "https://api.open-meteo.com/v1/forecast?latitude={}&longitude={}&daily=temperature_2m_max,temperature_2m_min,weathercode,precipitation_sum,wind_speed_10m_max&current_weather=true&timezone=auto&forecast_days={}",
        result.latitude, result.longitude, days
    );
    let fc_body = http_get(&forecast_url)?;
    let fc: ForecastResponse =
        serde_json::from_str(&fc_body).map_err(|e| format!("failed to parse forecast response: {e}"))?;

    let mut out = String::new();
    out.push_str(&format!("Weather for {}\n", place_name));

    if let Some(current) = fc.current_weather {
        out.push_str(&format!(
            "Current: {}°C, {}, Wind {} km/h\n\n",
            current.temperature,
            wmo_description(current.weathercode),
            current.windspeed,
        ));
    }

    out.push_str("Forecast:\n");
    for i in 0..fc.daily.time.len() {
        let date = &fc.daily.time[i];
        let max = fc.daily.temperature_2m_max[i];
        let min = fc.daily.temperature_2m_min[i];
        let code = fc.daily.weathercode[i];
        let precip = fc.daily.precipitation_sum[i];
        let wind = fc.daily.wind_speed_10m_max[i];
        out.push_str(&format!(
            "{}: {}°C / {}°C — {}, Precipitation {} mm, Wind {} km/h\n",
            date, max, min, wmo_description(code), precip, wind,
        ));
    }

    Ok(out)
}
