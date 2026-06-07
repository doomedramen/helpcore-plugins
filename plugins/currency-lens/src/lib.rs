// Currency Lens plugin - powered by the Frankfurter API (ECB reference rates, no API key needed)
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

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

struct CurrencyLens;

impl Guest for CurrencyLens {
    fn call(tool: String, input_json: String) -> Result<String, String> {
        let input: Value = serde_json::from_str(&input_json)
            .map_err(|e| format!("invalid input JSON: {e}"))?;

        match tool.as_str() {
            "currency_convert" => currency_convert(&input),
            _ => Err(format!("unknown tool: {tool}")),
        }
    }
}

export!(CurrencyLens);

fn url_encode(s: &str) -> String {
    let mut result = String::new();
    for byte in s.bytes() {
        match byte {
            b'0'..=b'9' | b'A'..=b'Z' | b'a'..=b'z' | b'-' | b'_' | b'.' | b'~' | b',' => {
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

/// Currency codes are always three-letter ISO 4217 codes.
fn normalize_code(code: &str) -> Result<String, String> {
    let trimmed = code.trim();
    if trimmed.len() != 3 || !trimmed.chars().all(|c| c.is_ascii_alphabetic()) {
        return Err(format!(
            "'{trimmed}' doesn't look like a currency code — use a three-letter ISO 4217 code, e.g. 'USD', 'GBP', 'EUR'."
        ));
    }
    Ok(trimmed.to_uppercase())
}

fn unknown_currency_error(bad_codes: &[&str]) -> Result<String, String> {
    let (status, body) = http_get("https://api.frankfurter.dev/v1/currencies")?;
    if status >= 400 {
        return Err(format!(
            "One or more currency codes were not recognised: {}",
            bad_codes.join(", ")
        ));
    }
    let known: BTreeMap<String, String> =
        serde_json::from_str(&body).map_err(|e| format!("failed to parse currency list: {e}"))?;

    let mut msg = format!(
        "Currency code(s) not recognised: {}. ",
        bad_codes.join(", ")
    );
    msg.push_str(&format!(
        "Supported codes include: {} (and {} more).",
        known.keys().take(12).cloned().collect::<Vec<_>>().join(", "),
        known.len().saturating_sub(12)
    ));
    Err(msg)
}

#[derive(Deserialize)]
struct ConversionResponse {
    amount: f64,
    base: String,
    date: String,
    rates: BTreeMap<String, f64>,
}

fn currency_convert(input: &Value) -> Result<String, String> {
    let amount = input.get("amount").and_then(Value::as_f64).unwrap_or(1.0);
    if !amount.is_finite() || amount < 0.0 {
        return Err("amount must be a positive number".to_string());
    }

    let from_raw = input
        .get("from")
        .and_then(Value::as_str)
        .ok_or("from is required, e.g. 'USD'")?;
    let to_raw = input
        .get("to")
        .and_then(Value::as_str)
        .ok_or("to is required, e.g. 'GBP' or 'GBP,EUR,JPY'")?;

    let from = normalize_code(from_raw)?;
    let to_codes: Vec<String> = to_raw
        .split(',')
        .map(normalize_code)
        .collect::<Result<Vec<_>, _>>()?;
    if to_codes.is_empty() {
        return Err("to must contain at least one currency code".to_string());
    }
    let to_joined = to_codes.join(",");

    let url = format!(
        "https://api.frankfurter.dev/v1/latest?amount={}&base={}&symbols={}",
        amount,
        url_encode(&from),
        url_encode(&to_joined)
    );

    let (status, body) = http_get(&url)?;
    if status == 404 {
        let mut bad = vec![from.as_str()];
        bad.extend(to_codes.iter().map(String::as_str));
        return unknown_currency_error(&bad);
    }
    if status >= 400 {
        return Err(format!("Currency conversion service returned HTTP {status}: {body}"));
    }

    let conv: ConversionResponse =
        serde_json::from_str(&body).map_err(|e| format!("failed to parse exchange rate response: {e}"))?;

    if conv.rates.is_empty() {
        return Err("No exchange rates were returned for the requested currencies.".to_string());
    }

    let mut out = String::new();
    out.push_str(&format!(
        "{:.2} {} =\n",
        conv.amount, conv.base
    ));
    for (code, value) in &conv.rates {
        out.push_str(&format!("  {:.2} {}\n", value, code));
    }
    out.push_str(&format!(
        "\n(Reference rates published by the European Central Bank, dated {})",
        conv.date
    ));

    Ok(out)
}
