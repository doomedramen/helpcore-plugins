use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};

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

/// Reject values that could escape the intended API path via path traversal.
/// Allows any non-empty ASCII string that contains neither '/' nor "..".
fn validate_url_path_segment(value: &str, field: &str) -> Result<(), String> {
    if value.is_empty() || value.contains('/') || value.contains("..") || !value.is_ascii() {
        return Err(format!(
            "invalid {field} '{value}': must be non-empty ASCII without '/' or '..'"
        ));
    }
    Ok(())
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
    #[serde(default)]
    area_id: Option<String>,
}

#[derive(Deserialize)]
struct AreaEntry {
    area_id: String,
    name: String,
}

#[derive(Deserialize)]
struct EntityRegistryEntry {
    entity_id: String,
    #[serde(default)]
    area_id: Option<String>,
}

// ── Tools ─────────────────────────────────────────────────────────────────────

fn fetch_room_info(
    config: &Config,
) -> Result<(HashMap<String, String>, HashMap<String, String>, HashMap<String, String>), String> {
    let areas: Vec<AreaEntry> = ha_request(config, "GET", "/config/area_registry", None)?;
    let area_names: HashMap<String, String> = areas.iter()
        .map(|a| (a.area_id.clone(), a.name.clone()))
        .collect();
    let name_to_area_id: HashMap<String, String> = areas.into_iter()
        .map(|a| (a.name, a.area_id))
        .collect();

    let registry: Vec<EntityRegistryEntry> = ha_request(config, "GET", "/config/entity_registry", None)?;
    let entity_areas: HashMap<String, String> = registry.into_iter()
        .filter_map(|e| {
            e.area_id.and_then(|aid| area_names.get(&aid).cloned().map(|n| (e.entity_id, n)))
        })
        .collect();

    Ok((area_names, entity_areas, name_to_area_id))
}

fn list_entities(input: &Value) -> Result<String, String> {
    let config = load_config()?;
    let domain_filter = input.get("domain").and_then(Value::as_str);

    let states: Vec<HaStateBrief> = ha_request(&config, "GET", "/states", None)?;

    let room_warning: Option<String>;
    let (area_names, entity_areas, name_to_area_id) = match fetch_room_info(&config) {
        Ok(info) => { room_warning = None; info }
        Err(e) => {
            room_warning = Some(format!("\n\n(Note: room grouping unavailable — {})", e));
            (HashMap::new(), HashMap::new(), HashMap::new())
        }
    };

    let prefix: Option<String> = domain_filter.map(|d| format!("{d}."));

    // room_name -> (area_id, [(entity_id, friendly_name, state)])
    let mut rooms: BTreeMap<String, (Option<String>, Vec<(String, String, String)>)> = BTreeMap::new();
    let mut unknown: Vec<(String, String, String)> = Vec::new();

    for s in states {
        if let Some(ref p) = prefix {
            if !s.entity_id.starts_with(p) {
                continue;
            }
        }

        let name = s.attributes.friendly_name.unwrap_or_else(|| s.entity_id.clone());

        let room = entity_areas.get(&s.entity_id)
            .cloned()
            .or_else(|| s.attributes.area_id.as_ref().and_then(|aid| area_names.get(aid).cloned()));

        let area_id = s.attributes.area_id.clone()
            .or_else(|| room.as_ref().and_then(|r| name_to_area_id.get(r).cloned()));

        match room {
            Some(r) => {
                let entry = rooms.entry(r).or_insert_with(|| (area_id.clone(), Vec::new()));
                if entry.0.is_none() {
                    entry.0 = area_id.clone();
                }
                entry.1.push((s.entity_id, name, s.state));
            },
            None => unknown.push((s.entity_id, name, s.state)),
        }
    }

    for list in rooms.values_mut() {
        list.1.sort_by(|a, b| a.1.cmp(&b.1));
    }
    unknown.sort_by(|a, b| a.1.cmp(&b.1));

    if rooms.is_empty() && unknown.is_empty() {
        let domain_msg = domain_filter.map(|d| format!(" in domain '{d}'")).unwrap_or_default();
        return Ok(format!("No entities found{domain_msg}."));
    }

    let header = match domain_filter {
        Some(d) => format!("Home Assistant entities (domain: {d}):"),
        None => "Home Assistant entities:".to_string(),
    };

    let mut result = String::new();
    result.push_str(&header);

    for (room, (area_id, entities)) in &rooms {
        match area_id {
            Some(aid) => result.push_str(&format!("\n\n{} (area_id: {}):", room, aid)),
            None => result.push_str(&format!("\n\n{}:", room)),
        }
        for (entity_id, name, state) in entities {
            result.push_str(&format!("\n  {} ({}) — {}", name, entity_id, state));
        }
    }

    if !unknown.is_empty() {
        result.push_str("\n\nOther:");
        for (entity_id, name, state) in &unknown {
            result.push_str(&format!("\n  {} ({}) — {}", name, entity_id, state));
        }
    }

    if let Some(w) = room_warning {
        result.push_str(&w);
    }
    Ok(result)
}

fn get_state(input: &Value) -> Result<String, String> {
    let config = load_config()?;
    let entity_id = input
        .get("entity_id")
        .and_then(Value::as_str)
        .ok_or("entity_id is required")?;
    validate_url_path_segment(entity_id, "entity_id")?;

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
    validate_url_path_segment(domain, "domain")?;
    validate_url_path_segment(service, "service")?;

    let mut body = serde_json::Map::new();
    if let Some(entity_id) = input.get("entity_id").and_then(Value::as_str) {
        body.insert("entity_id".into(), Value::String(entity_id.to_string()));
    }
    if let Some(area_id) = input.get("area_id").and_then(Value::as_str) {
        body.insert("area_id".into(), Value::String(area_id.to_string()));
    }
    if let Some(service_data) = input.get("service_data").and_then(Value::as_object) {
        for (k, v) in service_data {
            body.insert(k.clone(), v.clone());
        }
    }

    let res: Value = ha_request(
        &config,
        "POST",
        &format!("/services/{domain}/{service}"),
        Some(Value::Object(body)),
    )?;

    if let Some(arr) = res.as_array() {
        if arr.is_empty() && input.get("entity_id").or(input.get("area_id")).is_some() {
            return Err(
                "No entities were matched — the entity_id or area_id may be wrong. "
                    .to_string()
                    + "Use homeassistant_list_entities to find valid entity_ids first.",
            );
        }
        let mut affected = Vec::new();
        for entry in arr {
            let entity_id = entry.get("entity_id").and_then(Value::as_str).unwrap_or("?");
            let friendly = entry
                .get("attributes")
                .and_then(|a| a.get("friendly_name"))
                .and_then(Value::as_str);
            match friendly {
                Some(f) => affected.push(format!("{} ({})", f, entity_id)),
                None => affected.push(entity_id.to_string()),
            }
        }
        if affected.is_empty() {
            return Ok(format!("Called {domain}.{service} — no entities affected."));
        }
        return Ok(format!(
            "Called {domain}.{service} on: {}.",
            affected.join(", ")
        ));
    }

    Ok(format!("Called {domain}.{service} successfully."))
}
