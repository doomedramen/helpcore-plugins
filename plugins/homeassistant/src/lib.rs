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

struct HomeAssistant;

impl Guest for HomeAssistant {
    fn call(tool: String, input_json: String) -> Result<String, String> {
        let input: Value = serde_json::from_str(&input_json)
            .map_err(|e| format!("invalid input JSON: {e}"))?;

        match tool.as_str() {
            "homeassistant_list_entities" => list_entities(&input),
            "homeassistant_get_state"     => get_state(&input),
            "homeassistant_call_service"  => call_service(&input),
            _ => Err(format!("unknown tool: {tool}")),
        }
    }
}

export!(HomeAssistant);

// ── Config ────────────────────────────────────────────────────────────────────

struct Config {
    url: String,
    token: String,
}

fn load_config() -> Result<Config, String> {
    let url = host::config_read("url")
        .map_err(|_| "Home Assistant URL is not configured. Set it in the plugin settings.".to_string())?;
    let token = host::config_read("token")
        .map_err(|_| "Home Assistant access token is not configured. Set it in the plugin settings.".to_string())?;
    Ok(Config { url: url.trim_end_matches('/').to_string(), token })
}

// ── HTTP helpers ──────────────────────────────────────────────────────────────

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

// Generic: deserialise the HA response body directly into T, skipping unknown fields.
fn ha_request<T: for<'de> Deserialize<'de>>(
    config: &Config,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Result<T, String> {
    let mut headers = serde_json::Map::new();
    headers.insert(
        "Authorization".into(),
        Value::String(format!("Bearer {}", config.token)),
    );
    headers.insert(
        "Content-Type".into(),
        Value::String("application/json".into()),
    );

    let req = HttpRequest {
        method,
        url: format!("{}/api{}", config.url, path),
        headers,
        body: body.map(|b| serde_json::to_string(&b).unwrap_or_default()),
    };
    let req_json = serde_json::to_string(&req).map_err(|e| e.to_string())?;
    let resp_json = host::http_request(&req_json)?;
    let resp: HttpResponse = serde_json::from_str(&resp_json)
        .map_err(|e| format!("failed to parse HTTP response: {e}"))?;

    if resp.status == 401 {
        return Err("Unauthorized — check your Long-Lived Access Token in plugin settings.".into());
    }
    if resp.status >= 400 {
        return Err(format!("Home Assistant returned HTTP {}: {}", resp.status, resp.body));
    }

    serde_json::from_str(&resp.body).map_err(|e| format!("failed to parse HA response: {e}"))
}

// ── Typed HA state structs ────────────────────────────────────────────────────

// Minimal representation for list_entities — serde skips all other attribute
// fields rather than allocating them, which drastically reduces fuel usage.
#[derive(Deserialize)]
struct HaStateBrief {
    entity_id: String,
    state: String,
    attributes: HaAttributesBrief,
}

#[derive(Deserialize)]
struct HaAttributesBrief {
    friendly_name: Option<String>,
}

#[derive(Serialize)]
struct EntityListItem {
    entity_id: String,
    name: String,
    state: String,
}

// ── Tools ─────────────────────────────────────────────────────────────────────

fn list_entities(input: &Value) -> Result<String, String> {
    let config = load_config()?;
    let domain_filter = input.get("domain").and_then(Value::as_str);

    let states: Vec<HaStateBrief> = ha_request(&config, "GET", "/states", None)?;

    // Pre-compute the prefix once rather than formatting inside the hot loop.
    let prefix: Option<String> = domain_filter.map(|d| format!("{d}."));

    let items: Vec<EntityListItem> = states
        .into_iter()
        .filter(|s| {
            prefix.as_deref()
                .map(|p| s.entity_id.starts_with(p))
                .unwrap_or(true)
        })
        .map(|s| {
            let name = s.attributes.friendly_name
                .unwrap_or_else(|| s.entity_id.clone());
            EntityListItem { entity_id: s.entity_id, name, state: s.state }
        })
        .collect();

    serde_json::to_string(&items).map_err(|e| e.to_string())
}

fn get_state(input: &Value) -> Result<String, String> {
    let config = load_config()?;
    let entity_id = input
        .get("entity_id")
        .and_then(Value::as_str)
        .ok_or("entity_id is required")?;

    let state: Value = ha_request(&config, "GET", &format!("/states/{entity_id}"), None)?;
    serde_json::to_string(&state).map_err(|e| e.to_string())
}

fn call_service(input: &Value) -> Result<String, String> {
    let config = load_config()?;
    let domain = input
        .get("domain")
        .and_then(Value::as_str)
        .ok_or("domain is required")?;
    let service = input
        .get("service")
        .and_then(Value::as_str)
        .ok_or("service is required")?;

    let mut body = serde_json::Map::new();
    if let Some(entity_id) = input.get("entity_id").and_then(Value::as_str) {
        body.insert("entity_id".into(), Value::String(entity_id.to_string()));
    }
    if let Some(service_data) = input.get("service_data").and_then(Value::as_object) {
        for (k, v) in service_data {
            body.insert(k.clone(), v.clone());
        }
    }

    let _: Value = ha_request(
        &config,
        "POST",
        &format!("/services/{domain}/{service}"),
        Some(Value::Object(body)),
    )?;
    Ok(format!("Called {domain}.{service} successfully."))
}
