// Net Identity plugin - powered by ipinfo.io (no API key needed for basic lookups)
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

struct NetIdentity;

impl Guest for NetIdentity {
    fn call(tool: String, input_json: String) -> Result<String, String> {
        let input: Value = serde_json::from_str(&input_json)
            .map_err(|e| format!("invalid input JSON: {e}"))?;

        match tool.as_str() {
            "net_whoami" => net_whoami(&input),
            _ => Err(format!("unknown tool: {tool}")),
        }
    }
}

export!(NetIdentity);

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
    if resp.status == 404 {
        return Err("That IP address could not be found.".to_string());
    }
    if resp.status == 429 {
        return Err("ipinfo.io rate limit reached — please try again shortly.".to_string());
    }
    if resp.status >= 400 {
        return Err(format!("ipinfo.io returned HTTP {}: {}", resp.status, resp.body));
    }
    Ok(resp.body)
}

#[derive(Deserialize)]
struct IpInfo {
    ip: Option<String>,
    hostname: Option<String>,
    city: Option<String>,
    region: Option<String>,
    country: Option<String>,
    loc: Option<String>,
    org: Option<String>,
    postal: Option<String>,
    timezone: Option<String>,
    bogon: Option<bool>,
}

/// Reject values that could escape the intended URL path via injection or
/// path traversal. IPv4/IPv6 addresses only contain these characters.
fn validate_ip(ip: &str) -> Result<(), String> {
    let ok = !ip.is_empty()
        && ip.len() <= 64
        && ip.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == ':');
    if !ok {
        return Err(format!("'{ip}' does not look like a valid IP address."));
    }
    Ok(())
}

fn net_whoami(input: &Value) -> Result<String, String> {
    let ip = input.get("ip").and_then(Value::as_str).map(str::trim);

    let url = match ip {
        Some(addr) if !addr.is_empty() => {
            validate_ip(addr)?;
            format!("https://ipinfo.io/{addr}/json")
        }
        _ => "https://ipinfo.io/json".to_string(),
    };

    let body = http_get(&url)?;
    let info: IpInfo =
        serde_json::from_str(&body).map_err(|e| format!("failed to parse ipinfo.io response: {e}"))?;

    if info.bogon == Some(true) {
        return Ok(format!(
            "{} is a bogon address (private/reserved range) — it has no public geolocation.",
            info.ip.as_deref().unwrap_or("That address")
        ));
    }

    let mut out = String::new();
    let label = match ip {
        Some(addr) if !addr.is_empty() => format!("IP lookup for {addr}"),
        _ => "Your public IP address".to_string(),
    };
    out.push_str(&label);
    out.push('\n');

    if let Some(ref ip) = info.ip {
        out.push_str(&format!("IP address: {ip}\n"));
    }
    if let Some(ref host) = info.hostname {
        out.push_str(&format!("Hostname: {host}\n"));
    }

    let place_parts: Vec<&str> = [info.city.as_deref(), info.region.as_deref(), info.country.as_deref()]
        .into_iter()
        .flatten()
        .collect();
    if !place_parts.is_empty() {
        out.push_str(&format!("Location: {}\n", place_parts.join(", ")));
    }
    if let Some(ref postal) = info.postal {
        out.push_str(&format!("Postal code: {postal}\n"));
    }
    if let Some(ref loc) = info.loc {
        out.push_str(&format!("Coordinates: {loc}\n"));
    }
    if let Some(ref tz) = info.timezone {
        out.push_str(&format!("Timezone: {tz}\n"));
    }
    if let Some(ref org) = info.org {
        out.push_str(&format!("Network/ISP: {org}\n"));
    }

    out.push_str("\n(Location is approximate, derived from the IP address — not GPS.)");

    Ok(out)
}
