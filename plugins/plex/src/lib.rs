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

struct Plex;

impl Guest for Plex {
    fn call(tool: String, input_json: String) -> Result<String, String> {
        let input: Value =
            serde_json::from_str(&input_json).map_err(|e| format!("invalid input JSON: {e}"))?;

        match tool.as_str() {
            "plex_search" => plex_search(&input),
            "plex_recently_added" => plex_recently_added(&input),
            "plex_libraries" => plex_libraries(&input),
            "plex_now_playing" => plex_now_playing(&input),
            _ => Err(format!("unknown tool: {tool}")),
        }
    }
}

export!(Plex);

// ── Config ────────────────────────────────────────────────────────────────────

struct Config {
    url: String,
    token: String,
}

fn load_config() -> Result<Config, String> {
    let url = host::config_read("url").map_err(|_| {
        "Plex server URL is not configured. Set it in the plugin settings.".to_string()
    })?;
    let token = host::secret_read("token")
        .map_err(|_| "Plex token is not configured. Set it in the plugin settings.".to_string())?;
    Ok(Config {
        url: url.trim_end_matches('/').to_string(),
        token,
    })
}

// ── HTTP ──────────────────────────────────────────────────────────────────────

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

fn plex_get(path: &str) -> Result<String, String> {
    let config = load_config()?;
    let mut headers = serde_json::Map::new();
    headers.insert("X-Plex-Token".into(), Value::String(config.token.clone()));
    headers.insert(
        "Accept".into(),
        Value::String("application/json".to_string()),
    );
    let req = HttpRequest {
        method: "GET",
        url: format!("{}{}", config.url, path),
        headers,
        body: None,
    };
    let req_json =
        serde_json::to_string(&req).map_err(|e| format!("failed to serialize request: {e}"))?;
    let resp_json = host::http_request(&req_json)?;
    let resp: HttpResponse = serde_json::from_str(&resp_json)
        .map_err(|e| format!("failed to parse HTTP response: {e}"))?;

    if resp.status == 401 {
        return Err("Unauthorized — check your Plex token in plugin settings.".into());
    }
    if resp.status >= 400 {
        return Err(format!("Plex returned HTTP {}: {}", resp.status, resp.body));
    }
    Ok(resp.body)
}

fn url_encode(s: &str) -> String {
    let mut result = String::new();
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(byte as char);
            }
            b' ' => result.push_str("%20"),
            _ => result.push_str(&format!("%{:02X}", byte)),
        }
    }
    result
}

// ── XML parsing ───────────────────────────────────────────────────────────────

#[derive(Debug)]
struct Tag {
    name: String,
    attrs: Vec<(String, String)>,
    depth: usize,
    self_closing: bool,
}

fn tag_attr<'a>(attrs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

fn decode_entities(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'&' && i + 1 < bytes.len() && bytes[i + 1] != b'#' {
            let rest = &s[i..];
            if let Some(end) = rest.find(';') {
                let entity = &rest[..=end];
                match entity {
                    "&amp;" => {
                        result.push('&');
                        i += 5;
                        continue;
                    }
                    "&lt;" => {
                        result.push('<');
                        i += 4;
                        continue;
                    }
                    "&gt;" => {
                        result.push('>');
                        i += 4;
                        continue;
                    }
                    "&quot;" => {
                        result.push('"');
                        i += 6;
                        continue;
                    }
                    "&apos;" => {
                        result.push('\'');
                        i += 6;
                        continue;
                    }
                    _ => { /* unknown entity, keep as-is */ }
                }
            }
        }
        result.push(s[i..].chars().next().unwrap());
        i += s[i..].chars().next().map_or(1, |c| c.len_utf8());
    }
    result
}

fn parse_xml_tags(xml: &str) -> Vec<Tag> {
    let mut tags = Vec::new();
    let mut depth: usize = 0;
    let bytes = xml.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }
        i += 1; // skip '<'
        if i >= bytes.len() {
            break;
        }

        // Closing tag </name>
        if bytes[i] == b'/' {
            while i < bytes.len() && bytes[i] != b'>' {
                i += 1;
            }
            if i < bytes.len() {
                i += 1;
            } // skip '>'
            depth = depth.saturating_sub(1);
            continue;
        }

        // Skip <?xml...>, <!--...-->, <!DOCTYPE...>
        if bytes[i] == b'?' || bytes[i] == b'!' {
            while i < bytes.len() && bytes[i] != b'>' {
                i += 1;
            }
            if i < bytes.len() {
                i += 1;
            }
            continue;
        }

        // Parse tag name
        let name_start = i;
        while i < bytes.len()
            && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'-')
        {
            i += 1;
        }
        let name = std::str::from_utf8(&bytes[name_start..i]).unwrap_or("");
        if name.is_empty() {
            while i < bytes.len() && bytes[i] != b'>' {
                i += 1;
            }
            if i < bytes.len() {
                i += 1;
            }
            continue;
        }
        let name = name.to_string();

        // Parse attributes
        let mut attrs: Vec<(String, String)> = Vec::new();
        let mut self_closing = false;
        loop {
            while i < bytes.len() && bytes[i] == b' ' {
                i += 1;
            }
            if i >= bytes.len() {
                break;
            }
            if bytes[i] == b'>' {
                i += 1;
                break;
            }
            if bytes[i] == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'>' {
                self_closing = true;
                i += 2;
                break;
            }

            // Parse attribute name
            let attr_start = i;
            while i < bytes.len()
                && bytes[i] != b'='
                && bytes[i] != b' '
                && bytes[i] != b'/'
                && bytes[i] != b'>'
            {
                i += 1;
            }
            let attr_name = std::str::from_utf8(&bytes[attr_start..i]).unwrap_or("");
            if attr_name.is_empty() {
                while i < bytes.len() && bytes[i] != b'>' && bytes[i] != b'/' {
                    i += 1;
                }
                continue;
            }
            let attr_name = attr_name.to_string();

            // Skip whitespace to '='
            while i < bytes.len() && bytes[i] == b' ' {
                i += 1;
            }
            if i >= bytes.len() || bytes[i] != b'=' {
                continue;
            }
            i += 1; // skip '='
            while i < bytes.len() && bytes[i] == b' ' {
                i += 1;
            }

            // Parse quoted value
            if i >= bytes.len() {
                break;
            }
            let quote = bytes[i];
            if quote != b'"' && quote != b'\'' {
                break;
            }
            i += 1; // skip opening quote
            let val_start = i;
            while i < bytes.len() && bytes[i] != quote {
                i += 1;
            }
            let value = std::str::from_utf8(&bytes[val_start..i]).unwrap_or("");
            if i < bytes.len() {
                i += 1;
            } // skip closing quote

            attrs.push((attr_name, decode_entities(value)));
        }

        let current_depth = depth;
        if !self_closing {
            depth += 1;
        }

        tags.push(Tag {
            name,
            attrs,
            depth: current_depth,
            self_closing,
        });
    }

    tags
}

// ── Output helpers ────────────────────────────────────────────────────────────

fn format_date(date: &str) -> &str {
    // Plex ISO timestamps: "2024-01-15T14:30:00Z" → "2024-01-15"
    if date.len() >= 10 {
        &date[..10]
    } else {
        date
    }
}

fn format_duration_ms(ms: u64) -> String {
    let secs = ms / 1000;
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    match (h, m, s) {
        (0, 0, s) => format!("{s}s"),
        (0, m, 0) => format!("{m}m"),
        (0, m, s) => format!("{m}m {s}s"),
        (h, 0, 0) => format!("{h}h"),
        (h, m, 0) => format!("{h}h {m}m"),
        (h, m, s) => format!("{h}h {m}m {s}s"),
    }
}

fn thumb_url(config: &Config, path: &str) -> String {
    if path.is_empty() {
        return String::new();
    }
    format!("{}{}?X-Plex-Token={}", config.url, path, config.token)
}

fn library_name(attrs: &[(String, String)]) -> String {
    tag_attr(attrs, "librarySectionTitle")
        .map(|s| s.to_string())
        .unwrap_or_default()
}

fn human_type(plex_type: &str) -> &str {
    match plex_type {
        "movie" => "Movie",
        "episode" => "TV Episode",
        "show" => "TV Show",
        "season" => "Season",
        "track" => "Music Track",
        "artist" => "Artist",
        "album" => "Album",
        "photo" => "Photo",
        "clip" => "Clip",
        other => other,
    }
}

// ── plex_search ───────────────────────────────────────────────────────────────

fn plex_search(input: &Value) -> Result<String, String> {
    let query = input
        .get("query")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or("query is required")?;

    let config = load_config()?;
    let path = format!("/search?query={}", url_encode(query));
    let body = plex_get(&path)?;

    // Try JSON first, fall back to XML
    if body.trim_start().starts_with('{') {
        search_from_json(&body, &config)
    } else {
        search_from_xml(&body, &config)
    }
}

fn search_from_json(body: &str, config: &Config) -> Result<String, String> {
    let root: Value =
        serde_json::from_str(body).map_err(|e| format!("failed to parse response: {e}"))?;
    let container = &root["MediaContainer"];
    let metadata = container["Metadata"]
        .as_array()
        .ok_or_else(|| "Unexpected search response format".to_string())?;

    if metadata.is_empty() {
        return Ok(format!("No results found."));
    }

    let mut out = format!("Plex search results:\n");
    for item in metadata.iter().take(10) {
        let title = item["title"].as_str().unwrap_or("");
        let year = item["year"]
            .as_u64()
            .map(|y| y.to_string())
            .unwrap_or_default();
        let ptype = item["type"].as_str().unwrap_or("");
        let summary = item["summary"].as_str().unwrap_or("");
        let thumb = item["thumb"].as_str().unwrap_or("");
        let lib = item["librarySectionTitle"].as_str().unwrap_or("");
        let parent = item["parentTitle"].as_str().unwrap_or("");
        let grandparent = item["grandparentTitle"].as_str().unwrap_or("");

        out.push_str(&format_item(
            title,
            &year,
            ptype,
            summary,
            thumb,
            lib,
            parent,
            grandparent,
            config,
        ));
    }
    Ok(out.trim_end().to_string())
}

fn search_from_xml(body: &str, config: &Config) -> Result<String, String> {
    let tags = parse_xml_tags(body);
    let items: Vec<&Tag> = tags
        .iter()
        .filter(|t| {
            t.depth == 1
                && (t.name == "Video" || t.name == "Directory" || t.name == "Track")
                && t.self_closing
        })
        .take(10)
        .collect();

    if items.is_empty() {
        return Ok("No results found.".to_string());
    }

    let mut out = "Plex search results:\n".to_string();
    for tag in items {
        let title = tag_attr(&tag.attrs, "title").unwrap_or("");
        let year = tag_attr(&tag.attrs, "year").unwrap_or("");
        let ptype = tag_attr(&tag.attrs, "type").unwrap_or("");
        let summary = tag_attr(&tag.attrs, "summary").unwrap_or("");
        let thumb = tag_attr(&tag.attrs, "thumb").unwrap_or("");
        let lib = library_name(&tag.attrs);
        let parent = tag_attr(&tag.attrs, "parentTitle").unwrap_or("");
        let grandparent = tag_attr(&tag.attrs, "grandparentTitle").unwrap_or("");

        out.push_str(&format_item(
            title,
            year,
            ptype,
            summary,
            thumb,
            &lib,
            parent,
            grandparent,
            config,
        ));
    }
    Ok(out.trim_end().to_string())
}

fn format_item(
    title: &str,
    year: &str,
    ptype: &str,
    summary: &str,
    thumb: &str,
    lib: &str,
    parent: &str,
    grandparent: &str,
    config: &Config,
) -> String {
    let mut out = format!("  **{}**", if title.is_empty() { "Unknown" } else { title });

    match ptype {
        "track" => {
            if !grandparent.is_empty() {
                out.push_str(&format!(" — {grandparent}"));
            }
            if !parent.is_empty() {
                out.push_str(&format!(" ({parent})"));
            }
        }
        "episode" | "show" => {
            if !year.is_empty() && year != "0" {
                out.push_str(&format!(" ({year})"));
            }
            if !grandparent.is_empty() {
                out.push_str(&format!(" — {grandparent}"));
            }
        }
        _ => {
            if !year.is_empty() && year != "0" {
                out.push_str(&format!(" ({year})"));
            }
        }
    }
    out.push_str(&format!(" [{}]", human_type(ptype)));

    if !lib.is_empty() {
        out.push_str(&format!(" · {lib}"));
    }
    out.push('\n');

    if !summary.is_empty() {
        out.push_str(&format!("    {}\n", truncate(summary, 200)));
    }
    if !thumb.is_empty() {
        out.push_str(&format!("    Thumb: {}\n", thumb_url(config, thumb)));
    }
    out
}

// ── plex_recently_added ───────────────────────────────────────────────────────

fn plex_recently_added(input: &Value) -> Result<String, String> {
    let type_filter = input.get("type").and_then(Value::as_str).unwrap_or("all");
    let count = input
        .get("count")
        .and_then(|v| v.as_u64())
        .unwrap_or(10)
        .min(50)
        .max(1) as usize;

    let config = load_config()?;
    let body = plex_get("/library/recentlyAdded")?;

    let tags = parse_xml_tags(&body);
    let items: Vec<&Tag> = tags
        .iter()
        .filter(|t| {
            t.depth == 1
                && (t.name == "Video" || t.name == "Directory" || t.name == "Track")
                && t.self_closing
        })
        .filter(|t| {
            if type_filter == "all" {
                return true;
            }
            let item_type = tag_attr(&t.attrs, "type").unwrap_or("");
            match type_filter {
                "movie" => item_type == "movie",
                "show" => item_type == "episode" || item_type == "show",
                "music" => item_type == "track",
                _ => true,
            }
        })
        .take(count)
        .collect();

    if items.is_empty() {
        return Ok("No recently added items found.".to_string());
    }

    let header = if type_filter != "all" {
        format!("Recently added ({type_filter}):\n")
    } else {
        "Recently added:\n".to_string()
    };
    let mut out = header;

    for tag in items {
        let title = tag_attr(&tag.attrs, "title").unwrap_or("Unknown");
        let year = tag_attr(&tag.attrs, "year").unwrap_or("");
        let ptype = tag_attr(&tag.attrs, "type").unwrap_or("");
        let added = tag_attr(&tag.attrs, "addedAt").unwrap_or("");
        let summary = tag_attr(&tag.attrs, "summary").unwrap_or("");
        let lib = library_name(&tag.attrs);
        let grandparent = tag_attr(&tag.attrs, "grandparentTitle").unwrap_or("");
        let parent = tag_attr(&tag.attrs, "parentTitle").unwrap_or("");
        let thumb = tag_attr(&tag.attrs, "thumb").unwrap_or("");

        out.push_str(&format!("  **{title}**"));
        match ptype {
            "track" => {
                if !grandparent.is_empty() {
                    out.push_str(&format!(" — {grandparent}"));
                }
                if !parent.is_empty() {
                    out.push_str(&format!(" ({parent})"));
                }
            }
            "episode" => {
                if !grandparent.is_empty() {
                    out.push_str(&format!(" — {grandparent}"));
                }
            }
            _ => {
                if !year.is_empty() && year != "0" {
                    out.push_str(&format!(" ({year})"));
                }
            }
        }
        out.push_str(&format!(" [{}]", human_type(ptype)));
        if !lib.is_empty() {
            out.push_str(&format!(" · {lib}"));
        }
        out.push('\n');

        if !added.is_empty() {
            out.push_str(&format!("    Added: {}\n", format_date(added)));
        }
        if !summary.is_empty() {
            out.push_str(&format!("    {}\n", truncate(summary, 200)));
        }
        if !thumb.is_empty() {
            out.push_str(&format!("    Thumb: {}\n", thumb_url(&config, thumb)));
        }
    }
    Ok(out.trim_end().to_string())
}

// ── plex_libraries ────────────────────────────────────────────────────────────

fn plex_libraries(_input: &Value) -> Result<String, String> {
    let body = plex_get("/library/sections")?;

    let tags = parse_xml_tags(&body);
    let sections: Vec<&Tag> = tags
        .iter()
        .filter(|t| t.depth == 1 && t.name == "Directory" && t.self_closing)
        .collect();

    if sections.is_empty() {
        return Ok("No libraries found on this Plex server.".to_string());
    }

    let mut out = "Plex libraries:\n".to_string();
    for tag in sections {
        let title = tag_attr(&tag.attrs, "title").unwrap_or("Unknown");
        let stype = tag_attr(&tag.attrs, "type").unwrap_or("unknown");
        let key = tag_attr(&tag.attrs, "key").unwrap_or("");
        out.push_str(&format!("  {title} [{}] (key: {key})\n", human_type(stype)));
    }
    Ok(out.trim_end().to_string())
}

// ── plex_now_playing ──────────────────────────────────────────────────────────

fn plex_now_playing(_input: &Value) -> Result<String, String> {
    let body = plex_get("/status/sessions")?;

    let tags = parse_xml_tags(&body);

    // Collect session items: Video/Track at depth 1 that are NOT self_closing
    // Their User and Player children are at depth 2
    let mut sessions: Vec<(
        &str, // title
        &str, // type
        &str, // user
        &str, // player
        u64,  // view_offset (ms)
        u64,  // duration (ms)
        &str, // state
        &str, // grandparentTitle
    )> = Vec::new();

    let mut i = 0;
    while i < tags.len() {
        let tag = &tags[i];
        if tag.depth == 1 && (tag.name == "Video" || tag.name == "Track") && !tag.self_closing {
            let title = tag_attr(&tag.attrs, "title").unwrap_or("Unknown");
            let ptype = tag_attr(&tag.attrs, "type").unwrap_or("");
            let view_offset = tag_attr(&tag.attrs, "viewOffset")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            let duration = tag_attr(&tag.attrs, "duration")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            let grandparent = tag_attr(&tag.attrs, "grandparentTitle").unwrap_or("");

            // Look for User and Player child tags
            let mut user = "";
            let mut player = "";
            let mut player_state = "";
            let mut j = i + 1;
            while j < tags.len() && tags[j].depth > tag.depth {
                if tags[j].depth == tag.depth + 1 {
                    if tags[j].name == "User" {
                        user = tag_attr(&tags[j].attrs, "title").unwrap_or("");
                    } else if tags[j].name == "Player" {
                        player = tag_attr(&tags[j].attrs, "title").unwrap_or("");
                        player_state = tag_attr(&tags[j].attrs, "state").unwrap_or("playing");
                    }
                }
                j += 1;
            }
            i = j;
            sessions.push((
                title,
                ptype,
                user,
                player,
                view_offset,
                duration,
                player_state,
                grandparent,
            ));
        } else {
            i += 1;
        }
    }

    if sessions.is_empty() {
        return Ok("Nothing is currently playing on Plex.".to_string());
    }

    let mut out = "Now playing on Plex:\n".to_string();
    for (title, ptype, user, player, view_offset, duration, state, grandparent) in sessions {
        out.push_str(&format!("  **{title}**"));
        if ptype == "track" && !grandparent.is_empty() {
            out.push_str(&format!(" — {grandparent}"));
        }
        out.push_str(&format!(" [{}]", human_type(ptype)));
        out.push('\n');

        if !user.is_empty() {
            out.push_str(&format!("    User: {user}"));
            if !player.is_empty() {
                out.push_str(&format!(" on {player}"));
            }
            out.push('\n');
        }

        let progress = if duration > 0 {
            let pct = (view_offset as f64 / duration as f64 * 100.0) as u64;
            format!(
                "{} / {} ({}%)",
                format_duration_ms(view_offset),
                format_duration_ms(duration),
                pct
            )
        } else if view_offset > 0 {
            format!("{} elapsed", format_duration_ms(view_offset))
        } else {
            "paused".to_string()
        };
        out.push_str(&format!("    Progress: {progress} ({state})\n"));
    }
    Ok(out.trim_end().to_string())
}

// ── Utilities ─────────────────────────────────────────────────────────────────

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max])
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_basic_entities() {
        assert_eq!(decode_entities("Foo &amp; Bar"), "Foo & Bar");
        assert_eq!(decode_entities("a &lt; b"), "a < b");
        assert_eq!(decode_entities("a &gt; b"), "a > b");
        assert_eq!(decode_entities("&quot;hello&quot;"), "\"hello\"");
    }

    #[test]
    fn decode_no_entity_passthrough() {
        assert_eq!(decode_entities("Hello World"), "Hello World");
        assert_eq!(decode_entities(""), "");
    }

    #[test]
    fn parse_self_closing_tags() {
        let xml = r#"<MediaContainer size="2"><Video title="Inception" year="2010" type="movie" /><Directory title="Movies" type="movie" key="1" /></MediaContainer>"#;
        let tags = parse_xml_tags(xml);
        let videos: Vec<_> = tags.iter().filter(|t| t.name == "Video").collect();
        let dirs: Vec<_> = tags.iter().filter(|t| t.name == "Directory").collect();

        assert_eq!(videos.len(), 1);
        assert_eq!(tag_attr(&videos[0].attrs, "title"), Some("Inception"));
        assert_eq!(tag_attr(&videos[0].attrs, "year"), Some("2010"));

        assert_eq!(dirs.len(), 1);
        assert_eq!(tag_attr(&dirs[0].attrs, "title"), Some("Movies"));
        assert_eq!(tag_attr(&dirs[0].attrs, "type"), Some("movie"));
    }

    #[test]
    fn parse_nested_tags() {
        let xml = r#"<MediaContainer><Video title="Inception"><Player title="iPhone" /><User title="john" /></Video></MediaContainer>"#;
        let tags = parse_xml_tags(xml);

        let videos: Vec<_> = tags.iter().filter(|t| t.name == "Video").collect();
        assert_eq!(videos.len(), 1);
        assert_eq!(videos[0].depth, 1);
        assert!(!videos[0].self_closing);

        let users: Vec<_> = tags.iter().filter(|t| t.name == "User").collect();
        assert_eq!(users.len(), 1);
        assert_eq!(users[0].depth, 2);
        assert!(users[0].self_closing);
        assert_eq!(tag_attr(&users[0].attrs, "title"), Some("john"));
    }

    #[test]
    fn parse_search_response() {
        let xml = r#"<?xml version="1.0"?><MediaContainer size="2"><Video ratingKey="1" title="Inception" year="2010" summary="A thief" thumb="/thumb/1" type="movie" librarySectionTitle="Movies"/><Track ratingKey="2" title="Yellow Submarine" grandparentTitle="The Beatles" parentTitle="Revolver" type="track" librarySectionTitle="Music"/></MediaContainer>"#;
        let tags = parse_xml_tags(xml);

        let items: Vec<_> = tags
            .iter()
            .filter(|t| t.depth == 1 && t.self_closing)
            .collect();
        assert_eq!(items.len(), 2);

        let video = &items[0];
        assert_eq!(video.name, "Video");
        assert_eq!(tag_attr(&video.attrs, "title"), Some("Inception"));
        assert_eq!(
            tag_attr(&video.attrs, "librarySectionTitle"),
            Some("Movies")
        );

        let track = &items[1];
        assert_eq!(track.name, "Track");
        assert_eq!(tag_attr(&track.attrs, "title"), Some("Yellow Submarine"));
        assert_eq!(
            tag_attr(&track.attrs, "grandparentTitle"),
            Some("The Beatles")
        );
    }

    #[test]
    fn parse_sessions() {
        let xml = r#"<MediaContainer size="1"><Video ratingKey="10" title="Inception" type="movie" viewOffset="5400000" duration="8900000"><Player title="iPhone" /><User title="john" /></Video></MediaContainer>"#;
        let tags = parse_xml_tags(xml);

        let video = tags.iter().find(|t| t.name == "Video").unwrap();
        assert_eq!(video.depth, 1);
        assert!(!video.self_closing);

        let user = tags.iter().find(|t| t.name == "User").unwrap();
        assert_eq!(user.depth, 2);
        assert_eq!(tag_attr(&user.attrs, "title"), Some("john"));

        let player = tags.iter().find(|t| t.name == "Player").unwrap();
        assert_eq!(player.depth, 2);
        assert_eq!(tag_attr(&player.attrs, "title"), Some("iPhone"));
    }

    #[test]
    fn parse_angle_brackets_in_text() {
        let xml = r#"<MediaContainer><Video title="3 &lt; 5 &amp; 7 &gt; 1" type="movie"/></MediaContainer>"#;
        let tags = parse_xml_tags(xml);
        let video = tags.iter().find(|t| t.name == "Video").unwrap();
        assert_eq!(tag_attr(&video.attrs, "title"), Some("3 < 5 & 7 > 1"));
    }

    #[test]
    fn format_duration_basic() {
        assert_eq!(format_duration_ms(60000), "1m");
        assert_eq!(format_duration_ms(3600000), "1h");
        assert_eq!(format_duration_ms(90000), "1m 30s");
        assert_eq!(format_duration_ms(3660000), "1h 1m");
        assert_eq!(format_duration_ms(3725000), "1h 2m 5s");
        assert_eq!(format_duration_ms(30000), "30s");
    }

    #[test]
    fn format_date_iso() {
        assert_eq!(format_date("2024-01-15T14:30:00Z"), "2024-01-15");
        assert_eq!(format_date("2024-01-15"), "2024-01-15");
        assert_eq!(format_date("short"), "short");
    }

    #[test]
    fn url_encode_special() {
        let encoded = url_encode("Star Wars: Episode V");
        assert!(encoded.contains("Star%20Wars"));
    }

    #[test]
    fn human_type_mappings() {
        assert_eq!(human_type("movie"), "Movie");
        assert_eq!(human_type("episode"), "TV Episode");
        assert_eq!(human_type("track"), "Music Track");
        assert_eq!(human_type("artist"), "Artist");
        assert_eq!(human_type("unknown_thing"), "unknown_thing");
    }
}
