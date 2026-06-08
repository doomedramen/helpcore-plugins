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

struct MoviesTv;

impl Guest for MoviesTv {
    fn call(tool: String, input_json: String) -> Result<String, String> {
        let input: Value =
            serde_json::from_str(&input_json).map_err(|e| format!("invalid input JSON: {e}"))?;
        match tool.as_str() {
            "movie_search" => movie_search(&input),
            "tv_search" => tv_search(&input),
            "movie_detail" => movie_detail(&input),
            "tv_detail" => tv_detail(&input),
            _ => Err(format!("unknown tool: {tool}")),
        }
    }
}

export!(MoviesTv);

fn get_str<'a>(input: &'a Value, key: &str) -> Result<&'a str, String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("{key} is required"))
}

fn get_u64_opt(input: &Value, key: &str) -> Option<u64> {
    input.get(key).and_then(|v| v.as_u64())
}

#[derive(Serialize)]
struct HttpReq<'a> {
    method: &'a str,
    url: String,
    headers: serde_json::Map<String, Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    body: Option<String>,
}
#[derive(Deserialize)]
struct HttpResp {
    status: u16,
    body: String,
}

fn http_get(url: &str) -> Result<String, String> {
    let mut headers = serde_json::Map::new();
    headers.insert(
        "Accept".to_string(),
        Value::String("application/json".to_string()),
    );
    let req = HttpReq {
        method: "GET",
        url: url.to_string(),
        headers,
        body: None,
    };
    let req_json = serde_json::to_string(&req).map_err(|e| format!("serialize: {e}"))?;
    let resp_json = host::http_request(&req_json)?;
    let resp: HttpResp =
        serde_json::from_str(&resp_json).map_err(|e| format!("parse HTTP: {e}"))?;
    if resp.status >= 400 {
        return Err(format!("HTTP {}: {}", resp.status, resp.body));
    }
    Ok(resp.body)
}

fn get_api_key() -> Result<String, String> {
    host::secret_read("api_key").map_err(|_| "TMDB API key not configured. Get a free key at https://www.themoviedb.org/settings/api and add it to your plugin settings.".to_string())
}

fn tmdb_get(path: &str) -> Result<String, String> {
    let key = get_api_key()?;
    let sep = if path.contains('?') { '&' } else { '?' };
    http_get(&format!(
        "https://api.themoviedb.org/3{path}{sep}api_key={key}"
    ))
}

fn format_rating(vote: f64, count: u64) -> String {
    format!("{:.1}/10 ({} votes)", vote, count)
}

fn poster_url(path: &str, size: &str) -> String {
    if path.is_empty() {
        String::new()
    } else {
        format!("https://image.tmdb.org/t/p/{size}{path}")
    }
}

fn movie_search(input: &Value) -> Result<String, String> {
    let query = get_str(input, "query")?;
    let year = get_u64_opt(input, "year");
    let mut url = format!("/search/movie?query={}", query.replace(' ', "+"));
    if let Some(y) = year {
        url.push_str(&format!("&primary_release_year={y}"));
    }

    let body = tmdb_get(&url)?;
    let data: Value = serde_json::from_str(&body).map_err(|e| format!("parse: {e}"))?;
    let results = data["results"]
        .as_array()
        .ok_or_else(|| format!("No movies found for '{}'.", query))?;

    let mut out = String::new();
    for movie in results.iter().take(5) {
        let id = movie["id"].as_u64().unwrap_or(0);
        let title = movie["title"].as_str().unwrap_or("");
        let date = movie["release_date"].as_str().unwrap_or("?");
        let year = date.get(..4).unwrap_or("?");
        let vote = movie["vote_average"].as_f64().unwrap_or(0.0);
        let count = movie["vote_count"].as_u64().unwrap_or(0);
        let overview = movie["overview"].as_str().unwrap_or("");
        let poster = poster_url(movie["poster_path"].as_str().unwrap_or(""), "w200");
        let overview = truncate(overview, 200);

        out.push_str(&format!(
            "**{title}** ({year}) [ID: {id}]\n  Rating: {}\n  {overview}\n  Poster: {poster}\n\n",
            format_rating(vote, count)
        ));
    }
    Ok(out.trim().to_string())
}

fn tv_search(input: &Value) -> Result<String, String> {
    let query = get_str(input, "query")?;
    let year = get_u64_opt(input, "year");
    let mut url = format!("/search/tv?query={}", query.replace(' ', "+"));
    if let Some(y) = year {
        url.push_str(&format!("&first_air_date_year={y}"));
    }

    let body = tmdb_get(&url)?;
    let data: Value = serde_json::from_str(&body).map_err(|e| format!("parse: {e}"))?;
    let results = data["results"]
        .as_array()
        .ok_or_else(|| format!("No TV shows found for '{}'.", query))?;

    let mut out = String::new();
    for show in results.iter().take(5) {
        let id = show["id"].as_u64().unwrap_or(0);
        let name = show["name"].as_str().unwrap_or("");
        let date = show["first_air_date"].as_str().unwrap_or("?");
        let year = date.get(..4).unwrap_or("?");
        let vote = show["vote_average"].as_f64().unwrap_or(0.0);
        let count = show["vote_count"].as_u64().unwrap_or(0);
        let overview = show["overview"].as_str().unwrap_or("");
        let poster = poster_url(show["poster_path"].as_str().unwrap_or(""), "w200");
        let origin = show["origin_country"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        let overview = truncate(overview, 200);

        out.push_str(&format!(
            "**{name}** ({year}) [ID: {id}]\n  Rating: {}\n  Origin: {origin}\n  {overview}\n  Poster: {poster}\n\n",
            format_rating(vote, count)
        ));
    }
    Ok(out.trim().to_string())
}

fn movie_detail(input: &Value) -> Result<String, String> {
    let movie_id = input
        .get("movie_id")
        .and_then(|v| v.as_u64())
        .ok_or("movie_id is required")?;
    let body = tmdb_get(&format!("/movie/{movie_id}?append_to_response=credits"))?;
    let m: Value = serde_json::from_str(&body).map_err(|e| format!("parse: {e}"))?;

    let title = m["title"].as_str().unwrap_or("");
    let tagline = m["tagline"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(|t| format!("_{t}_\n"))
        .unwrap_or_default();
    let date = m["release_date"].as_str().unwrap_or("?");
    let runtime = m["runtime"].as_u64().unwrap_or(0);
    let vote = m["vote_average"].as_f64().unwrap_or(0.0);
    let count = m["vote_count"].as_u64().unwrap_or(0);
    let budget = m["budget"].as_u64().unwrap_or(0);
    let revenue = m["revenue"].as_u64().unwrap_or(0);
    let overview = m["overview"].as_str().unwrap_or("");

    let genres: Vec<&str> = m["genres"]
        .as_array()
        .map(|a| a.iter().filter_map(|g| g["name"].as_str()).collect())
        .unwrap_or_default();
    let countries: Vec<&str> = m["production_countries"]
        .as_array()
        .map(|a| a.iter().filter_map(|c| c["name"].as_str()).collect())
        .unwrap_or_default();

    let cast: Vec<String> = m["credits"]["cast"]
        .as_array()
        .map(|a| {
            a.iter()
                .take(10)
                .map(|c| {
                    let name = c["name"].as_str().unwrap_or("");
                    let character = c["character"].as_str().unwrap_or("");
                    format!("{name} as {character}")
                })
                .collect()
        })
        .unwrap_or_default();

    let director = m["credits"]["crew"]
        .as_array()
        .map(|a| {
            a.iter()
                .find(|c| c["job"].as_str() == Some("Director"))
                .and_then(|c| c["name"].as_str())
                .unwrap_or("")
        })
        .unwrap_or("");

    let poster = poster_url(m["poster_path"].as_str().unwrap_or(""), "w300");

    let mut out = format!("**{title}** ({date})\n{tagline}");
    if !director.is_empty() {
        out.push_str(&format!("Director: {director}\n"));
    }
    out.push_str(&format!("Rating: {}\n", format_rating(vote, count)));
    out.push_str(&format!("Runtime: {runtime} min\n"));
    out.push_str(&format!("Genres: {}\n", genres.join(", ")));
    out.push_str(&format!("Countries: {}\n", countries.join(", ")));
    if budget > 0 {
        out.push_str(&format!("Budget: ${}\n", format_money(budget)));
    }
    if revenue > 0 {
        out.push_str(&format!("Revenue: ${}\n", format_money(revenue)));
    }
    out.push_str(&format!("\n{overview}\n\n"));
    out.push_str(&format!("Cast:\n{}\n\n", cast.join("\n")));
    out.push_str(&format!("Poster: {poster}"));

    Ok(out)
}

fn tv_detail(input: &Value) -> Result<String, String> {
    let tv_id = input
        .get("tv_id")
        .and_then(|v| v.as_u64())
        .ok_or("tv_id is required")?;
    let body = tmdb_get(&format!("/tv/{tv_id}"))?;
    let m: Value = serde_json::from_str(&body).map_err(|e| format!("parse: {e}"))?;

    let name = m["name"].as_str().unwrap_or("");
    let date = m["first_air_date"].as_str().unwrap_or("?");
    let last = m["last_air_date"].as_str().unwrap_or("?");
    let status = m["status"].as_str().unwrap_or("");
    let seasons = m["number_of_seasons"].as_u64().unwrap_or(0);
    let episodes = m["number_of_episodes"].as_u64().unwrap_or(0);
    let runtime = m["episode_run_time"]
        .as_array()
        .and_then(|a| a.first())
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let vote = m["vote_average"].as_f64().unwrap_or(0.0);
    let count = m["vote_count"].as_u64().unwrap_or(0);
    let overview = m["overview"].as_str().unwrap_or("");

    let genres: Vec<&str> = m["genres"]
        .as_array()
        .map(|a| a.iter().filter_map(|g| g["name"].as_str()).collect())
        .unwrap_or_default();
    let networks: Vec<&str> = m["networks"]
        .as_array()
        .map(|a| a.iter().filter_map(|n| n["name"].as_str()).collect())
        .unwrap_or_default();
    let created_by: Vec<&str> = m["created_by"]
        .as_array()
        .map(|a| a.iter().filter_map(|c| c["name"].as_str()).collect())
        .unwrap_or_default();

    let poster = poster_url(m["poster_path"].as_str().unwrap_or(""), "w300");

    let mut out = format!("**{name}** ({date} — {last})\n");
    out.push_str(&format!("Status: {status}\n"));
    out.push_str(&format!("Rating: {}\n", format_rating(vote, count)));
    out.push_str(&format!(
        "Seasons: {seasons} | Episodes: {episodes} | Runtime: {runtime} min\n"
    ));
    out.push_str(&format!("Genres: {}\n", genres.join(", ")));
    if !networks.is_empty() {
        out.push_str(&format!("Networks: {}\n", networks.join(", ")));
    }
    if !created_by.is_empty() {
        out.push_str(&format!("Created by: {}\n", created_by.join(", ")));
    }
    out.push_str(&format!("\n{overview}\n\n"));
    out.push_str(&format!("Poster: {poster}"));

    Ok(out)
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max])
    }
}

fn format_money(n: u64) -> String {
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
