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

struct HomeAssistant;

impl Guest for HomeAssistant {
    fn call(tool: String, input_json: String) -> Result<String, String> {
        let input: Value =
            serde_json::from_str(&input_json).map_err(|e| format!("invalid input JSON: {e}"))?;

        match tool.as_str() {
            "homeassistant_list_entities" => list_entities(&input),
            "homeassistant_get_state" => get_state(&input),
            "homeassistant_call_service" => call_service(&input),
            "homeassistant_set_state" => set_state(&input),
            "homeassistant_list_automations" => list_automations(&input),
            "homeassistant_trigger_automation" => trigger_automation(&input),
            "homeassistant_set_timer" => set_timer(&input),
            "homeassistant_get_history" => get_history(&input),
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
    let url = host::config_read("url").map_err(|_| {
        "Home Assistant URL is not configured. Set it in the plugin settings.".to_string()
    })?;
    let token = host::secret_read("token").map_err(|_| {
        "Home Assistant access token is not configured. Set it in the plugin settings.".to_string()
    })?;
    Ok(Config {
        url: url.trim_end_matches('/').to_string(),
        token,
    })
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
        return Err(format!(
            "Home Assistant returned HTTP {}: {}",
            resp.status, resp.body
        ));
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

#[derive(Deserialize)]
struct HaHistoryEntry {
    state: String,
    last_changed: String,
}

// ── Tools ─────────────────────────────────────────────────────────────────────

fn fetch_room_info(
    config: &Config,
) -> Result<
    (
        HashMap<String, String>,
        HashMap<String, String>,
        HashMap<String, String>,
    ),
    String,
> {
    // /api/config/area_registry is WebSocket-only; use the template API instead.
    let areas_tmpl = r#"{% set ns = namespace(r=[]) %}{% for a in areas() %}{% set ns.r = ns.r + [{"area_id": a, "name": area_name(a)}] %}{% endfor %}{{ ns.r | to_json }}"#;
    let areas: Vec<AreaEntry> = ha_request(
        config,
        "POST",
        "/template",
        Some(serde_json::json!({"template": areas_tmpl})),
    )?;

    let area_names: HashMap<String, String> = areas
        .iter()
        .map(|a| (a.area_id.clone(), a.name.clone()))
        .collect();
    let name_to_area_id: HashMap<String, String> =
        areas.into_iter().map(|a| (a.name, a.area_id)).collect();

    let entities_tmpl = r#"{% set ns = namespace(r=[]) %}{% for a in areas() %}{% for e in area_entities(a) %}{% set ns.r = ns.r + [{"entity_id": e, "area_id": a}] %}{% endfor %}{% endfor %}{{ ns.r | to_json }}"#;
    let registry: Vec<EntityRegistryEntry> = ha_request(
        config,
        "POST",
        "/template",
        Some(serde_json::json!({"template": entities_tmpl})),
    )?;

    let entity_areas: HashMap<String, String> = registry
        .into_iter()
        .filter_map(|e| {
            e.area_id
                .and_then(|aid| area_names.get(&aid).cloned().map(|n| (e.entity_id, n)))
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
        Ok(info) => {
            room_warning = None;
            info
        }
        Err(e) => {
            room_warning = Some(format!("\n\n(Note: room grouping unavailable — {})", e));
            (HashMap::new(), HashMap::new(), HashMap::new())
        }
    };

    let prefix: Option<String> = domain_filter.map(|d| format!("{d}."));

    // room_name -> (area_id, [(entity_id, friendly_name, state)])
    let mut rooms: BTreeMap<String, (Option<String>, Vec<(String, String, String)>)> =
        BTreeMap::new();
    let mut unknown: Vec<(String, String, String)> = Vec::new();

    for s in states {
        if let Some(ref p) = prefix {
            if !s.entity_id.starts_with(p) {
                continue;
            }
        }

        let name = s
            .attributes
            .friendly_name
            .unwrap_or_else(|| s.entity_id.clone());

        let room = entity_areas.get(&s.entity_id).cloned().or_else(|| {
            s.attributes
                .area_id
                .as_ref()
                .and_then(|aid| area_names.get(aid).cloned())
        });

        let area_id = s
            .attributes
            .area_id
            .clone()
            .or_else(|| room.as_ref().and_then(|r| name_to_area_id.get(r).cloned()));

        match room {
            Some(r) => {
                let entry = rooms
                    .entry(r)
                    .or_insert_with(|| (area_id.clone(), Vec::new()));
                if entry.0.is_none() {
                    entry.0 = area_id.clone();
                }
                entry.1.push((s.entity_id, name, s.state));
            }
            None => unknown.push((s.entity_id, name, s.state)),
        }
    }

    for list in rooms.values_mut() {
        list.1.sort_by(|a, b| a.1.cmp(&b.1));
    }
    unknown.sort_by(|a, b| a.1.cmp(&b.1));

    if rooms.is_empty() && unknown.is_empty() {
        let domain_msg = domain_filter
            .map(|d| format!(" in domain '{d}'"))
            .unwrap_or_default();
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
                "No entities were matched — the entity_id or area_id may be wrong. ".to_string()
                    + "Use homeassistant_list_entities to find valid entity_ids first.",
            );
        }
        let mut affected = Vec::new();
        for entry in arr {
            let entity_id = entry
                .get("entity_id")
                .and_then(Value::as_str)
                .unwrap_or("?");
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

// ── set_state ─────────────────────────────────────────────────────────────────

fn resolve_service(
    domain: &str,
    attribute: &str,
    value: &Value,
) -> Result<(&'static str, &'static str, serde_json::Map<String, Value>), String> {
    let attr = attribute.to_lowercase();
    let mut data: serde_json::Map<String, Value> = serde_json::Map::new();

    macro_rules! with_key {
        ($svc_domain:expr, $svc:expr, $key:expr) => {{
            data.insert($key.into(), value.clone());
            Ok(($svc_domain, $svc, data))
        }};
    }
    macro_rules! no_data {
        ($svc_domain:expr, $svc:expr) => {{
            Ok(($svc_domain, $svc, data))
        }};
    }

    match domain {
        "light" => match attr.as_str() {
            "brightness"                       => with_key!("light", "turn_on", "brightness_pct"),
            "color_temp" | "colour_temp"       => with_key!("light", "turn_on", "color_temp"),
            "color_temp_kelvin" | "colour_temp_kelvin" => with_key!("light", "turn_on", "color_temp_kelvin"),
            "rgb_color" | "rgb" | "color" | "colour" => with_key!("light", "turn_on", "rgb_color"),
            "state" => {
                if value.as_str().map(|v| v.eq_ignore_ascii_case("off")).unwrap_or(false) {
                    no_data!("light", "turn_off")
                } else {
                    no_data!("light", "turn_on")
                }
            }
            _ => Err(format!("Unknown attribute '{attribute}' for light. Try: brightness, color_temp, rgb_color, state")),
        },

        "climate" => match attr.as_str() {
            "temperature"                       => with_key!("climate", "set_temperature", "temperature"),
            "mode" | "hvac_mode"                => with_key!("climate", "set_hvac_mode", "hvac_mode"),
            "fan_mode"                          => with_key!("climate", "set_fan_mode", "fan_mode"),
            "humidity"                          => with_key!("climate", "set_humidity", "humidity"),
            "preset_mode"                       => with_key!("climate", "set_preset_mode", "preset_mode"),
            _ => Err(format!("Unknown attribute '{attribute}' for climate. Try: temperature, mode, fan_mode, humidity, preset_mode")),
        },

        "cover" => match attr.as_str() {
            "position"      => with_key!("cover", "set_cover_position", "position"),
            "tilt_position" => with_key!("cover", "set_cover_tilt_position", "tilt_position"),
            "state" => {
                let v = value.as_str().map(|s| s.to_lowercase()).unwrap_or_default();
                match v.as_str() {
                    "close" | "closed" => no_data!("cover", "close_cover"),
                    "stop"             => no_data!("cover", "stop_cover"),
                    _                  => no_data!("cover", "open_cover"),
                }
            }
            _ => Err(format!("Unknown attribute '{attribute}' for cover. Try: position, tilt_position, state")),
        },

        "fan" => match attr.as_str() {
            "percentage" | "speed" => with_key!("fan", "set_percentage", "percentage"),
            "preset_mode"          => with_key!("fan", "set_preset_mode", "preset_mode"),
            "direction"            => with_key!("fan", "set_direction", "direction"),
            "state" => {
                if value.as_str().map(|v| v.eq_ignore_ascii_case("off")).unwrap_or(false) {
                    no_data!("fan", "turn_off")
                } else {
                    no_data!("fan", "turn_on")
                }
            }
            _ => Err(format!("Unknown attribute '{attribute}' for fan. Try: percentage/speed, preset_mode, state")),
        },

        "media_player" => match attr.as_str() {
            "volume" | "volume_level" => {
                // HA expects 0.0–1.0; accept 0–100 as well
                let normalized = value.as_f64().map(|n| {
                    if n > 1.0 { Value::from(n / 100.0) } else { value.clone() }
                }).unwrap_or_else(|| value.clone());
                data.insert("volume_level".into(), normalized);
                Ok(("media_player", "volume_set", data))
            }
            "source"     => with_key!("media_player", "select_source", "source"),
            "sound_mode" => with_key!("media_player", "select_sound_mode", "sound_mode"),
            _ => Err(format!("Unknown attribute '{attribute}' for media_player. Try: volume, source, sound_mode")),
        },

        "vacuum" => match attr.as_str() {
            "fan_speed" | "speed" | "mode" => with_key!("vacuum", "set_fan_speed", "fan_speed"),
            _ => Err(format!("Unknown attribute '{attribute}' for vacuum. Try: fan_speed/mode")),
        },

        "water_heater" => match attr.as_str() {
            "temperature"                    => with_key!("water_heater", "set_temperature", "temperature"),
            "mode" | "operation_mode"        => with_key!("water_heater", "set_operation_mode", "operation_mode"),
            "away_mode"                      => with_key!("water_heater", "set_away_mode", "away_mode"),
            _ => Err(format!("Unknown attribute '{attribute}' for water_heater. Try: temperature, mode")),
        },

        "input_number" => with_key!("input_number", "set_value", "value"),
        "number"       => with_key!("number", "set_value", "value"),
        "input_select" => with_key!("input_select", "select_option", "option"),
        "select"       => with_key!("select", "select_option", "option"),
        "input_text"   => with_key!("input_text", "set_value", "value"),
        "input_boolean" => {
            let is_off = value.as_str()
                .map(|v| v.eq_ignore_ascii_case("off") || v.eq_ignore_ascii_case("false"))
                .or_else(|| value.as_bool().map(|b| !b))
                .unwrap_or(false);
            if is_off { no_data!("input_boolean", "turn_off") }
            else      { no_data!("input_boolean", "turn_on")  }
        }

        _ => Err(format!(
            "set_state does not support domain '{domain}'. \
            Use homeassistant_call_service for custom service calls."
        )),
    }
}

fn set_state(input: &Value) -> Result<String, String> {
    let config = load_config()?;
    let entity_id = input
        .get("entity_id")
        .and_then(Value::as_str)
        .ok_or("entity_id is required")?;
    let attribute = input
        .get("attribute")
        .and_then(Value::as_str)
        .ok_or("attribute is required")?;
    let value = input.get("value").ok_or("value is required")?;

    validate_url_path_segment(entity_id, "entity_id")?;

    let domain = entity_id
        .split('.')
        .next()
        .ok_or("invalid entity_id: expected format domain.name")?;

    let (svc_domain, svc_name, mut service_data) = resolve_service(domain, attribute, value)?;
    service_data.insert("entity_id".into(), Value::String(entity_id.to_string()));

    let res: Value = ha_request(
        &config,
        "POST",
        &format!("/services/{svc_domain}/{svc_name}"),
        Some(Value::Object(service_data)),
    )?;

    let display_name = res
        .as_array()
        .and_then(|arr| {
            arr.iter()
                .find(|e| e.get("entity_id").and_then(Value::as_str) == Some(entity_id))
        })
        .and_then(|e| e.get("attributes"))
        .and_then(|a| a.get("friendly_name"))
        .and_then(Value::as_str)
        .unwrap_or(entity_id);

    Ok(format!("Set {display_name} {attribute} to {value}."))
}

// ── list_automations ──────────────────────────────────────────────────────────

fn list_automations(_input: &Value) -> Result<String, String> {
    let config = load_config()?;

    let states: Vec<HaStateBrief> = ha_request(&config, "GET", "/states", None)?;

    let mut automations: Vec<(String, String, String)> = Vec::new();
    let mut scripts: Vec<(String, String)> = Vec::new();
    let mut scenes: Vec<(String, String)> = Vec::new();

    for s in states {
        let name = s
            .attributes
            .friendly_name
            .unwrap_or_else(|| s.entity_id.clone());
        match s.entity_id.split('.').next() {
            Some("automation") => automations.push((s.entity_id, name, s.state)),
            Some("script") => scripts.push((s.entity_id, name)),
            Some("scene") => scenes.push((s.entity_id, name)),
            _ => {}
        }
    }

    if automations.is_empty() && scripts.is_empty() && scenes.is_empty() {
        return Ok("No automations, scripts, or scenes found.".to_string());
    }

    automations.sort_by(|a, b| a.1.cmp(&b.1));
    scripts.sort_by(|a, b| a.1.cmp(&b.1));
    scenes.sort_by(|a, b| a.1.cmp(&b.1));

    let mut result = String::new();

    if !automations.is_empty() {
        result.push_str("Automations:");
        for (entity_id, name, state) in &automations {
            let status = if state == "on" { "enabled" } else { "disabled" };
            result.push_str(&format!("\n  {name} ({entity_id}) — {status}"));
        }
    }
    if !scripts.is_empty() {
        if !result.is_empty() {
            result.push_str("\n\n");
        }
        result.push_str("Scripts:");
        for (entity_id, name) in &scripts {
            result.push_str(&format!("\n  {name} ({entity_id})"));
        }
    }
    if !scenes.is_empty() {
        if !result.is_empty() {
            result.push_str("\n\n");
        }
        result.push_str("Scenes:");
        for (entity_id, name) in &scenes {
            result.push_str(&format!("\n  {name} ({entity_id})"));
        }
    }

    Ok(result)
}

// ── trigger_automation ────────────────────────────────────────────────────────

fn trigger_automation(input: &Value) -> Result<String, String> {
    let config = load_config()?;
    let name = input
        .get("name")
        .and_then(Value::as_str)
        .ok_or("name is required")?;

    let (entity_id, domain) = if name.contains('.') {
        validate_url_path_segment(name, "name")?;
        let domain = name
            .split('.')
            .next()
            .ok_or("invalid entity_id")?
            .to_string();
        (name.to_string(), domain)
    } else {
        let states: Vec<HaStateBrief> = ha_request(&config, "GET", "/states", None)?;
        let needle = name.to_lowercase();

        let matched = states
            .into_iter()
            .filter(|s| {
                matches!(
                    s.entity_id.split('.').next(),
                    Some("automation" | "script" | "scene")
                )
            })
            .find(|s| {
                s.attributes
                    .friendly_name
                    .as_ref()
                    .map(|n| n.to_lowercase().contains(&needle))
                    .unwrap_or(false)
                    || s.entity_id.to_lowercase().contains(&needle)
            });

        match matched {
            Some(s) => {
                let domain = s.entity_id.split('.').next().unwrap_or("").to_string();
                (s.entity_id, domain)
            }
            None => {
                return Err(format!(
                    "No automation, script, or scene found matching '{name}'. \
                Use homeassistant_list_automations to see available options."
                ))
            }
        }
    };

    let (svc_domain, svc_name) = match domain.as_str() {
        "automation" => ("automation", "trigger"),
        "script" => ("script", "turn_on"),
        "scene" => ("scene", "turn_on"),
        other => return Err(format!("Cannot trigger entity of domain '{other}'")),
    };

    let mut body = serde_json::json!({"entity_id": entity_id});
    if domain == "automation" {
        body.as_object_mut()
            .unwrap()
            .insert("skip_condition".into(), Value::Bool(true));
    }

    let _: Value = ha_request(
        &config,
        "POST",
        &format!("/services/{svc_domain}/{svc_name}"),
        Some(body),
    )?;

    let verb = match (svc_domain, svc_name) {
        ("scene", _) => "Activated scene",
        ("automation", _) => "Triggered automation",
        _ => "Started script",
    };
    Ok(format!("{verb}: {entity_id}."))
}

// ── set_timer ─────────────────────────────────────────────────────────────────

fn parse_duration(s: &str) -> Result<u64, String> {
    let s = s.trim().to_lowercase();
    let mut total: u64 = 0;
    let mut chars = s.chars().peekable();

    while chars.peek().is_some() {
        while chars.peek().is_some_and(|c| c.is_whitespace() || *c == ',') {
            chars.next();
        }
        if chars.peek().is_none() {
            break;
        }

        let mut digits = String::new();
        while chars.peek().is_some_and(|c| c.is_ascii_digit()) {
            digits.push(chars.next().unwrap());
        }
        if digits.is_empty() {
            while chars.peek().is_some_and(|c| c.is_alphabetic()) {
                chars.next();
            }
            continue;
        }

        let n: u64 = digits
            .parse()
            .map_err(|_| format!("invalid number '{digits}'"))?;

        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }

        let mut unit = String::new();
        while chars.peek().is_some_and(|c| c.is_alphabetic()) {
            unit.push(chars.next().unwrap());
        }

        let mult = match unit.as_str() {
            "h" | "hr" | "hrs" | "hour" | "hours" => 3600u64,
            "m" | "min" | "mins" | "minute" | "minutes" => 60,
            "s" | "sec" | "secs" | "second" | "seconds" => 1,
            "" => 1,
            other => {
                return Err(format!(
                    "Unknown duration unit '{other}'. Use h, m, or s — e.g. '30m', '1h30m', '90s'."
                ))
            }
        };

        total = total.saturating_add(n.saturating_mul(mult));
    }

    if total == 0 {
        return Err(
            "Duration must be greater than 0. Examples: '30m', '1h', '90s', '1h30m'.".to_string(),
        );
    }
    Ok(total)
}

fn format_duration(secs: u64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    match (h, m, s) {
        (h, 0, 0) if h > 0 => format!("{h}h"),
        (0, m, 0) if m > 0 => format!("{m}m"),
        (0, 0, s) => format!("{s}s"),
        (h, m, 0) => format!("{h}h {m}m"),
        (0, m, s) => format!("{m}m {s}s"),
        (h, m, s) => format!("{h}h {m}m {s}s"),
    }
}

fn set_timer(input: &Value) -> Result<String, String> {
    let config = load_config()?;
    let entity_id = input
        .get("entity_id")
        .and_then(Value::as_str)
        .ok_or("entity_id is required")?;
    let duration_str = input
        .get("duration")
        .and_then(Value::as_str)
        .ok_or("duration is required")?;

    validate_url_path_segment(entity_id, "entity_id")?;

    let delay_seconds = parse_duration(duration_str)?;
    let display = format_duration(delay_seconds);

    let _: Value = ha_request(
        &config,
        "POST",
        "/events/plugin_set_timer",
        Some(serde_json::json!({
            "entity_id": entity_id,
            "delay_seconds": delay_seconds,
        })),
    )?;

    let setup = "Note: requires a one-time Home Assistant automation:\n\
        • Trigger: Event type = plugin_set_timer\n\
        • Action 1: Delay — {{ trigger.event.data.delay_seconds }} seconds\n\
        • Action 2: Service = homeassistant.turn_off, Entity = {{ trigger.event.data.entity_id }}";

    Ok(format!(
        "Timer set — {entity_id} will turn off in {display}.\n\n{setup}"
    ))
}

// ── get_history ───────────────────────────────────────────────────────────────

fn get_history(input: &Value) -> Result<String, String> {
    let config = load_config()?;
    let entity_id = input
        .get("entity_id")
        .and_then(Value::as_str)
        .ok_or("entity_id is required")?;
    let hours_back = input
        .get("hours_back")
        .and_then(Value::as_f64)
        .unwrap_or(24.0)
        .clamp(0.5, 168.0);

    validate_url_path_segment(entity_id, "entity_id")?;

    let hours = hours_back.ceil() as u64;
    // Wrap in JSON quotes so ha_request can deserialise the rendered string as a Rust String.
    let tmpl =
        format!(r#""{{{{ (now() - timedelta(hours={hours})).strftime('%Y-%m-%dT%H:%M:%S') }}}}""#);
    let start_time: String = ha_request(
        &config,
        "POST",
        "/template",
        Some(serde_json::json!({"template": tmpl})),
    )?;

    let path = format!(
        "/history/period/{}?filter_entity_id={}&minimal_response",
        start_time, entity_id
    );
    let history: Vec<Vec<HaHistoryEntry>> = ha_request(&config, "GET", &path, None)?;

    let entries: Vec<&HaHistoryEntry> = history.iter().flatten().collect();

    let hours_display = if hours_back < 1.0 {
        format!("{} minutes", (hours_back * 60.0) as u64)
    } else {
        format!("{hours_back:.0}h")
    };

    if entries.is_empty() {
        return Ok(format!(
            "No history found for {entity_id} in the last {hours_display}."
        ));
    }

    let mut result = format!("History for {entity_id} (last {hours_display}):");
    let mut prev_state: Option<String> = None;

    for entry in &entries {
        if prev_state.as_deref() == Some(entry.state.as_str()) {
            continue;
        }
        let time = entry
            .last_changed
            .split('T')
            .nth(1)
            .and_then(|t| t.get(..5))
            .unwrap_or(&entry.last_changed);
        result.push_str(&format!("\n  {time} → {}", entry.state));
        prev_state = Some(entry.state.clone());
    }

    Ok(result)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── validate_url_path_segment ─────────────────────────────────────────────

    #[test]
    fn path_segment_valid() {
        assert!(validate_url_path_segment("light.orb", "x").is_ok());
        assert!(validate_url_path_segment("automation.goodnight_routine", "x").is_ok());
        assert!(validate_url_path_segment("abc123", "x").is_ok());
    }

    #[test]
    fn path_segment_rejects_slash() {
        assert!(validate_url_path_segment("light/orb", "x").is_err());
        assert!(validate_url_path_segment("../../etc/passwd", "x").is_err());
    }

    #[test]
    fn path_segment_rejects_dotdot() {
        assert!(validate_url_path_segment("a..b", "x").is_err());
    }

    #[test]
    fn path_segment_rejects_empty() {
        assert!(validate_url_path_segment("", "x").is_err());
    }

    #[test]
    fn path_segment_rejects_non_ascii() {
        assert!(validate_url_path_segment("light.café", "x").is_err());
    }

    // ── parse_duration ────────────────────────────────────────────────────────

    #[test]
    fn duration_unit_suffixes() {
        assert_eq!(parse_duration("30m"), Ok(1800));
        assert_eq!(parse_duration("1h"), Ok(3600));
        assert_eq!(parse_duration("90s"), Ok(90));
        assert_eq!(parse_duration("1h30m"), Ok(5400));
        assert_eq!(parse_duration("2h15m30s"), Ok(8130));
    }

    #[test]
    fn duration_word_units() {
        assert_eq!(parse_duration("30 minutes"), Ok(1800));
        assert_eq!(parse_duration("2 hours"), Ok(7200));
        assert_eq!(parse_duration("45 seconds"), Ok(45));
        assert_eq!(parse_duration("1 hour 30 minutes"), Ok(5400));
    }

    #[test]
    fn duration_abbreviations() {
        assert_eq!(parse_duration("30min"), Ok(1800));
        assert_eq!(parse_duration("2hr"), Ok(7200));
        assert_eq!(parse_duration("10sec"), Ok(10));
        assert_eq!(parse_duration("2hrs"), Ok(7200));
        assert_eq!(parse_duration("30mins"), Ok(1800));
    }

    #[test]
    fn duration_bare_number_is_seconds() {
        assert_eq!(parse_duration("60"), Ok(60));
        assert_eq!(parse_duration("120"), Ok(120));
    }

    #[test]
    fn duration_case_insensitive() {
        assert_eq!(parse_duration("30M"), Ok(1800));
        assert_eq!(parse_duration("1H"), Ok(3600));
        assert_eq!(parse_duration("1 HOUR"), Ok(3600));
        assert_eq!(parse_duration("30 MINUTES"), Ok(1800));
    }

    #[test]
    fn duration_zero_is_error() {
        assert!(parse_duration("0").is_err());
        assert!(parse_duration("0m").is_err());
    }

    #[test]
    fn duration_empty_is_error() {
        assert!(parse_duration("").is_err());
        assert!(parse_duration("   ").is_err());
    }

    #[test]
    fn duration_unknown_unit_is_error() {
        assert!(parse_duration("5d").is_err());
        assert!(parse_duration("2w").is_err());
    }

    // ── format_duration ───────────────────────────────────────────────────────

    #[test]
    fn format_round_values() {
        assert_eq!(format_duration(3600), "1h");
        assert_eq!(format_duration(1800), "30m");
        assert_eq!(format_duration(90), "1m 30s");
        assert_eq!(format_duration(45), "45s");
        assert_eq!(format_duration(5400), "1h 30m");
        assert_eq!(format_duration(8130), "2h 15m 30s");
    }

    #[test]
    fn format_parse_roundtrip() {
        for secs in [30u64, 60, 90, 1800, 3600, 5400, 7261] {
            let s = format_duration(secs);
            assert_eq!(
                parse_duration(&s),
                Ok(secs),
                "roundtrip failed for {secs}s → {s:?}"
            );
        }
    }

    // ── resolve_service ───────────────────────────────────────────────────────

    fn v_num(n: f64) -> Value {
        Value::from(n)
    }
    fn v_str(s: &str) -> Value {
        Value::String(s.to_string())
    }

    #[test]
    fn light_brightness() {
        let (dom, svc, data) = resolve_service("light", "brightness", &v_num(50.0)).unwrap();
        assert_eq!((dom, svc), ("light", "turn_on"));
        assert_eq!(data["brightness_pct"], v_num(50.0));
    }

    #[test]
    fn light_state_off() {
        let (dom, svc, data) = resolve_service("light", "state", &v_str("off")).unwrap();
        assert_eq!((dom, svc), ("light", "turn_off"));
        assert!(data.is_empty());
    }

    #[test]
    fn light_state_on() {
        let (dom, svc, _) = resolve_service("light", "state", &v_str("on")).unwrap();
        assert_eq!((dom, svc), ("light", "turn_on"));
    }

    #[test]
    fn light_color_aliases() {
        for attr in ["rgb_color", "rgb", "color", "colour"] {
            let (_, svc, data) = resolve_service("light", attr, &v_str("255,0,0")).unwrap();
            assert_eq!(svc, "turn_on");
            assert!(data.contains_key("rgb_color"), "attr={attr}");
        }
    }

    #[test]
    fn climate_temperature() {
        let (dom, svc, data) = resolve_service("climate", "temperature", &v_num(21.0)).unwrap();
        assert_eq!((dom, svc), ("climate", "set_temperature"));
        assert_eq!(data["temperature"], v_num(21.0));
    }

    #[test]
    fn climate_mode_aliases() {
        for attr in ["mode", "hvac_mode"] {
            let (_, svc, data) = resolve_service("climate", attr, &v_str("cool")).unwrap();
            assert_eq!(svc, "set_hvac_mode");
            assert_eq!(data["hvac_mode"], v_str("cool"), "attr={attr}");
        }
    }

    #[test]
    fn cover_position() {
        let (dom, svc, data) = resolve_service("cover", "position", &v_num(50.0)).unwrap();
        assert_eq!((dom, svc), ("cover", "set_cover_position"));
        assert_eq!(data["position"], v_num(50.0));
    }

    #[test]
    fn cover_state_close() {
        let (_, svc, _) = resolve_service("cover", "state", &v_str("close")).unwrap();
        assert_eq!(svc, "close_cover");
        let (_, svc, _) = resolve_service("cover", "state", &v_str("closed")).unwrap();
        assert_eq!(svc, "close_cover");
    }

    #[test]
    fn fan_speed_aliases() {
        for attr in ["percentage", "speed"] {
            let (_, svc, data) = resolve_service("fan", attr, &v_num(75.0)).unwrap();
            assert_eq!(svc, "set_percentage");
            assert_eq!(data["percentage"], v_num(75.0), "attr={attr}");
        }
    }

    #[test]
    fn media_player_volume_normalises_over_one() {
        let (_, svc, data) = resolve_service("media_player", "volume", &v_num(50.0)).unwrap();
        assert_eq!(svc, "volume_set");
        let level = data["volume_level"].as_f64().unwrap();
        assert!((level - 0.5).abs() < 1e-9, "expected 0.5, got {level}");
    }

    #[test]
    fn media_player_volume_passthrough_under_one() {
        let (_, _, data) = resolve_service("media_player", "volume", &v_num(0.8)).unwrap();
        let level = data["volume_level"].as_f64().unwrap();
        assert!((level - 0.8).abs() < 1e-9, "expected 0.8, got {level}");
    }

    #[test]
    fn input_number_and_number() {
        for domain in ["input_number", "number"] {
            let (_, svc, data) = resolve_service(domain, "value", &v_num(42.0)).unwrap();
            assert_eq!(svc, "set_value");
            assert_eq!(data["value"], v_num(42.0), "domain={domain}");
        }
    }

    #[test]
    fn input_boolean_off_variants() {
        for val in [v_str("off"), v_str("false"), Value::Bool(false)] {
            let (_, svc, _) = resolve_service("input_boolean", "state", &val).unwrap();
            assert_eq!(svc, "turn_off", "val={val}");
        }
    }

    #[test]
    fn input_boolean_on_variants() {
        for val in [v_str("on"), v_str("true"), Value::Bool(true)] {
            let (_, svc, _) = resolve_service("input_boolean", "state", &val).unwrap();
            assert_eq!(svc, "turn_on", "val={val}");
        }
    }

    #[test]
    fn unknown_attribute_is_error() {
        assert!(resolve_service("light", "sparkle", &v_num(1.0)).is_err());
        assert!(resolve_service("climate", "colour", &v_str("red")).is_err());
    }

    #[test]
    fn unsupported_domain_is_error() {
        assert!(resolve_service("unknown_domain", "state", &v_str("on")).is_err());
    }
}
