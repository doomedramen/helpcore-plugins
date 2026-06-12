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
            secret-read: func(key: string) -> result<string, string>;
        }

        world plugin {
            import host;
            export call: func(tool: string, input-json: string) -> result<string, string>;
        }
    "#,
    world: "plugin",
});

use helpcore::plugin::host;

struct WebSearch;

impl Guest for WebSearch {
    fn call(tool: String, input_json: String) -> Result<String, String> {
        let input: Value =
            serde_json::from_str(&input_json).map_err(|e| format!("invalid input JSON: {e}"))?;

        match tool.as_str() {
            "web_search" => web_search(&input),
            _ => Err(format!("unknown tool: {tool}")),
        }
    }
}

export!(WebSearch);

const BRAVE_API_URL: &str = "https://api.search.brave.com/res/v1/web/search";
const DEFAULT_RESULTS: usize = 5;
const MAX_RESULTS: usize = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Provider {
    Brave,
    Searxng,
    Whoogle,
}

impl Provider {
    fn from_config(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "" | "brave" => Ok(Self::Brave),
            "searxng" => Ok(Self::Searxng),
            "whoogle" => Ok(Self::Whoogle),
            value => Err(format!(
                "unsupported search provider \"{value}\"; choose brave, searxng, or whoogle"
            )),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Brave => "Brave Search",
            Self::Searxng => "SearXNG",
            Self::Whoogle => "Whoogle",
        }
    }
}

struct SearchInput {
    query: String,
    max_results: usize,
    freshness: Option<String>,
}

struct ProviderConfig {
    provider: Provider,
    base_url: Option<String>,
    api_key: Option<String>,
}

#[derive(Debug)]
struct SearchResult {
    title: String,
    url: String,
    snippet: String,
}

#[derive(Debug, Serialize)]
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

#[derive(Deserialize)]
struct BraveResponse {
    web: Option<BraveWebResults>,
}

#[derive(Deserialize)]
struct BraveWebResults {
    #[serde(default)]
    results: Vec<BraveResult>,
}

#[derive(Deserialize)]
struct BraveResult {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Deserialize)]
struct SearxngResponse {
    #[serde(default)]
    results: Vec<SearxngResult>,
}

#[derive(Deserialize)]
struct SearxngResult {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    content: Option<String>,
}

#[derive(Deserialize)]
struct WhoogleResponse {
    #[serde(default)]
    blocked: bool,
    #[serde(default)]
    error_message: Option<String>,
    #[serde(default)]
    results: Vec<WhoogleResult>,
}

#[derive(Deserialize)]
struct WhoogleResult {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    href: Option<String>,
    #[serde(default)]
    content: Option<String>,
}

fn parse_search_input(input: &Value) -> Result<SearchInput, String> {
    let query = input
        .get("query")
        .and_then(Value::as_str)
        .ok_or("query is required, e.g. 'rust programming language'")?
        .trim();

    if query.is_empty() {
        return Err("query must not be empty".to_string());
    }

    let max_results = input
        .get("max_results")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_RESULTS as u64)
        .clamp(1, MAX_RESULTS as u64) as usize;

    let freshness = input
        .get("freshness")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| match value {
            "day" | "week" | "month" | "year" => Ok(value.to_string()),
            _ => Err("freshness must be one of: day, week, month, year".to_string()),
        })
        .transpose()?;

    Ok(SearchInput {
        query: query.to_string(),
        max_results,
        freshness,
    })
}

fn read_optional_config(key: &str) -> Option<String> {
    host::config_read(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn get_brave_api_key() -> Result<String, String> {
    let key = host::secret_read("api_key").map_err(|e| {
        if e.contains("not approved") {
            e
        } else {
            "Brave Search API key is not configured. Create one at https://brave.com/search/api/ and add it to the plugin settings."
                .to_string()
        }
    })?;

    let key = key.trim();
    if key.is_empty() {
        return Err(
            "Brave Search API key is empty. Add a valid key to the plugin settings.".to_string(),
        );
    }

    Ok(key.to_string())
}

fn normalize_base_url(value: Option<String>, provider: Provider) -> Result<String, String> {
    let value = value.ok_or_else(|| {
        format!(
            "{} base URL is not configured. Set it in plugin settings.",
            provider.label()
        )
    })?;
    let value = value.trim().trim_end_matches('/');

    if !(value.starts_with("http://") || value.starts_with("https://")) {
        return Err(format!(
            "{} base URL must start with http:// or https://",
            provider.label()
        ));
    }
    if value.contains('?') || value.contains('#') {
        return Err(format!(
            "{} base URL must not contain a query string or fragment",
            provider.label()
        ));
    }

    Ok(value.to_string())
}

fn load_provider_config() -> Result<ProviderConfig, String> {
    let provider = Provider::from_config(
        read_optional_config("provider")
            .as_deref()
            .unwrap_or("brave"),
    )?;

    match provider {
        Provider::Brave => Ok(ProviderConfig {
            provider,
            base_url: None,
            api_key: Some(get_brave_api_key()?),
        }),
        Provider::Searxng | Provider::Whoogle => Ok(ProviderConfig {
            provider,
            base_url: Some(normalize_base_url(
                read_optional_config("base_url"),
                provider,
            )?),
            api_key: None,
        }),
    }
}

fn url_encode(input: &str) -> String {
    let mut encoded = String::new();
    for byte in input.bytes() {
        match byte {
            b'0'..=b'9' | b'A'..=b'Z' | b'a'..=b'z' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

fn brave_freshness(value: &str) -> &'static str {
    match value {
        "day" => "pd",
        "week" => "pw",
        "month" => "pm",
        "year" => "py",
        _ => unreachable!("freshness is validated before building the request"),
    }
}

fn build_request(
    config: &ProviderConfig,
    input: &SearchInput,
) -> Result<HttpRequest<'static>, String> {
    let mut headers = serde_json::Map::new();
    headers.insert("Accept".into(), Value::String("application/json".into()));
    headers.insert(
        "User-Agent".into(),
        Value::String("helpcore-web-search/0.2".into()),
    );

    let url = match config.provider {
        Provider::Brave => {
            headers.insert(
                "X-Subscription-Token".into(),
                Value::String(config.api_key.clone().unwrap_or_default()),
            );
            let mut url = format!(
                "{BRAVE_API_URL}?q={}&count={}&safesearch=moderate",
                url_encode(&input.query),
                input.max_results
            );
            if let Some(freshness) = input.freshness.as_deref() {
                url.push_str("&freshness=");
                url.push_str(brave_freshness(freshness));
            }
            url
        }
        Provider::Searxng => {
            let mut url = format!(
                "{}/search?q={}&format=json&safesearch=1",
                config.base_url.as_deref().unwrap_or_default(),
                url_encode(&input.query)
            );
            if let Some(freshness) = input.freshness.as_deref() {
                if freshness == "week" {
                    return Err(
                        "SearXNG does not support a week freshness filter; use day or month."
                            .to_string(),
                    );
                }
                url.push_str("&time_range=");
                url.push_str(freshness);
            }
            url
        }
        Provider::Whoogle => {
            let query = match input.freshness.as_deref() {
                Some("week") => {
                    return Err(
                        "Whoogle does not support a week freshness filter; use day or month."
                            .to_string(),
                    )
                }
                Some(freshness) => format!("{} :past {freshness}", input.query),
                None => input.query.clone(),
            };
            format!(
                "{}/search?q={}&format=json",
                config.base_url.as_deref().unwrap_or_default(),
                url_encode(&query)
            )
        }
    };

    Ok(HttpRequest {
        method: "GET",
        url,
        headers,
        body: None,
    })
}

fn http_get(request: &HttpRequest<'_>) -> Result<HttpResponse, String> {
    let request_json = serde_json::to_string(request)
        .map_err(|e| format!("failed to serialize search request: {e}"))?;
    let response_json = host::http_request(&request_json)?;

    serde_json::from_str(&response_json)
        .map_err(|e| format!("failed to parse search HTTP response: {e}"))
}

fn error_detail(body: &str) -> Option<String> {
    let value: Value = serde_json::from_str(body).ok()?;
    let detail = [
        value.pointer("/error/detail"),
        value.get("error_message"),
        value.get("message"),
        value.get("detail"),
    ]
    .into_iter()
    .flatten()
    .find_map(Value::as_str)
    .map(str::to_string);
    detail
}

fn check_status(provider: Provider, response: &HttpResponse) -> Result<(), String> {
    if (200..=299).contains(&response.status) {
        return Ok(());
    }

    if provider == Provider::Brave && matches!(response.status, 401 | 403) {
        return Err(
            "Brave Search rejected the API key. Check the key and subscription in plugin settings."
                .to_string(),
        );
    }
    if provider == Provider::Searxng && response.status == 403 {
        return Err(
            "SearXNG rejected JSON search. Enable json under search.formats in settings.yml."
                .to_string(),
        );
    }
    if response.status == 429 {
        return Err(format!(
            "{} rate limit reached. Wait briefly before searching again.",
            provider.label()
        ));
    }

    let detail = error_detail(&response.body)
        .map(|value| format!(": {value}"))
        .unwrap_or_default();
    match response.status {
        400..=499 => Err(format!(
            "{} rejected the request (HTTP {}){detail}",
            provider.label(),
            response.status
        )),
        500..=599 => Err(format!(
            "{} is unavailable (HTTP {}){detail}",
            provider.label(),
            response.status
        )),
        _ => Err(format!(
            "{} returned an unexpected HTTP status: {}",
            provider.label(),
            response.status
        )),
    }
}

fn decode_entity(entity: &str) -> Option<char> {
    match entity {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" | "#39" => Some('\''),
        "nbsp" => Some(' '),
        _ => {
            let number = entity.strip_prefix('#')?;
            if let Some(hex) = number
                .strip_prefix('x')
                .or_else(|| number.strip_prefix('X'))
            {
                u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
            } else {
                number.parse::<u32>().ok().and_then(char::from_u32)
            }
        }
    }
}

fn clean_text(input: &str) -> String {
    let mut without_tags = String::with_capacity(input.len());
    let mut in_tag = false;
    for character in input.chars() {
        match character {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => without_tags.push(character),
            _ => {}
        }
    }

    let mut decoded = String::with_capacity(without_tags.len());
    let mut remaining = without_tags.as_str();
    while let Some(start) = remaining.find('&') {
        decoded.push_str(&remaining[..start]);
        let entity_start = start + 1;
        let after_ampersand = &remaining[entity_start..];
        if let Some(end) = after_ampersand.find(';').filter(|end| *end <= 10) {
            let entity = &after_ampersand[..end];
            if let Some(character) = decode_entity(entity) {
                decoded.push(character);
                remaining = &after_ampersand[end + 1..];
                continue;
            }
        }
        decoded.push('&');
        remaining = after_ampersand;
    }
    decoded.push_str(remaining);

    decoded.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn normalized_result(
    title: Option<String>,
    url: Option<String>,
    snippet: Option<String>,
) -> Option<SearchResult> {
    let url = url?.trim().to_string();
    if url.is_empty() {
        return None;
    }
    let title = title
        .as_deref()
        .map(clean_text)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| url.clone());
    let snippet = snippet.as_deref().map(clean_text).unwrap_or_default();

    Some(SearchResult {
        title,
        url,
        snippet,
    })
}

fn parse_results(
    provider: Provider,
    body: &str,
    max_results: usize,
) -> Result<Vec<SearchResult>, String> {
    let results: Vec<SearchResult> = match provider {
        Provider::Brave => {
            let response: BraveResponse = serde_json::from_str(body)
                .map_err(|e| format!("failed to parse Brave Search response: {e}"))?;
            response
                .web
                .map(|web| web.results)
                .unwrap_or_default()
                .into_iter()
                .filter_map(|result| {
                    normalized_result(result.title, result.url, result.description)
                })
                .collect()
        }
        Provider::Searxng => {
            let response: SearxngResponse = serde_json::from_str(body)
                .map_err(|e| format!("failed to parse SearXNG response: {e}"))?;
            response
                .results
                .into_iter()
                .filter_map(|result| normalized_result(result.title, result.url, result.content))
                .collect()
        }
        Provider::Whoogle => {
            let response: WhoogleResponse = serde_json::from_str(body)
                .map_err(|e| format!("failed to parse Whoogle response: {e}"))?;
            if response.blocked {
                return Err(format!(
                    "Whoogle was blocked by Google{}. Switch to SearXNG or Brave Search.",
                    response
                        .error_message
                        .map(|message| format!(": {message}"))
                        .unwrap_or_default()
                ));
            }
            response
                .results
                .into_iter()
                .filter_map(|result| {
                    normalized_result(result.title.or(result.text), result.href, result.content)
                })
                .collect()
        }
    };

    Ok(results.into_iter().take(max_results).collect())
}

fn format_results(query: &str, results: Vec<SearchResult>) -> String {
    if results.is_empty() {
        return format!("No results found for \"{query}\".");
    }

    let mut output = String::new();
    for (index, result) in results.iter().enumerate() {
        if index > 0 {
            output.push('\n');
        }

        output.push_str(&format!("{}. {}\n", index + 1, result.title));
        output.push_str(&format!("   {}\n", result.url));
        if !result.snippet.is_empty() {
            output.push_str(&format!("   {}\n", result.snippet));
        }
    }
    output
}

fn web_search(input: &Value) -> Result<String, String> {
    let input = parse_search_input(input)?;
    let config = load_provider_config()?;
    let request = build_request(&config, &input)?;
    let response = http_get(&request)?;
    check_status(config.provider, &response)?;
    let results = parse_results(config.provider, &response.body, input.max_results)?;
    Ok(format_results(&input.query, results))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn config(provider: Provider, base_url: Option<&str>) -> ProviderConfig {
        ProviderConfig {
            provider,
            base_url: base_url.map(str::to_string),
            api_key: (provider == Provider::Brave).then(|| "test-key".to_string()),
        }
    }

    #[test]
    fn builds_brave_url_with_operators_and_freshness() {
        let input = parse_search_input(&json!({
            "query": "site:example.com \"rust async\" -tokio",
            "max_results": 8,
            "freshness": "week"
        }))
        .unwrap();
        let request = build_request(&config(Provider::Brave, None), &input).unwrap();
        assert_eq!(
            request.url,
            "https://api.search.brave.com/res/v1/web/search?q=site%3Aexample.com%20%22rust%20async%22%20-tokio&count=8&safesearch=moderate&freshness=pw"
        );
        assert_eq!(
            request.headers.get("X-Subscription-Token"),
            Some(&Value::String("test-key".to_string()))
        );
    }

    #[test]
    fn builds_searxng_json_url() {
        let input = parse_search_input(&json!({
            "query": "rust wasm",
            "freshness": "month"
        }))
        .unwrap();
        let request = build_request(
            &config(Provider::Searxng, Some("http://searxng:8080")),
            &input,
        )
        .unwrap();
        assert_eq!(
            request.url,
            "http://searxng:8080/search?q=rust%20wasm&format=json&safesearch=1&time_range=month"
        );
    }

    #[test]
    fn builds_whoogle_json_url_with_freshness() {
        let input = parse_search_input(&json!({
            "query": "rust wasm",
            "freshness": "day"
        }))
        .unwrap();
        let request = build_request(
            &config(Provider::Whoogle, Some("http://whoogle:5000")),
            &input,
        )
        .unwrap();
        assert_eq!(
            request.url,
            "http://whoogle:5000/search?q=rust%20wasm%20%3Apast%20day&format=json"
        );
    }

    #[test]
    fn rejects_unsupported_week_filter() {
        let input = parse_search_input(&json!({
            "query": "test",
            "freshness": "week"
        }))
        .unwrap();
        let error = build_request(
            &config(Provider::Searxng, Some("http://searxng:8080")),
            &input,
        )
        .unwrap_err();
        assert!(error.contains("does not support a week"));
    }

    #[test]
    fn clamps_result_count() {
        let input = parse_search_input(&json!({
            "query": "test",
            "max_results": 100
        }))
        .unwrap();
        assert_eq!(input.max_results, 10);
    }

    #[test]
    fn rejects_invalid_freshness() {
        let error = parse_search_input(&json!({
            "query": "test",
            "freshness": "hour"
        }))
        .err()
        .unwrap();
        assert!(error.contains("day, week, month, year"));
    }

    #[test]
    fn validates_base_urls() {
        assert_eq!(
            normalize_base_url(Some("http://searxng:8080/".to_string()), Provider::Searxng)
                .unwrap(),
            "http://searxng:8080"
        );
        assert!(normalize_base_url(Some("searxng:8080".to_string()), Provider::Searxng).is_err());
    }

    #[test]
    fn encodes_utf8_query_bytes() {
        assert_eq!(url_encode("café"), "caf%C3%A9");
    }

    #[test]
    fn cleans_markup_entities_and_whitespace() {
        assert_eq!(
            clean_text("A <strong>bold</strong> &amp; useful &#x2014; result"),
            "A bold & useful \u{2014} result"
        );
    }

    #[test]
    fn parses_brave_results() {
        let body = r#"{
            "web": {
                "results": [
                    {
                        "title": "Rust &amp; WebAssembly",
                        "url": "https://example.com/rust",
                        "description": "A <strong>useful</strong> result."
                    },
                    {
                        "title": "Ignored",
                        "url": "",
                        "description": "Missing URL"
                    }
                ]
            }
        }"#;

        let output = format_results(
            "rust wasm",
            parse_results(Provider::Brave, body, 5).unwrap(),
        );
        assert_eq!(
            output,
            "1. Rust & WebAssembly\n   https://example.com/rust\n   A useful result.\n"
        );
    }

    #[test]
    fn parses_searxng_results_and_applies_limit() {
        let body = r#"{
            "results": [
                {
                    "title": "First",
                    "url": "https://example.com/1",
                    "content": "First snippet"
                },
                {
                    "title": "Second",
                    "url": "https://example.com/2",
                    "content": "Second snippet"
                }
            ]
        }"#;

        let results = parse_results(Provider::Searxng, body, 1).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "First");
        assert_eq!(results[0].snippet, "First snippet");
    }

    #[test]
    fn parses_whoogle_results() {
        let body = r#"{
            "query": "rust",
            "results": [
                {
                    "title": "Rust",
                    "text": "Rust combined result text",
                    "href": "https://www.rust-lang.org/",
                    "content": "A language empowering everyone."
                }
            ]
        }"#;

        let results = parse_results(Provider::Whoogle, body, 5).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Rust");
        assert_eq!(results[0].url, "https://www.rust-lang.org/");
        assert_eq!(results[0].snippet, "A language empowering everyone.");
    }

    #[test]
    fn reports_whoogle_blocks() {
        let body = r#"{
            "blocked": true,
            "error_message": "Google blocked the request",
            "results": []
        }"#;

        let error = parse_results(Provider::Whoogle, body, 5).unwrap_err();
        assert!(error.contains("blocked by Google"));
        assert!(error.contains("Switch to SearXNG"));
    }

    #[test]
    fn explains_searxng_json_403() {
        let response = HttpResponse {
            status: 403,
            body: String::new(),
        };
        let error = check_status(Provider::Searxng, &response).unwrap_err();
        assert!(error.contains("search.formats"));
    }

    #[test]
    fn extracts_api_error_detail() {
        let body = r#"{"error":{"detail":"Invalid request parameter"}}"#;
        assert_eq!(
            error_detail(body).as_deref(),
            Some("Invalid request parameter")
        );
    }

    #[test]
    fn handles_missing_web_results() {
        let results =
            parse_results(Provider::Brave, r#"{"query":{"original":"nothing"}}"#, 5).unwrap();
        assert!(results.is_empty());
        assert_eq!(
            format_results("nothing", results),
            "No results found for \"nothing\"."
        );
    }
}
