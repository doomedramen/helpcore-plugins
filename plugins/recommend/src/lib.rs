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

struct Recommend;

impl Guest for Recommend {
    fn call(tool: String, input_json: String) -> Result<String, String> {
        let input: Value = serde_json::from_str(&input_json)
            .map_err(|e| format!("invalid input JSON: {e}"))?;
        match tool.as_str() {
            "recommend" => recommend(&input),
            _ => Err(format!("unknown tool: {tool}")),
        }
    }
}

export!(Recommend);

fn get_str<'a>(input: &'a Value, key: &str) -> Result<&'a str, String> {
    input.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
        .ok_or_else(|| format!("{key} is required"))
}

fn get_u64_opt(input: &Value, key: &str) -> Option<u64> {
    input.get(key).and_then(|v| v.as_u64())
}

#[derive(Serialize)]
struct HttpReq<'a> { method: &'a str, url: String, headers: serde_json::Map<String, Value>, #[serde(skip_serializing_if = "Option::is_none")] body: Option<String> }
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

fn get_api_key() -> Option<String> {
    host::config_read("api_key").ok()
}

fn type_param(media_type: &str) -> Result<&'static str, String> {
    match media_type {
        "music" => Ok("music"),
        "movies" => Ok("movies"),
        "shows" => Ok("shows"),
        "books" => Ok("books"),
        "authors" => Ok("authors"),
        "games" => Ok("games"),
        "podcasts" => Ok("podcasts"),
        _ => Err(format!("Unknown type: {media_type}. Use: music, movies, shows, books, authors, games, or podcasts.")),
    }
}

fn recommend(input: &Value) -> Result<String, String> {
    let query = get_str(input, "query")?;
    let mtype = get_str(input, "type")?;
    let limit = get_u64_opt(input, "limit").unwrap_or(10).min(20);

    let type_str = type_param(&mtype.to_lowercase())?;

    let mut url = format!(
        "https://tastedive.com/api/similar?q={}&type={type_str}&limit={limit}&info=1",
        query.replace(' ', "+")
    );
    if let Some(key) = get_api_key() {
        url.push_str(&format!("&k={key}"));
    }

    let body = http_get(&url)?;
    let data: Value = serde_json::from_str(&body).map_err(|_| "TasteDive API returned an unexpected response. Try again later.".to_string())?;

    let info = &data["Similar"]["Info"];
    let name = info[0]["Name"].as_str().unwrap_or(query);
    let wtype = info[0]["Type"].as_str().unwrap_or(type_str);

    let similar = &data["Similar"]["Results"].as_array()
        .ok_or_else(|| format!("No recommendations found for '{}'.", query))?;

    let type_label = match type_str {
        "music" => "Artists", "movies" => "Movies", "shows" => "TV Shows",
        "books" => "Books", "authors" => "Authors", "games" => "Games",
        "podcasts" => "Podcasts", _ => "Items",
    };

    let mut out = format!("If you like **{name}** ({wtype}), try these {type_label}:\n\n");

    for item in similar.iter() {
        let item_name = item["Name"].as_str().unwrap_or("");
        let desc = item.get("wTeaser").or(item.get("yID")).and_then(Value::as_str).unwrap_or("");
        let desc = if desc.len() > 150 { format!("{}…", &desc[..147]) } else { desc.to_string() };

        out.push_str(&format!("**{item_name}**"));
        if !desc.is_empty() {
            out.push_str(&format!("\n  {desc}"));
        }
        out.push('\n');
    }

    Ok(out)
}
