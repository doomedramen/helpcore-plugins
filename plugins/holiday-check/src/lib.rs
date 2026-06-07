// Holiday Check plugin - powered by the Nager.Date public holiday API (no API key needed)
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

struct HolidayCheck;

impl Guest for HolidayCheck {
    fn call(tool: String, input_json: String) -> Result<String, String> {
        let input: Value = serde_json::from_str(&input_json)
            .map_err(|e| format!("invalid input JSON: {e}"))?;

        match tool.as_str() {
            "holiday_list" => holiday_list(&input),
            "holiday_next" => holiday_next(&input),
            _ => Err(format!("unknown tool: {tool}")),
        }
    }
}

export!(HolidayCheck);

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

/// ISO 3166-1 alpha-2 country codes are exactly two ASCII letters.
fn normalize_country(code: &str) -> Result<String, String> {
    let trimmed = code.trim();
    if trimmed.len() != 2 || !trimmed.chars().all(|c| c.is_ascii_alphabetic()) {
        return Err(format!(
            "'{trimmed}' doesn't look like a country code — use a two-letter ISO 3166-1 code, e.g. 'GB', 'US', 'DE', 'JP'."
        ));
    }
    Ok(trimmed.to_uppercase())
}

fn unknown_country_error(country: &str, status: u16, body: &str) -> String {
    if status == 404 {
        format!(
            "'{country}' isn't a recognised country code. Use a two-letter ISO 3166-1 code, e.g. 'GB', 'US', 'DE', 'JP', 'AU'."
        )
    } else {
        format!("Holiday lookup failed (HTTP {status}): {body}")
    }
}

#[derive(Deserialize)]
struct Holiday {
    date: String,
    #[serde(rename = "localName")]
    local_name: String,
    name: String,
    #[serde(default)]
    counties: Option<Vec<String>>,
    #[serde(default)]
    global: bool,
}

fn render_holidays(holidays: &[Holiday], header: &str, empty_msg: &str) -> String {
    if holidays.is_empty() {
        return empty_msg.to_string();
    }

    let mut out = String::new();
    out.push_str(header);
    for h in holidays {
        out.push_str(&format!("\n  {} — {}", h.date, h.name));
        if h.local_name != h.name {
            out.push_str(&format!(" (locally: {})", h.local_name));
        }
        if !h.global {
            match &h.counties {
                Some(counties) if !counties.is_empty() => {
                    out.push_str(&format!(" [regional: {}]", counties.join(", ")));
                }
                _ => out.push_str(" [regional]"),
            }
        }
    }
    out
}

fn holiday_list(input: &Value) -> Result<String, String> {
    let country_raw = input
        .get("country")
        .and_then(Value::as_str)
        .ok_or("country is required, e.g. 'GB'")?;
    let country = normalize_country(country_raw)?;

    let year = input
        .get("year")
        .and_then(Value::as_u64)
        .ok_or("year is required, e.g. 2026")?;
    if !(1975..=2200).contains(&year) {
        return Err(format!("'{year}' is outside the supported range (1975–2200)."));
    }

    let url = format!("https://date.nager.at/api/v3/PublicHolidays/{year}/{country}");
    let (status, body) = http_get(&url)?;
    if status >= 400 {
        return Err(unknown_country_error(&country, status, &body));
    }

    let holidays: Vec<Holiday> =
        serde_json::from_str(&body).map_err(|e| format!("failed to parse holiday response: {e}"))?;

    Ok(render_holidays(
        &holidays,
        &format!("Public holidays in {country} for {year}:"),
        &format!("No public holidays found for {country} in {year}."),
    ))
}

fn holiday_next(input: &Value) -> Result<String, String> {
    let country_raw = input
        .get("country")
        .and_then(Value::as_str)
        .ok_or("country is required, e.g. 'GB'")?;
    let country = normalize_country(country_raw)?;

    let url = format!("https://date.nager.at/api/v3/NextPublicHolidays/{country}");
    let (status, body) = http_get(&url)?;
    if status >= 400 {
        return Err(unknown_country_error(&country, status, &body));
    }

    let holidays: Vec<Holiday> =
        serde_json::from_str(&body).map_err(|e| format!("failed to parse holiday response: {e}"))?;

    Ok(render_holidays(
        &holidays,
        &format!("Upcoming public holidays in {country}:"),
        &format!("No upcoming public holidays found for {country}."),
    ))
}
