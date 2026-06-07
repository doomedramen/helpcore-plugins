// World Clock plugin - powered by timeapi.io (no API key needed)
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

struct WorldClock;

impl Guest for WorldClock {
    fn call(tool: String, input_json: String) -> Result<String, String> {
        let input: Value = serde_json::from_str(&input_json)
            .map_err(|e| format!("invalid input JSON: {e}"))?;

        match tool.as_str() {
            "clock_now" => clock_now(&input),
            "clock_convert" => clock_convert(&input),
            _ => Err(format!("unknown tool: {tool}")),
        }
    }
}

export!(WorldClock);

fn url_encode(s: &str) -> String {
    let mut result = String::new();
    for byte in s.bytes() {
        match byte {
            b'0'..=b'9' | b'A'..=b'Z' | b'a'..=b'z' | b'-' | b'_' | b'.' | b'~' => {
                result.push(byte as char);
            }
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

fn http_call(method: &str, url: &str, body: Option<Value>) -> Result<String, String> {
    let mut headers = serde_json::Map::new();
    headers.insert("Accept".into(), Value::String("application/json".into()));
    if body.is_some() {
        headers.insert("Content-Type".into(), Value::String("application/json".into()));
    }

    let req = HttpRequest {
        method,
        url: url.to_string(),
        headers,
        body: body.map(|b| serde_json::to_string(&b).unwrap_or_default()),
    };
    let req_json =
        serde_json::to_string(&req).map_err(|e| format!("failed to serialize request: {e}"))?;
    let resp_json = host::http_request(&req_json)?;
    let resp: HttpResponse =
        serde_json::from_str(&resp_json).map_err(|e| format!("failed to parse HTTP response: {e}"))?;

    if resp.status == 400 {
        return Err(format!(
            "timeapi.io rejected the request — check that the timezone names and date/time are valid. ({})",
            resp.body.trim()
        ));
    }
    if resp.status >= 400 {
        return Err(format!("timeapi.io returned HTTP {}: {}", resp.status, resp.body));
    }
    Ok(resp.body)
}

/// Reject values that could escape the intended URL path or break out of the
/// JSON request body. IANA timezone names only ever contain these characters.
fn validate_timezone(tz: &str) -> Result<(), String> {
    let ok = !tz.is_empty()
        && tz.len() <= 64
        && tz.chars().all(|c| c.is_ascii_alphanumeric() || c == '/' || c == '_' || c == '-' || c == '+');
    if !ok {
        return Err(format!(
            "'{tz}' doesn't look like a valid IANA timezone name (e.g. 'Europe/London', 'America/New_York')."
        ));
    }
    Ok(())
}

/// Normalise a user-supplied date/time into the "YYYY-MM-DD HH:MM:SS" format
/// that timeapi.io's conversion endpoint expects (it rejects ISO8601 'T'
/// separators, trailing 'Z'/offsets, and missing seconds).
fn normalize_datetime(input: &str) -> Result<String, String> {
    let mut s = input.trim().replace('T', " ");

    // Strip a trailing 'Z' or numeric UTC offset (e.g. "+01:00", "-0500").
    if s.ends_with('Z') || s.ends_with('z') {
        s.pop();
    } else if let Some(plus) = s.rfind(['+', '-']) {
        // Only treat it as an offset if it appears after a space (i.e. in the
        // time portion, not the date's hyphens) and looks like H[H][:MM].
        if let Some(space) = s.find(' ') {
            if plus > space {
                let candidate = &s[plus + 1..];
                if candidate.len() <= 6
                    && candidate.chars().all(|c| c.is_ascii_digit() || c == ':')
                {
                    s.truncate(plus);
                }
            }
        }
    }

    let s = s.trim().to_string();
    let parts: Vec<&str> = s.splitn(2, ' ').collect();
    if parts.len() != 2 {
        return Err(format!(
            "'{input}' doesn't look like a date and time. Use the format 'YYYY-MM-DD HH:MM' (24-hour), e.g. '2026-06-07 15:00'."
        ));
    }
    let (date, time) = (parts[0], parts[1]);

    let time = match time.matches(':').count() {
        0 => format!("{time}:00:00"),
        1 => format!("{time}:00"),
        _ => time.to_string(),
    };

    Ok(format!("{date} {time}"))
}

#[derive(Deserialize)]
struct CurrentTime {
    date: String,
    time: String,
    #[serde(rename = "dayOfWeek")]
    day_of_week: String,
    #[serde(rename = "timeZone")]
    time_zone: String,
    #[serde(rename = "dstActive")]
    dst_active: bool,
}

fn clock_now(input: &Value) -> Result<String, String> {
    let timezone = input
        .get("timezone")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("UTC");
    validate_timezone(timezone)?;

    let url = format!(
        "https://timeapi.io/api/time/current/zone?timeZone={}",
        url_encode(timezone)
    );
    let body = http_call("GET", &url, None)?;
    let now: CurrentTime =
        serde_json::from_str(&body).map_err(|e| format!("failed to parse timeapi.io response: {e}"))?;

    let dst_note = if now.dst_active { " (daylight saving time is in effect)" } else { "" };
    Ok(format!(
        "It is currently {} on {}, {} in {}{}.",
        now.time, now.day_of_week, now.date, now.time_zone, dst_note
    ))
}

#[derive(Deserialize)]
struct ConvertResult {
    #[serde(rename = "fromTimezone")]
    from_timezone: String,
    #[serde(rename = "fromDateTime")]
    from_date_time: String,
    #[serde(rename = "toTimeZone")]
    to_timezone: String,
    #[serde(rename = "conversionResult")]
    result: ConvertedTime,
}

#[derive(Deserialize)]
struct ConvertedTime {
    date: String,
    time: String,
    #[serde(rename = "dstActive")]
    dst_active: bool,
}

fn clock_convert(input: &Value) -> Result<String, String> {
    let time = input
        .get("time")
        .and_then(Value::as_str)
        .ok_or("time is required, e.g. '2026-06-07 15:00'")?;
    let from_tz = input
        .get("from_timezone")
        .and_then(Value::as_str)
        .ok_or("from_timezone is required, e.g. 'Europe/London'")?
        .trim();
    let to_tz = input
        .get("to_timezone")
        .and_then(Value::as_str)
        .ok_or("to_timezone is required, e.g. 'Asia/Tokyo'")?
        .trim();
    validate_timezone(from_tz)?;
    validate_timezone(to_tz)?;

    let normalized = normalize_datetime(time)?;

    let body = serde_json::json!({
        "fromTimeZone": from_tz,
        "dateTime": normalized,
        "toTimeZone": to_tz,
        "dstAmbiguity": "",
    });

    let resp_body = http_call(
        "POST",
        "https://timeapi.io/api/conversion/converttimezone",
        Some(body),
    )?;
    let conv: ConvertResult = serde_json::from_str(&resp_body)
        .map_err(|e| format!("failed to parse timeapi.io response: {e}"))?;

    let dst_note = if conv.result.dst_active {
        " (daylight saving time is in effect there)"
    } else {
        ""
    };

    Ok(format!(
        "{} in {} is {} {} in {}{}.",
        conv.from_date_time.replace('T', " at "),
        conv.from_timezone,
        conv.result.date,
        conv.result.time,
        conv.to_timezone,
        dst_note
    ))
}
