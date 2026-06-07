// DNS Peek plugin - powered by Cloudflare's DNS-over-HTTPS resolver (no API key needed)
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

struct DnsPeek;

impl Guest for DnsPeek {
    fn call(tool: String, input_json: String) -> Result<String, String> {
        let input: Value = serde_json::from_str(&input_json)
            .map_err(|e| format!("invalid input JSON: {e}"))?;

        match tool.as_str() {
            "dns_lookup" => dns_lookup(&input),
            _ => Err(format!("unknown tool: {tool}")),
        }
    }
}

export!(DnsPeek);

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

fn http_get(url: &str) -> Result<(u16, String), String> {
    let mut headers = serde_json::Map::new();
    headers.insert("Accept".into(), Value::String("application/dns-json".into()));

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

/// Reasonable validation for a domain name: ASCII letters/digits/hyphens/dots only.
fn validate_domain(domain: &str) -> Result<String, String> {
    let trimmed = domain.trim().trim_end_matches('.');
    let ok = !trimmed.is_empty()
        && trimmed.len() <= 253
        && trimmed
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '_');
    if !ok {
        return Err(format!("'{trimmed}' doesn't look like a valid domain name."));
    }
    Ok(trimmed.to_string())
}

fn record_type_name(code: u16) -> String {
    match code {
        1 => "A".to_string(),
        2 => "NS".to_string(),
        5 => "CNAME".to_string(),
        6 => "SOA".to_string(),
        12 => "PTR".to_string(),
        15 => "MX".to_string(),
        16 => "TXT".to_string(),
        17 => "RP".to_string(),
        18 => "AFSDB".to_string(),
        24 => "SIG".to_string(),
        25 => "KEY".to_string(),
        28 => "AAAA".to_string(),
        33 => "SRV".to_string(),
        35 => "NAPTR".to_string(),
        36 => "KX".to_string(),
        37 => "CERT".to_string(),
        39 => "DNAME".to_string(),
        43 => "DS".to_string(),
        44 => "SSHFP".to_string(),
        46 => "RRSIG".to_string(),
        47 => "NSEC".to_string(),
        48 => "DNSKEY".to_string(),
        52 => "TLSA".to_string(),
        257 => "CAA".to_string(),
        other => format!("TYPE{other}"),
    }
}

fn rcode_meaning(status: u16) -> Option<&'static str> {
    match status {
        0 => None, // NOERROR — nothing to explain
        1 => Some("the resolver could not interpret the query (format error)"),
        2 => Some("the authoritative server failed to answer (server failure)"),
        3 => Some("the domain does not exist (NXDOMAIN)"),
        5 => Some("the query was refused by the resolver"),
        _ => Some("the resolver returned a non-standard response code"),
    }
}

#[derive(Deserialize)]
struct DohResponse {
    #[serde(rename = "Status")]
    status: u16,
    #[serde(rename = "Answer", default)]
    answer: Vec<DohRecord>,
}

#[derive(Deserialize)]
struct DohRecord {
    #[serde(rename = "type")]
    rtype: u16,
    #[serde(rename = "TTL")]
    ttl: u32,
    data: String,
}

fn dns_lookup(input: &Value) -> Result<String, String> {
    let domain_raw = input
        .get("domain")
        .and_then(Value::as_str)
        .ok_or("domain is required, e.g. 'example.com'")?;
    let domain = validate_domain(domain_raw)?;

    let record_type = input
        .get("type")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("A")
        .to_uppercase();
    if !record_type.chars().all(|c| c.is_ascii_alphanumeric()) || record_type.len() > 10 {
        return Err(format!("'{record_type}' doesn't look like a valid DNS record type."));
    }

    let url = format!(
        "https://cloudflare-dns.com/dns-query?name={}&type={}",
        url_encode(&domain),
        url_encode(&record_type)
    );
    let (status, body) = http_get(&url)?;
    if status == 400 {
        return Err(format!(
            "'{record_type}' isn't a recognised DNS record type. Try A, AAAA, MX, TXT, NS, CNAME, SOA, or CAA."
        ));
    }
    if status >= 400 {
        return Err(format!("DNS lookup failed (HTTP {status}): {body}"));
    }

    let doh: DohResponse =
        serde_json::from_str(&body).map_err(|e| format!("failed to parse DNS response: {e}"))?;

    if let Some(meaning) = rcode_meaning(doh.status) {
        return Ok(format!(
            "DNS lookup for {domain} ({record_type}): {meaning}."
        ));
    }

    if doh.answer.is_empty() {
        return Ok(format!(
            "{domain} has no {record_type} records published."
        ));
    }

    let mut out = String::new();
    out.push_str(&format!("{record_type} records for {domain}:"));
    for rec in &doh.answer {
        out.push_str(&format!(
            "\n  {} (type {}, TTL {}s)",
            rec.data.trim(),
            record_type_name(rec.rtype),
            rec.ttl
        ));
    }
    Ok(out)
}
