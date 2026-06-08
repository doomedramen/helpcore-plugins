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

struct RandomFacts;

impl Guest for RandomFacts {
    fn call(tool: String, input_json: String) -> Result<String, String> {
        let input: Value = serde_json::from_str(&input_json)
            .map_err(|e| format!("invalid input JSON: {e}"))?;
        match tool.as_str() {
            "random_joke" => random_joke(&input),
            "random_fact" => random_fact(&input),
            "random_quote" => random_quote(&input),
            _ => Err(format!("unknown tool: {tool}")),
        }
    }
}

export!(RandomFacts);

fn get_str_opt<'a>(input: &'a Value, key: &str) -> Option<&'a str> {
    input.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
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

fn random_joke(input: &Value) -> Result<String, String> {
    let jtype = get_str_opt(input, "type").unwrap_or("general");
    match jtype {
        "programming" => {
            let body = http_get("https://official-joke-api.appspot.com/jokes/programming/random")?;
            let jokes: Vec<Value> = serde_json::from_str(&body).map_err(|e| format!("parse: {e}"))?;
            if let Some(j) = jokes.first() {
                Ok(format!("{}\n{}", j["setup"].as_str().unwrap_or(""), j["punchline"].as_str().unwrap_or("")))
            } else { Ok("Couldn't find a programming joke.".to_string()) }
        }
        "chuck" => {
            let body = http_get("https://api.chucknorris.io/jokes/random")?;
            let joke: Value = serde_json::from_str(&body).map_err(|e| format!("parse: {e}"))?;
            Ok(joke["value"].as_str().unwrap_or("No Chuck Norris joke found.").to_string())
        }
        _ => {
            let body = http_get("https://official-joke-api.appspot.com/jokes/random")?;
            let joke: Value = serde_json::from_str(&body).map_err(|e| format!("parse: {e}"))?;
            Ok(format!("{}\n{}", joke["setup"].as_str().unwrap_or(""), joke["punchline"].as_str().unwrap_or("")))
        }
    }
}

fn random_fact(input: &Value) -> Result<String, String> {
    let lang = get_str_opt(input, "language").unwrap_or("en");
    let body = http_get(&format!("https://uselessfacts.jsph.pl/api/v2/facts/random?language={lang}"))?;
    let fact: Value = serde_json::from_str(&body).map_err(|e| format!("parse: {e}"))?;
    Ok(fact["text"].as_str().unwrap_or("No fact found.").to_string())
}

fn random_quote(input: &Value) -> Result<String, String> {
    let tag = get_str_opt(input, "tag");
    let url = if let Some(t) = tag {
        format!("https://api.quotable.io/random?tags={}", t.replace(' ', "+"))
    } else {
        "https://api.quotable.io/random".to_string()
    };
    let body = match http_get(&url) {
        Ok(b) => b,
        Err(_) => http_get("https://api.quotable.io/random")?
    };
    let quote: Value = serde_json::from_str(&body).map_err(|e| format!("parse: {e}"))?;
    let q: Value = if quote.is_array() { quote[0].clone() } else { quote };
    let content = q["content"].as_str().unwrap_or("");
    let author = q["author"].as_str().unwrap_or("Unknown");
    Ok(format!("\"{content}\"\n— {author}"))
}
