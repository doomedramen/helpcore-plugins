// Page Fetch plugin - fetches a URL and converts HTML to clean plain text (no API key needed)
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

struct PageFetch;

impl Guest for PageFetch {
    fn call(tool: String, input_json: String) -> Result<String, String> {
        let input: Value = serde_json::from_str(&input_json)
            .map_err(|e| format!("invalid input JSON: {e}"))?;

        match tool.as_str() {
            "page_fetch" => page_fetch(&input),
            _ => Err(format!("unknown tool: {tool}")),
        }
    }
}

export!(PageFetch);

const MAX_OUTPUT_CHARS: usize = 12_000;

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

fn validate_url(url: &str) -> Result<String, String> {
    let trimmed = url.trim();
    if !(trimmed.starts_with("http://") || trimmed.starts_with("https://")) {
        return Err(format!(
            "'{trimmed}' doesn't look like a valid URL — it must start with http:// or https://"
        ));
    }
    Ok(trimmed.to_string())
}

fn fetch(url: &str) -> Result<(u16, String), String> {
    let mut headers = serde_json::Map::new();
    headers.insert(
        "Accept".into(),
        Value::String("text/html,application/xhtml+xml,text/plain;q=0.9,*/*;q=0.8".into()),
    );
    headers.insert(
        "User-Agent".into(),
        Value::String(
            "Mozilla/5.0 (compatible; helpcore-page-fetch/0.1; +https://github.com/doomedramen/helpcore)"
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

// ── HTML → text conversion ────────────────────────────────────────────────────

/// Tags whose presence should introduce a line break in the extracted text,
/// so paragraphs/headings/list items don't run together.
fn is_block_tag(name: &str) -> bool {
    matches!(
        name,
        "br" | "p" | "div" | "tr" | "li" | "ul" | "ol" | "table" | "blockquote"
            | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "section" | "article"
            | "header" | "footer" | "nav" | "main" | "form" | "pre" | "hr" | "dd" | "dt"
    )
}

/// Decode a single HTML entity body (without the surrounding `&`/`;`).
fn decode_one_entity(entity: &str) -> Option<char> {
    let named = match entity {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some('\u{00A0}'),
        "mdash" => Some('—'),
        "ndash" => Some('–'),
        "hellip" => Some('…'),
        "copy" => Some('©'),
        "reg" => Some('®'),
        "trade" => Some('™'),
        "rsquo" => Some('\u{2019}'),
        "lsquo" => Some('\u{2018}'),
        "rdquo" => Some('\u{201D}'),
        "ldquo" => Some('\u{201C}'),
        _ => None,
    };
    if named.is_some() {
        return named;
    }
    // Numeric character references: &#1234; (decimal) or &#x1F600; (hex).
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
                        // Skip ahead past the consumed entity.
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

/// Strip any remaining tags from a small fragment (used for `<title>` contents,
/// which occasionally contain nested markup like `<span>`).
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

/// Collapse runs of whitespace within lines, and collapse multiple consecutive
/// blank lines down to a single blank line, so output reads like clean prose.
fn clean_whitespace(raw: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    for line in raw.split('\n') {
        let collapsed = line.split_whitespace().collect::<Vec<_>>().join(" ");
        lines.push(collapsed);
    }

    let mut result: Vec<&str> = Vec::with_capacity(lines.len());
    let mut prev_blank = true; // skip leading blank lines
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

/// ASCII-only lowercasing that preserves byte length and offsets, so the
/// result stays index-aligned with the original string (full Unicode
/// `to_lowercase()` can change the byte length of some characters). Tag and
/// attribute names are always ASCII, so this is sufficient for case-insensitive
/// tag matching while keeping it cheap enough to compute once up front.
fn ascii_lower(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii() { c.to_ascii_lowercase() } else { c })
        .collect()
}

/// Convert raw HTML into (title, body text). Strips `<script>`/`<style>`/
/// `<noscript>`/`<template>` blocks and HTML comments entirely, turns
/// block-level elements into line breaks, decodes entities, and discards tags.
fn html_to_text(html: &str) -> (Option<String>, String) {
    let mut title: Option<String> = None;
    let mut out = String::with_capacity(html.len() / 2);

    // Computed once up front (rather than per-tag) to avoid O(n·m) blowups on
    // pages with many <script>/<style>/<title> tags — index-aligned with `html`.
    let lower_html = ascii_lower(html);

    let bytes = html.as_bytes();
    let n = bytes.len();
    let mut i = 0;

    while i < n {
        if bytes[i] != b'<' {
            let next_lt = html[i..].find('<').map(|p| i + p).unwrap_or(n);
            out.push_str(&decode_entities(&html[i..next_lt]));
            i = next_lt;
            continue;
        }

        // HTML comment.
        if html[i..].starts_with("<!--") {
            i = match html[i + 4..].find("-->") {
                Some(rel) => i + 4 + rel + 3,
                None => n,
            };
            continue;
        }

        // Other declarations like <!DOCTYPE ...> or malformed '<' with no tag name.
        let Some(rel_gt) = html[i..].find('>') else {
            break; // unterminated tag — stop processing
        };
        let tag_inner = &html[i + 1..i + rel_gt];
        let tag_end = i + rel_gt + 1;

        let trimmed = tag_inner.trim_start();
        let is_closing = trimmed.starts_with('/');
        let name_start = if is_closing { 1 } else { 0 };
        let name: String = trimmed[name_start..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_lowercase();

        if !is_closing && matches!(name.as_str(), "script" | "style" | "noscript" | "template") {
            let close_tag = format!("</{name}");
            i = match lower_html[tag_end..].find(&close_tag) {
                Some(rel) => {
                    let abs = tag_end + rel;
                    match html[abs..].find('>') {
                        Some(g) => abs + g + 1,
                        None => n,
                    }
                }
                None => n,
            };
            continue;
        }

        if !is_closing && name == "title" && title.is_none() {
            i = match lower_html[tag_end..].find("</title") {
                Some(rel) => {
                    let abs = tag_end + rel;
                    let inner = &html[tag_end..abs];
                    let cleaned = decode_entities(&strip_tags(inner)).trim().to_string();
                    if !cleaned.is_empty() {
                        title = Some(cleaned);
                    }
                    match html[abs..].find('>') {
                        Some(g) => abs + g + 1,
                        None => n,
                    }
                }
                None => tag_end,
            };
            continue;
        }

        if is_block_tag(&name) {
            out.push('\n');
        }

        i = tag_end;
    }

    (title, clean_whitespace(&out))
}

fn page_fetch(input: &Value) -> Result<String, String> {
    let url = input
        .get("url")
        .and_then(Value::as_str)
        .ok_or("url is required, e.g. 'https://example.com/article'")?;
    let url = validate_url(url)?;

    let (status, body) = fetch(&url)?;
    if status == 404 {
        return Err(format!("The page at {url} could not be found (HTTP 404)."));
    }
    if status == 401 || status == 403 {
        return Err(format!(
            "Access to {url} was denied (HTTP {status}) — it may require login or block automated requests."
        ));
    }
    if status >= 400 {
        return Err(format!("Fetching {url} failed with HTTP {status}."));
    }
    if body.trim().is_empty() {
        return Ok(format!(
            "The page at {url} returned no content (HTTP {status})."
        ));
    }

    let (title, mut text) = html_to_text(&body);

    let mut truncated = false;
    if text.chars().count() > MAX_OUTPUT_CHARS {
        text = text.chars().take(MAX_OUTPUT_CHARS).collect();
        truncated = true;
    }

    if text.trim().is_empty() {
        return Ok(format!(
            "Fetched {url} (HTTP {status}){}, but no readable text content was found — the page may rely on JavaScript to render its content.",
            title.as_ref().map(|t| format!(" ('{t}')")).unwrap_or_default()
        ));
    }

    let mut out = String::new();
    match &title {
        Some(t) => out.push_str(&format!("# {t}\nSource: {url}\n\n{text}")),
        None => out.push_str(&format!("Source: {url}\n\n{text}")),
    }
    if truncated {
        out.push_str("\n\n[Content truncated — the page is longer than shown here.]");
    }

    Ok(out)
}
