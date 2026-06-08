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

struct Finance;

impl Guest for Finance {
    fn call(tool: String, input_json: String) -> Result<String, String> {
        let input: Value = serde_json::from_str(&input_json)
            .map_err(|e| format!("invalid input JSON: {e}"))?;

        match tool.as_str() {
            "stock_quote" => stock_quote(&input),
            "crypto_quote" => crypto_quote(&input),
            _ => Err(format!("unknown tool: {tool}")),
        }
    }
}

export!(Finance);

fn get_str<'a>(input: &'a Value, key: &str) -> Result<&'a str, String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("{key} is required"))
}

fn get_str_opt<'a>(input: &'a Value, key: &str) -> Option<&'a str> {
    input.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
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

fn stock_quote(input: &Value) -> Result<String, String> {
    let symbol = get_str(input, "symbol")?.to_uppercase();
    let url = format!(
        "https://query1.finance.yahoo.com/v8/finance/chart/{symbol}?interval=1d&range=1d"
    );
    let body = http_get(&url)?;

    let data: Value = serde_json::from_str(&body)
        .map_err(|_| format!("Could not find stock symbol '{symbol}'. Check the ticker and try again."))?;

    let result = data.get("chart").and_then(|c| c.get("result"))
        .and_then(|r| r.get(0))
        .ok_or_else(|| format!("No data found for symbol '{symbol}'"))?;

    let meta = result.get("meta").ok_or("no meta data in response")?;

    let price = meta.get("regularMarketPrice")
        .and_then(Value::as_f64)
        .ok_or("could not find price")?;

    let prev_close = meta.get("previousClose")
        .and_then(Value::as_f64)
        .or(meta.get("chartPreviousClose").and_then(Value::as_f64));

    let currency = meta.get("currency")
        .and_then(Value::as_str)
        .unwrap_or("USD");

    let name = meta.get("shortName")
        .or(meta.get("longName"))
        .and_then(Value::as_str)
        .unwrap_or(&symbol);

    let mut out = format!("{name} ({symbol}): {currency} {price:.2}");

    if let Some(prev) = prev_close {
        let change = price - prev;
        let pct = (change / prev) * 100.0;
        let arrow = if change >= 0.0 { "▲" } else { "▼" };
        out.push_str(&format!("  {arrow} {change:+.2} ({pct:+.2}%) today"));
    }

    Ok(out)
}

fn crypto_quote(input: &Value) -> Result<String, String> {
    let coin = get_str(input, "coin")?.to_lowercase();
    let currency = get_str_opt(input, "currency").unwrap_or("usd").to_lowercase();

    // Try to resolve coin name/symbol to CoinGecko ID
    let coin_id = if coin.len() <= 6 && coin.chars().all(|c| c.is_alphabetic()) {
        // Could be a symbol - search
        match search_crypto(&coin) {
            Ok(id) => id,
            Err(_) => coin.clone(),
        }
    } else {
        coin.clone()
    };

    let url = format!(
        "https://api.coingecko.com/api/v3/simple/price?ids={coin_id}&vs_currencies={currency}&include_24hr_change=true"
    );
    let body = http_get(&url)?;

    let data: Value = serde_json::from_str(&body)
        .map_err(|_| format!("Could not find cryptocurrency '{coin}'. Try using the full name (e.g. 'bitcoin' instead of 'btc')."))?;

    let coin_data = data.get(&coin_id).ok_or_else(|| format!("Unknown cryptocurrency: {coin}"))?;

    let price = coin_data.get(&currency)
        .and_then(Value::as_f64)
        .ok_or_else(|| format!("Price not available for {currency}"))?;

    let amount = if price < 1.0 { format!("{price:.6}") } else { format!("{price:.2}") };

    let currency_upper = currency.to_uppercase();
    let mut out = format!("{} ({coin_id}): {currency_upper} {amount}", capitalize(&coin_id));

    if let Some(change) = coin_data.get(format!("{currency}_24h_change")).and_then(Value::as_f64) {
        let arrow = if change >= 0.0 { "▲" } else { "▼" };
        out.push_str(&format!("  {arrow} {change:+.2}% 24h"));
    }

    Ok(out)
}

fn search_crypto(symbol: &str) -> Result<String, String> {
    let url = format!("https://api.coingecko.com/api/v3/search?query={symbol}");
    let body = http_get(&url)?;
    let data: Value = serde_json::from_str(&body)
        .map_err(|e| format!("search failed: {e}"))?;

    let coins = data.get("coins").and_then(Value::as_array)
        .ok_or("no search results")?;

    // Match exact symbol (case-insensitive)
    let symbol_upper = symbol.to_uppercase();
    for coin in coins {
        if let Some(sym) = coin.get("symbol").and_then(Value::as_str) {
            if sym.to_uppercase() == symbol_upper {
                return coin.get("id")
                    .and_then(Value::as_str)
                    .map(|s| s.to_string())
                    .ok_or_else(|| "no id in search result".to_string());
            }
        }
    }

    // Fallback: first result
    coins.first()
        .and_then(|c| c.get("id"))
        .and_then(Value::as_str)
        .map(|s| s.to_string())
        .ok_or_else(|| format!("Could not find cryptocurrency for symbol '{symbol}'"))
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().to_string() + chars.as_str(),
        None => String::new(),
    }
}
