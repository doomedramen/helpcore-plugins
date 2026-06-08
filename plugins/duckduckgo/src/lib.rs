// DuckDuckGo Search plugin — HTML search, no API key needed
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

struct DuckDuckGo;

impl Guest for DuckDuckGo {
    fn call(tool: String, input_json: String) -> Result<String, String> {
        let input: Value = serde_json::from_str(&input_json)
            .map_err(|e| format!("invalid input JSON: {e}"))?;

        match tool.as_str() {
            "duckduckgo_search" => duckduckgo_search(&input),
            _ => Err(format!("unknown tool: {tool}")),
        }
    }
}

export!(DuckDuckGo);

const MAX_RESULTS_DEFAULT: usize = 5;
const MAX_RESULTS_LIMIT: usize = 10;

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

#[derive(Serialize)]
struct SearchResult {
    title: String,
    url: String,
    snippet: String,
}

fn http_get(url: &str) -> Result<(u16, String), String> {
    let mut headers = serde_json::Map::new();
    headers.insert(
        "Accept".into(),
        Value::String("text/html,application/xhtml+xml,text/plain;q=0.9,*/*;q=0.8".into()),
    );
    headers.insert(
        "User-Agent".into(),
        Value::String(
            "Mozilla/5.0 (compatible; helpcore-duckduckgo/0.1; +https://github.com/doomedramen/helpcore)"
                .into(),
        ),
    );

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

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'A'..=b'F' => Some(b - b'A' + 10),
        b'a'..=b'f' => Some(b - b'a' + 10),
        _ => None,
    }
}

fn percent_decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                out.push((hi << 4 | lo) as char);
                i += 3;
                continue;
            }
        }
        if bytes[i] == b'+' {
            out.push(' ');
        } else {
            out.push(bytes[i] as char);
        }
        i += 1;
    }
    out
}

fn decode_one_entity(entity: &str) -> Option<char> {
    let named = match entity {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some('\u{00A0}'),
        "mdash" => Some('\u{2014}'),
        "ndash" => Some('\u{2013}'),
        "hellip" => Some('\u{2026}'),
        "copy" => Some('\u{00A9}'),
        "reg" => Some('\u{00AE}'),
        "trade" => Some('\u{2122}'),
        "rsquo" => Some('\u{2019}'),
        "lsquo" => Some('\u{2018}'),
        "rdquo" => Some('\u{201D}'),
        "ldquo" => Some('\u{201C}'),
        "laquo" => Some('\u{00AB}'),
        "raquo" => Some('\u{00BB}'),
        "deg" => Some('\u{00B0}'),
        "middot" => Some('\u{00B7}'),
        "bull" => Some('\u{2022}'),
        "euro" => Some('\u{20AC}'),
        "pound" => Some('\u{00A3}'),
        "yen" => Some('\u{00A5}'),
        "cent" => Some('\u{00A2}'),
        "sect" => Some('\u{00A7}'),
        "uml" => Some('\u{00A8}'),
        "micro" => Some('\u{00B5}'),
        "plusmn" => Some('\u{00B1}'),
        "times" => Some('\u{00D7}'),
        "divide" => Some('\u{00F7}'),
        "sup1" => Some('\u{00B9}'),
        "sup2" => Some('\u{00B2}'),
        "sup3" => Some('\u{00B3}'),
        "frac14" => Some('\u{00BC}'),
        "frac12" => Some('\u{00BD}'),
        "frac34" => Some('\u{00BE}'),
        _ => None,
    };
    if named.is_some() {
        return named;
    }
    if let Some(rest) = entity.strip_prefix('#') {
        if let Some(hex) = rest.strip_prefix('x').or_else(|| rest.strip_prefix('X')) {
            return u32::from_str_radix(hex, 16).ok().and_then(char::from_u32);
        }
        return rest.parse::<u32>().ok().and_then(char::from_u32);
    }
    None
}

fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut chars = s.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if c == '&' {
            if let Some(rel_semi) = s[i..].find(';') {
                if rel_semi <= 12 {
                    let entity = &s[i + 1..i + rel_semi];
                    if let Some(decoded) = decode_one_entity(entity) {
                        out.push(decoded);
                        let target = i + rel_semi + 1;
                        while let Some(&(j, _)) = chars.peek() {
                            if j < target {
                                chars.next();
                            } else {
                                break;
                            }
                        }
                        continue;
                    }
                }
            }
        }
        out.push(c);
    }
    out
}

fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

fn clean_whitespace(raw: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    for line in raw.split('\n') {
        let collapsed = line.split_whitespace().collect::<Vec<_>>().join(" ");
        lines.push(collapsed);
    }

    let mut result: Vec<&str> = Vec::with_capacity(lines.len());
    let mut prev_blank = true;
    for line in &lines {
        if line.is_empty() {
            if !prev_blank {
                result.push("");
            }
            prev_blank = true;
        } else {
            result.push(line);
            prev_blank = false;
        }
    }
    while result.last() == Some(&"") {
        result.pop();
    }
    result.join("\n")
}

fn ascii_lower(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii() {
                c.to_ascii_lowercase()
            } else {
                c
            }
        })
        .collect()
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

fn decode_redirect_url(href: &str) -> Option<String> {
    let trimmed = href.trim();

    if trimmed.is_empty() {
        return None;
    }

    let full = if trimmed.starts_with("//") {
        format!("https:{}", trimmed)
    } else {
        trimmed.to_string()
    };

    if let Some(pos) = full.find("/l/?uddg=") {
        let after = &full[pos + 9..];
        let encoded = match after.find('&') {
            Some(amp) => &after[..amp],
            None => after,
        };
        let decoded = percent_decode(encoded);
        if is_http_url(&decoded) {
            return Some(decoded);
        }
    }

    if is_http_url(trimmed) {
        return Some(trimmed.to_string());
    }

    None
}

fn locate_attr_value(html: &str, lower: &str, tag_start: usize, tag_end: usize, attr: &str) -> Option<(usize, usize)> {
    let search = format!("{}=", attr);
    let lower_search = ascii_lower(&search);
    let tag_lower = &lower[tag_start..tag_end];

    // Find `attr=` at an attribute-name boundary (preceded by whitespace) so we
    // don't match a longer attribute that happens to end in the same name, e.g.
    // `data-href=` or `xlink:href=` when looking for `href=`.
    let mut search_from = 0;
    let attr_pos = loop {
        let rel = tag_lower[search_from..].find(&lower_search)?;
        let pos = search_from + rel;
        if matches!(tag_lower[..pos].chars().last(), Some(c) if c.is_whitespace()) {
            break pos;
        }
        search_from = pos + 1;
    };

    let abs_pos = tag_start + attr_pos + search.len();
    let rest = &html[abs_pos..tag_end];
    let trimmed = rest.trim_start();
    let offset = rest.len() - trimmed.len();

    let delim = trimmed.as_bytes().first().copied();
    match delim {
        Some(b'"') | Some(b'\'') => {
            let d = delim.unwrap() as char;
            let inner = &trimmed[1..];
            if let Some(end) = inner.find(d) {
                return Some((abs_pos + offset + 1, abs_pos + offset + 1 + end));
            }
        }
        _ => {
            let end_pos = trimmed.find(|c: char| c.is_whitespace() || c == '>').unwrap_or(trimmed.len());
            let val = trimmed[..end_pos].trim_end();
            if !val.is_empty() {
                return Some((abs_pos + offset, abs_pos + offset + val.len()));
            }
        }
    }
    None
}

fn has_result_ad_class(tag_lower: &str) -> bool {
    if let Some(class_start) = tag_lower.find("class=") {
        let after = &tag_lower[class_start + 6..];
        let trimmed = after.trim_start();
        let class_val = match trimmed.as_bytes().first() {
            Some(b'"') | Some(b'\'') => {
                let d = trimmed.as_bytes()[0] as char;
                let inner = &trimmed[1..];
                inner.find(d).map(|end| &inner[..end]).unwrap_or("")
            }
            _ => {
                trimmed
                    .find(|c: char| c.is_whitespace() || c == '>')
                    .map(|end| &trimmed[..end])
                    .unwrap_or(trimmed)
            }
        };
        return class_val.split_whitespace().any(|c| c == "result--ad");
    }
    false
}

fn find_matching_div_end(html: &str, lower: &str, open_end: usize) -> Option<usize> {
    let bytes = html.as_bytes();
    let lower_bytes = lower.as_bytes();
    let n = bytes.len();
    let mut depth: u32 = 1;
    let mut i = open_end;

    while i < n {
        if lower_bytes[i] == b'<' {
            let remaining = n - i;
            if remaining >= 6 && lower_bytes[i..i + 6] == *b"</div>" {
                i += 6;
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
                continue;
            }
            if remaining >= 5 && lower_bytes[i..i + 5] == *b"</div" {
                let gt = html[i..].find('>').map(|p| i + p + 1).unwrap_or(n);
                depth -= 1;
                if depth == 0 {
                    return Some(gt);
                }
                i = gt;
                continue;
            }
            if remaining >= 4 && lower_bytes[i..i + 4] == *b"<div" {
                let after = &html[i..];
                let tag_end = after.find('>').map(|p| i + p + 1).unwrap_or(n);
                let tag_content = &html[i..tag_end];
                if !tag_content.ends_with("/>") {
                    depth += 1;
                }
                i = tag_end;
                continue;
            }
        }
        i += 1;
    }
    None
}

fn is_challenge_page(body: &str) -> bool {
    let lower = ascii_lower(body);
    lower.contains("please complete the following challenge")
        || lower.contains("sorry, we couldn't verify")
        || lower.contains("we've detected unusual traffic")
        || lower.contains("captcha")
        || (lower.contains("challenge") && lower.contains("verify"))
}

fn duckduckgo_search(input: &Value) -> Result<String, String> {
    let query = input
        .get("query")
        .and_then(Value::as_str)
        .ok_or("query is required, e.g. 'rust programming language'")?;

    let query = query.trim();
    if query.is_empty() {
        return Err("query must not be empty".to_string());
    }

    let max_results = input
        .get("max_results")
        .and_then(Value::as_u64)
        .unwrap_or(MAX_RESULTS_DEFAULT as u64)
        .clamp(1, MAX_RESULTS_LIMIT as u64) as usize;

    let search_url = format!(
        "https://html.duckduckgo.com/html/?q={}",
        url_encode(query)
    );

    let (status, body) = http_get(&search_url)?;

    if status == 404 {
        return Err("DuckDuckGo search returned 404 — the service may be temporarily unavailable.".to_string());
    }
    if status == 401 || status == 403 {
        return Err(format!(
            "Access to DuckDuckGo was denied (HTTP {status}) — the host may be blocking the request."
        ));
    }
    if status >= 500 {
        return Err(format!(
            "DuckDuckGo returned a server error (HTTP {status}) — try again later."
        ));
    }
    if status >= 400 {
        return Err(format!("DuckDuckGo search failed with HTTP {status}."));
    }

    let results = parse_results(&body, max_results);

    if results.is_empty() {
        if is_challenge_page(&body) {
            return Err(
                "DuckDuckGo returned a verification challenge page instead of results — it may be rate-limiting or blocking automated requests. Wait a moment and try again, or rephrase the query."
                    .to_string(),
            );
        }
        return Ok(format!("No results found for \"{query}\"."));
    }

    let mut out = String::new();
    for (i, result) in results.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&format!("{}. {}\n", i + 1, result.title));
        out.push_str(&format!("   {}\n", result.url));
        if !result.snippet.is_empty() {
            out.push_str(&format!("   {}\n", result.snippet));
        }
    }

    Ok(out)
}

fn parse_results(html: &str, max_results: usize) -> Vec<SearchResult> {
    let lower = ascii_lower(html);
    let n = html.len();
    let mut results: Vec<SearchResult> = Vec::new();
    let mut seen_urls: Vec<String> = Vec::new();
    let mut i: usize = 0;

    while i < n && results.len() < max_results {
        let rem = &lower[i..];
        let div_open = match rem.find("<div") {
            Some(pos) => i + pos,
            None => break,
        };

        let tag_end = html[div_open..].find('>').map(|p| div_open + p).unwrap_or(n);
        let tag_content = &lower[div_open..tag_end];

        if !tag_content.contains("class=\"result") && !tag_content.contains("class='result") {
            i = tag_end;
            continue;
        }

        if !tag_content.contains("result__a") && !tag_content.contains("result__snippet") {
            if tag_content.contains("class=\"result")
                || tag_content.contains("class='result")
                || tag_content.contains("class=result")
            {
                let word_after = &tag_content[tag_content.find("result").unwrap()..];
                if !word_after.starts_with("result__a")
                    && !word_after.starts_with("result__snippet")
                    && !word_after.starts_with("result__url")
                    && !word_after.starts_with("result__check")
                    && !word_after.starts_with("result__title")
                    && !word_after.starts_with("result__body")
                    && !word_after.starts_with("result__extras")
                    && !word_after.starts_with("result ")
                    && !word_after.starts_with("result\t")
                    && !word_after.starts_with("result\"")
                    && !word_after.starts_with("result'")
                    && !word_after.starts_with("result>")
                    && !word_after.starts_with("results_links")
                {
                    i = tag_end;
                    continue;
                }
            }
        }

        if has_result_ad_class(tag_content) {
            i = tag_end;
            continue;
        }

        let block_end = match find_matching_div_end(html, &lower, tag_end) {
            Some(end) => end,
            None => {
                i = tag_end;
                continue;
            }
        };

        let title = extract_title_link(html, &lower, tag_end, block_end);
        let snippet = extract_snippet(html, &lower, tag_end, block_end);

        if title.is_none() {
            i = block_end;
            continue;
        }

        let (title_text, href) = title.unwrap();
        let decoded_url = match decode_redirect_url(&href) {
            Some(url) => url,
            None => {
                i = block_end;
                continue;
            }
        };

        if !is_http_url(&decoded_url) {
            i = block_end;
            continue;
        }

        if seen_urls.iter().any(|u| u == &decoded_url) {
            i = block_end;
            continue;
        }
        seen_urls.push(decoded_url.clone());

        let clean_title = clean_whitespace(&decode_entities(&strip_tags(&title_text)));
        let clean_snippet = snippet
            .map(|s| clean_whitespace(&decode_entities(&strip_tags(&s))))
            .unwrap_or_default();

        if clean_title.is_empty() {
            i = block_end;
            continue;
        }

        results.push(SearchResult {
            title: clean_title,
            url: decoded_url,
            snippet: clean_snippet,
        });

        i = block_end;
    }

    results
}

fn extract_title_link(html: &str, lower: &str, block_start: usize, block_end: usize) -> Option<(String, String)> {
    let block_lower = &lower[block_start..block_end];
    let link_pos = block_lower.find("class=\"result__a\"")
        .or_else(|| block_lower.find("class='result__a'"))?;

    let abs_pos = block_start + link_pos;
    let before = &lower[block_start..abs_pos];
    let tag_start = before.rfind("<a ").or_else(|| before.rfind("<a\t")).or_else(|| before.rfind("<a>"))?;
    let tag_abs_start = block_start + tag_start;
    let tag_end = html[tag_abs_start..block_end].find('>').map(|p| tag_abs_start + p + 1)?;

    let href_start_end = locate_attr_value(html, lower, tag_abs_start, tag_end, "href")?;
    let href = html[href_start_end.0..href_start_end.1].to_string();

    let title_end = find_closing_tag(html, &lower, "a", tag_end, block_end)?;
    let title_text = html[tag_end..title_end].to_string();

    Some((title_text, href))
}

fn extract_snippet(html: &str, lower: &str, block_start: usize, block_end: usize) -> Option<String> {
    let block_lower = &lower[block_start..block_end];
    let snippet_pos = block_lower.find("class=\"result__snippet\"")
        .or_else(|| block_lower.find("class='result__snippet'"))?;

    let abs_pos = block_start + snippet_pos;
    let before = &lower[block_start..abs_pos];
    let tag_start = before.rfind("<a ").or_else(|| before.rfind("<a\t"))?;
    let tag_abs_start = block_start + tag_start;
    let tag_end = html[tag_abs_start..block_end].find('>').map(|p| tag_abs_start + p + 1)?;

    let content_end = find_closing_tag(html, lower, "a", tag_end, block_end)?;
    let snippet_text = html[tag_end..content_end].to_string();

    let trimmed = snippet_text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn find_closing_tag(_html: &str, lower: &str, tag: &str, start: usize, limit: usize) -> Option<usize> {
    let search_lower = ascii_lower(&format!("</{}", tag));
    let remaining = &lower[start..limit];

    // `</a` also matches inside `</abbr>`, `</article>`, etc., so require the
    // match to be followed by `>` or whitespace to confirm it closes `tag`.
    let mut search_from = 0;
    loop {
        let rel = remaining[search_from..].find(&search_lower)?;
        let pos = search_from + rel;
        let after = &remaining[pos + search_lower.len()..];
        match after.chars().next() {
            None | Some('>') => return Some(start + pos),
            Some(c) if c.is_whitespace() => return Some(start + pos),
            _ => search_from = pos + 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_required() {
        let input = serde_json::json!({});
        let err = duckduckgo_search(&input).unwrap_err();
        assert!(err.contains("query is required"));
    }

    #[test]
    fn query_empty() {
        let input = serde_json::json!({"query": ""});
        let err = duckduckgo_search(&input).unwrap_err();
        assert!(err.contains("must not be empty"));
    }

    #[test]
    fn query_whitespace_only() {
        let input = serde_json::json!({"query": "   "});
        let err = duckduckgo_search(&input).unwrap_err();
        assert!(err.contains("must not be empty"));
    }

    #[test]
    fn max_results_default() {
        let input = serde_json::json!({"query": "test"});
        let val: Value = serde_json::from_str(&serde_json::to_string(&input).unwrap()).unwrap();
        let max = val.get("max_results").and_then(Value::as_u64);
        assert!(max.is_none());
    }

    #[test]
    fn max_results_clamped_low() {
        let val = 0u64.clamp(1, 10);
        assert_eq!(val, 1);
    }

    #[test]
    fn max_results_clamped_high() {
        let val = 100u64.clamp(1, 10);
        assert_eq!(val, 10);
    }

    #[test]
    fn url_encode_basic() {
        assert_eq!(url_encode("hello"), "hello");
        assert_eq!(url_encode("hello world"), "hello%20world");
        assert_eq!(url_encode("rust & go"), "rust%20%26%20go");
        assert_eq!(url_encode("café"), "caf%C3%A9");
    }

    #[test]
    fn percent_decode_basic() {
        assert_eq!(percent_decode("hello%20world"), "hello world");
        assert_eq!(percent_decode("https%3A%2F%2Fexample.com"), "https://example.com");
        assert_eq!(percent_decode("%26"), "&");
        assert_eq!(percent_decode("plain"), "plain");
        assert_eq!(percent_decode(""), "");
    }

    #[test]
    fn percent_decode_plus() {
        assert_eq!(percent_decode("a+b"), "a b");
        assert_eq!(percent_decode("hello+world"), "hello world");
    }

    #[test]
    fn percent_decode_incomplete() {
        assert_eq!(percent_decode("bad%2"), "bad%2");
        assert_eq!(percent_decode("bad%"), "bad%");
        assert_eq!(percent_decode("%ZZfail"), "%ZZfail");
    }

    #[test]
    fn decode_redirect_standard() {
        let url = decode_redirect_url(
            "//duckduckgo.com/l/?uddg=https%3A%2F%2Fwww.example.com%2Fpage&rut=abc",
        );
        assert_eq!(url, Some("https://www.example.com/page".to_string()));
    }

    #[test]
    fn decode_redirect_no_rut() {
        let url = decode_redirect_url(
            "https://duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.org",
        );
        assert_eq!(url, Some("https://example.org".to_string()));
    }

    #[test]
    fn decode_redirect_direct_http() {
        let url = decode_redirect_url("https://example.com/article");
        assert_eq!(url, Some("https://example.com/article".to_string()));
    }

    #[test]
    fn decode_redirect_empty() {
        let url = decode_redirect_url("");
        assert!(url.is_none());
    }

    #[test]
    fn decode_redirect_non_http() {
        let url = decode_redirect_url("javascript:void(0)");
        assert!(url.is_none());
    }

    #[test]
    fn decode_redirect_ftp_rejected() {
        let url = decode_redirect_url("ftp://files.example.com");
        assert!(url.is_none());
    }

    #[test]
    fn decode_redirect_mailto_rejected() {
        let url = decode_redirect_url("mailto:user@example.com");
        assert!(url.is_none());
    }

    #[test]
    fn is_http_url_valid() {
        assert!(is_http_url("https://example.com"));
        assert!(is_http_url("http://example.com"));
    }

    #[test]
    fn is_http_url_invalid() {
        assert!(!is_http_url("ftp://example.com"));
        assert!(!is_http_url("javascript:void(0)"));
        assert!(!is_http_url("mailto:test@test.com"));
        assert!(!is_http_url(""));
    }

    #[test]
    fn html_entity_named() {
        assert_eq!(decode_entities("foo &amp; bar"), "foo & bar");
        assert_eq!(decode_entities("&lt;div&gt;"), "<div>");
        assert_eq!(decode_entities("&quot;hello&quot;"), "\"hello\"");
        assert_eq!(decode_entities("don&apos;t"), "don't");
    }

    #[test]
    fn html_entity_numeric_decimal() {
        assert_eq!(decode_entities("&#60;div&#62;"), "<div>");
        assert_eq!(decode_entities("&#38;amp;"), "&amp;");
    }

    #[test]
    fn html_entity_numeric_hex() {
        assert_eq!(decode_entities("&#x3C;div&#x3E;"), "<div>");
        assert_eq!(decode_entities("&#x26;amp;"), "&amp;");
    }

    #[test]
    fn html_entity_unknown_passthrough() {
        assert_eq!(decode_entities("&unknown;"), "&unknown;");
        assert_eq!(decode_entities("&"), "&");
    }

    #[test]
    fn html_entity_no_semicolon() {
        assert_eq!(decode_entities("no entity & here"), "no entity & here");
    }

    #[test]
    fn strip_tags_basic() {
        assert_eq!(strip_tags("<b>bold</b> text"), "bold text");
        assert_eq!(strip_tags("<a href='x'>link</a>"), "link");
    }

    #[test]
    fn strip_tags_nested() {
        assert_eq!(strip_tags("<b><i>nested</i></b>"), "nested");
        assert_eq!(strip_tags("<span class='x'>text</span>"), "text");
    }

    #[test]
    fn strip_tags_malformed() {
        assert_eq!(strip_tags("text <without> close"), "text  close");
        assert_eq!(strip_tags("start> text"), "start text");
    }

    #[test]
    fn clean_whitespace_basic() {
        assert_eq!(clean_whitespace("hello  world"), "hello world");
        assert_eq!(clean_whitespace("line1\n\n\nline2"), "line1\n\nline2");
    }

    #[test]
    fn clean_whitespace_leading_trailing() {
        assert_eq!(clean_whitespace("\n\n\ntext\n\n"), "text");
        assert_eq!(clean_whitespace("  trimmed  "), "trimmed");
    }

    #[test]
    fn clean_whitespace_tabs() {
        assert_eq!(clean_whitespace("col1\tcol2\tcol3"), "col1 col2 col3");
    }

    #[test]
    fn has_result_ad_detects_ad() {
        assert!(has_result_ad_class("class=\"result result--ad\""));
        assert!(has_result_ad_class("class='result--ad result'"));
        assert!(has_result_ad_class("class=result--ad"));
    }

    #[test]
    fn has_result_ad_no_ad() {
        assert!(!has_result_ad_class("class=\"result\""));
        assert!(!has_result_ad_class("class='result  web-result'"));
        assert!(!has_result_ad_class(""));
    }

    #[test]
    fn is_challenge_detects_challenge() {
        assert!(is_challenge_page("Please complete the following challenge to continue"));
        assert!(is_challenge_page("Sorry, we couldn't verify your request"));
        assert!(is_challenge_page("We've detected unusual traffic from your network"));
    }

    #[test]
    fn is_challenge_no_false_positive() {
        assert!(!is_challenge_page("<div class='result'>normal results</div>"));
        assert!(!is_challenge_page("search results page"));
        assert!(!is_challenge_page(""));
    }

    #[test]
    fn parse_results_empty_html() {
        let results = parse_results("", 5);
        assert!(results.is_empty());
    }

    #[test]
    fn parse_results_no_results() {
        let html = "<html><body><p>No results found.</p></body></html>";
        let results = parse_results(html, 5);
        assert!(results.is_empty());
    }

    #[test]
    fn parse_results_single_result() {
        let html = r#"<div class="result results_links results_links_deep web-result">
    <div class="links_main links_deep result__body">
        <h2 class="result__title">
            <a class="result__a" href="https://example.com/page">Example Title</a>
        </h2>
        <a class="result__snippet">This is a snippet about examples.</a>
    </div>
</div>"#;
        let results = parse_results(html, 5);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Example Title");
        assert_eq!(results[0].url, "https://example.com/page");
        assert_eq!(results[0].snippet, "This is a snippet about examples.");
    }

    #[test]
    fn parse_results_with_redirect_url() {
        let html = r#"<div class="result results_links results_links_deep web-result">
    <div class="links_main links_deep result__body">
        <h2 class="result__title">
            <a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fpage">Example</a>
        </h2>
        <a class="result__snippet">Snippet text</a>
    </div>
</div>"#;
        let results = parse_results(html, 5);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].url, "https://example.com/page");
    }

    #[test]
    fn parse_results_skips_ads() {
        let html = r#"<div class="result result--ad">
    <div class="links_main links_deep result__body">
        <h2 class="result__title">
            <a class="result__a" href="https://ad-site.com">Ad Title</a>
        </h2>
        <a class="result__snippet">Buy now!</a>
    </div>
</div>
<div class="result results_links results_links_deep web-result">
    <div class="links_main links_deep result__body">
        <h2 class="result__title">
            <a class="result__a" href="https://real-site.com">Real Title</a>
        </h2>
        <a class="result__snippet">Real content here.</a>
    </div>
</div>"#;
        let results = parse_results(html, 5);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Real Title");
    }

    #[test]
    fn parse_results_deduplicates() {
        let result_block = r#"<div class="result results_links results_links_deep web-result">
    <div class="links_main links_deep result__body">
        <h2 class="result__title">
            <a class="result__a" href="https://example.com">Same Title</a>
        </h2>
        <a class="result__snippet">First snippet.</a>
    </div>
</div>"#;
        let html = format!("{}{}", result_block, result_block);
        let results = parse_results(&html, 5);
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn parse_results_respects_max() {
        let result_block = r#"<div class="result results_links results_links_deep web-result">
    <div class="links_main links_deep result__body">
        <h2 class="result__title">
            <a class="result__a" href="https://example.com/PAGE">Title PAGE</a>
        </h2>
        <a class="result__snippet">Snippet</a>
    </div>
</div>"#;
        let mut html = String::new();
        for i in 1..=10 {
            html.push_str(&result_block.replace("PAGE", &i.to_string()));
        }
        let results = parse_results(&html, 3);
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn parse_results_nested_markup_in_title() {
        let html = r#"<div class="result results_links results_links_deep web-result">
    <div class="links_main links_deep result__body">
        <h2 class="result__title">
            <a class="result__a" href="https://example.com">Title with <b>bold</b> text</a>
        </h2>
        <a class="result__snippet">Snippet</a>
    </div>
</div>"#;
        let results = parse_results(html, 5);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Title with bold text");
    }

    #[test]
    fn parse_results_title_with_abbr_tag_not_mistaken_for_closing_a() {
        let html = r#"<div class="result results_links results_links_deep web-result">
    <div class="links_main links_deep result__body">
        <h2 class="result__title">
            <a class="result__a" href="https://example.com">Guide to <abbr>HTML</abbr> and CSS</a>
        </h2>
        <a class="result__snippet">Snippet</a>
    </div>
</div>"#;
        let results = parse_results(html, 5);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Guide to HTML and CSS");
    }

    #[test]
    fn parse_results_nested_markup_in_snippet() {
        let html = r#"<div class="result results_links results_links_deep web-result">
    <div class="links_main links_deep result__body">
        <h2 class="result__title">
            <a class="result__a" href="https://example.com">Title</a>
        </h2>
        <a class="result__snippet">Snippet with <b>highlighted</b> and <i>italic</i> text.</a>
    </div>
</div>"#;
        let results = parse_results(html, 5);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].snippet, "Snippet with highlighted and italic text.");
    }

    #[test]
    fn parse_results_entities_in_title() {
        let html = r#"<div class="result results_links results_links_deep web-result">
    <div class="links_main links_deep result__body">
        <h2 class="result__title">
            <a class="result__a" href="https://example.com">Foo &amp; Bar &mdash; Guide</a>
        </h2>
        <a class="result__snippet">Content</a>
    </div>
</div>"#;
        let results = parse_results(html, 5);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Foo & Bar \u{2014} Guide");
    }

    #[test]
    fn parse_results_missing_snippet() {
        let html = r#"<div class="result results_links results_links_deep web-result">
    <div class="links_main links_deep result__body">
        <h2 class="result__title">
            <a class="result__a" href="https://example.com">Title Only</a>
        </h2>
    </div>
</div>"#;
        let results = parse_results(html, 5);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].snippet, "");
    }

    #[test]
    fn parse_results_rejects_non_http_links() {
        let html = r#"<div class="result results_links results_links_deep web-result">
    <div class="links_main links_deep result__body">
        <h2 class="result__title">
            <a class="result__a" href="//duckduckgo.com/l/?uddg=ftp%3A%2F%2Ffiles.example.com">FTP Result</a>
        </h2>
        <a class="result__snippet">This should be rejected.</a>
    </div>
</div>"#;
        let results = parse_results(html, 5);
        assert!(results.is_empty());
    }

    #[test]
    fn parse_results_rejects_empty_title() {
        let html = r#"<div class="result results_links results_links_deep web-result">
    <div class="links_main links_deep result__body">
        <h2 class="result__title">
            <a class="result__a" href="https://example.com"></a>
        </h2>
        <a class="result__snippet">Snippet with no title</a>
    </div>
</div>"#;
        let results = parse_results(html, 5);
        assert!(results.is_empty());
    }

    #[test]
    fn parse_results_whitespace_only_title_rejected() {
        let html = r#"<div class="result results_links results_links_deep web-result">
    <div class="links_main links_deep result__body">
        <h2 class="result__title">
            <a class="result__a" href="https://example.com">   </a>
        </h2>
        <a class="result__snippet">Snippet</a>
    </div>
</div>"#;
        let results = parse_results(html, 5);
        assert!(results.is_empty());
    }

    #[test]
    fn parse_results_url_with_encoded_entities() {
        let html = r#"<div class="result results_links results_links_deep web-result">
    <div class="links_main links_deep result__body">
        <h2 class="result__title">
            <a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2F%3Fq%3Dhello%26lang%3Den">Search</a>
        </h2>
        <a class="result__snippet">Result with query params</a>
    </div>
</div>"#;
        let results = parse_results(html, 5);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].url, "https://example.com/?q=hello&lang=en");
    }

    #[test]
    fn parse_results_multiple_same_domain_different_paths() {
        let html = format!(
            "{}{}",
            r#"<div class="result results_links results_links_deep web-result">
    <div class="links_main links_deep result__body">
        <h2 class="result__title">
            <a class="result__a" href="https://example.com/page1">Page 1</a>
        </h2>
        <a class="result__snippet">First page</a>
    </div>
</div>"#,
            r#"<div class="result results_links results_links_deep web-result">
    <div class="links_main links_deep result__body">
        <h2 class="result__title">
            <a class="result__a" href="https://example.com/page2">Page 2</a>
        </h2>
        <a class="result__snippet">Second page</a>
    </div>
</div>"#
        );
        let results = parse_results(&html, 5);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Page 1");
        assert_eq!(results[1].title, "Page 2");
    }

    #[test]
    fn parse_results_mixed_valid_and_invalid() {
        let html = format!(
            "{}{}{}",
            r#"<div class="result results_links results_links_deep web-result">
    <div class="links_main links_deep result__body">
        <h2 class="result__title">
            <a class="result__a" href="https://good.com">Good</a>
        </h2>
        <a class="result__snippet">Valid</a>
    </div>
</div>"#,
            r#"<div class="result results_links results_links_deep web-result">
    <div class="links_main links_deep result__body">
        <h2 class="result__title">
            <a class="result__a" href="javascript:void(0)">Bad</a>
        </h2>
        <a class="result__snippet">Invalid link</a>
    </div>
</div>"#,
            r#"<div class="result results_links results_links_deep web-result">
    <div class="links_main links_deep result__body">
        <h2 class="result__title">
            <a class="result__a" href="https://also-good.com">Also Good</a>
        </h2>
        <a class="result__snippet">Valid too</a>
    </div>
</div>"#
        );
        let results = parse_results(&html, 5);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Good");
        assert_eq!(results[1].title, "Also Good");
    }

    #[test]
    fn locate_attr_value_double_quoted() {
        let tag = "<a class=\"result__a\" href=\"https://example.com\" rel=\"nofollow\">";
        let lower = ascii_lower(tag);
        let val = locate_attr_value(tag, &lower, 0, tag.len(), "href");
        assert!(val.is_some());
        let (start, end) = val.unwrap();
        assert_eq!(&tag[start..end], "https://example.com");
    }

    #[test]
    fn locate_attr_value_single_quoted() {
        let tag = "<a class='result__a' href='https://example.com' rel='nofollow'>";
        let lower = ascii_lower(tag);
        let val = locate_attr_value(tag, &lower, 0, tag.len(), "href");
        assert!(val.is_some());
        let (start, end) = val.unwrap();
        assert_eq!(&tag[start..end], "https://example.com");
    }

    #[test]
    fn locate_attr_value_not_found() {
        let tag = "<a class='result__a'>No href here</a>";
        let lower = ascii_lower(tag);
        let val = locate_attr_value(tag, &lower, 0, tag.len(), "href");
        assert!(val.is_none());
    }

    #[test]
    fn locate_attr_value_skips_attr_with_matching_suffix() {
        let tag = "<a data-href=\"wrong\" class=\"result__a\" href=\"https://example.com\">";
        let lower = ascii_lower(tag);
        let val = locate_attr_value(tag, &lower, 0, tag.len(), "href");
        assert!(val.is_some());
        let (start, end) = val.unwrap();
        assert_eq!(&tag[start..end], "https://example.com");
    }

    #[test]
    fn find_closing_tag_skips_similarly_named_tags() {
        let html = "<a>Some <abbr>text</abbr> and <article>more</article> here</a> tail";
        let lower = ascii_lower(html);
        let start = html.find('>').unwrap() + 1;
        let end = find_closing_tag(html, &lower, "a", start, html.len());
        assert!(end.is_some());
        assert_eq!(
            &html[start..end.unwrap()],
            "Some <abbr>text</abbr> and <article>more</article> here"
        );
    }

    #[test]
    fn find_matching_div_end_simple() {
        let html = "<div class=\"result\">content</div>after";
        let lower = ascii_lower(html);
        let gt = html.find('>').unwrap() + 1;
        let end = find_matching_div_end(html, &lower, gt);
        assert!(end.is_some());
        assert_eq!(&html[..end.unwrap()], "<div class=\"result\">content</div>");
    }

    #[test]
    fn find_matching_div_end_nested() {
        let html = "<div class=\"result\"><div>inner</div>outer</div>end";
        let lower = ascii_lower(html);
        let gt = html.find('>').unwrap() + 1;
        let end = find_matching_div_end(html, &lower, gt);
        assert!(end.is_some());
        assert_eq!(end.unwrap(), html.len() - 3); // before "end"
    }

    #[test]
    fn find_matching_div_end_self_closing() {
        let html = "<div class=\"result\"><div class=\"spacer\"/>content</div>";
        let lower = ascii_lower(html);
        let gt = html.find('>').unwrap() + 1;
        let end = find_matching_div_end(html, &lower, gt);
        assert!(end.is_some());
    }
}
