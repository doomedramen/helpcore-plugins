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

struct CountryInfo;

impl Guest for CountryInfo {
    fn call(tool: String, input_json: String) -> Result<String, String> {
        let input: Value = serde_json::from_str(&input_json)
            .map_err(|e| format!("invalid input JSON: {e}"))?;
        match tool.as_str() {
            "country_lookup" => country_lookup(&input),
            _ => Err(format!("unknown tool: {tool}")),
        }
    }
}

export!(CountryInfo);

fn get_str<'a>(input: &'a Value, key: &str) -> Result<&'a str, String> {
    input.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
        .ok_or_else(|| format!("{key} is required"))
}

#[derive(Serialize)]
struct HttpReq<'a> {
    method: &'a str, url: String,
    headers: serde_json::Map<String, Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    body: Option<String>,
}

#[derive(Deserialize)]
struct HttpResp { status: u16, body: String }

fn http_get(url: &str) -> Result<String, String> {
    let req = HttpReq { method: "GET", url: url.to_string(), headers: serde_json::Map::new(), body: None };
    let req_json = serde_json::to_string(&req).map_err(|e| format!("serialize: {e}"))?;
    let resp_json = host::http_request(&req_json)?;
    let resp: HttpResp = serde_json::from_str(&resp_json).map_err(|e| format!("parse HTTP: {e}"))?;
    if resp.status >= 400 { return Err(format!("HTTP {}: {}", resp.status, resp.body)); }
    Ok(resp.body)
}

fn country_lookup(input: &Value) -> Result<String, String> {
    let country = get_str(input, "country")?;
    let url = format!("https://restcountries.com/v3.1/name/{}?fullText=false", country.replace(' ', "%20"));
    let body = match http_get(&url) {
        Ok(b) => b,
        Err(_) => {
            // Try by code
            let code_url = format!("https://restcountries.com/v3.1/alpha/{}", country.to_lowercase());
            http_get(&code_url).map_err(|_| format!("Country '{}' not found.", country))?
        }
    };

    let data: Vec<Value> = serde_json::from_str(&body)
        .map_err(|_| format!("No country found for '{}'", country))?;

    if data.is_empty() {
        return Err(format!("No country found for '{}'", country));
    }

    let c = &data[0];
    let name = c.get("name").and_then(|n| n.get("common")).and_then(Value::as_str).unwrap_or("?");
    let official = c.get("name").and_then(|n| n.get("official")).and_then(Value::as_str).unwrap_or("");
    let capital = c.get("capital").and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")).unwrap_or_default();
    let region = c.get("region").and_then(Value::as_str).unwrap_or("");
    let subregion = c.get("subregion").and_then(Value::as_str).unwrap_or("");
    let population = c.get("population").and_then(Value::as_u64).unwrap_or(0);
    let area = c.get("area").and_then(Value::as_f64).unwrap_or(0.0);

    let currencies = c.get("currencies").and_then(Value::as_object).map(|obj| {
        obj.values().map(|v| {
            let name = v.get("name").and_then(Value::as_str).unwrap_or("");
            let symbol = v.get("symbol").and_then(Value::as_str).unwrap_or("");
            if symbol.is_empty() { name.to_string() } else { format!("{name} ({symbol})") }
        }).collect::<Vec<_>>().join(", ")
    }).unwrap_or_default();

    let languages = c.get("languages").and_then(Value::as_object).map(|obj| {
        obj.values().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")
    }).unwrap_or_default();

    let flag = c.get("flags").and_then(|f| f.get("png")).and_then(Value::as_str).unwrap_or("");
    let demonym = c.get("demonyms").and_then(|d| d.get("eng")).and_then(|d| d.get("m")).and_then(Value::as_str).unwrap_or("");

    let timezones = c.get("timezones").and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")).unwrap_or_default();

    let borders: String = c.get("borders").and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "))
        .unwrap_or_else(|| "none".to_string());

    let cca2 = c.get("cca2").and_then(Value::as_str).unwrap_or("");
    let cca3 = c.get("cca3").and_then(Value::as_str).unwrap_or("");

    Ok(format!(
        "**{official}** ({name})\n\
         Capital: {capital}\n\
         Region: {subregion} ({region})\n\
         Population: {population}\n\
         Area: {area:.0} km²\n\
         Currency: {currencies}\n\
         Languages: {languages}\n\
         Demonym: {demonym}\n\
         ISO Codes: {cca2}/{cca3}\n\
         Timezones: {timezones}\n\
         Borders: {borders}\n\
         Flag: {flag}",
        official = official, name = name, capital = capital,
        subregion = subregion, region = region,
        population = format_number(population),
        area = area,
        currencies = currencies,
        languages = languages,
        demonym = demonym,
        cca2 = cca2, cca3 = cca3,
        timezones = timezones,
        borders = borders,
        flag = flag
    ))
}

fn format_number(n: u64) -> String {
    let s = n.to_string();
    let len = s.len();
    let mut result = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (len - i) % 3 == 0 {
            result.push(',');
        }
        result.push(ch);
    }
    result
}
