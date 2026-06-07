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

struct Proxmox;

impl Guest for Proxmox {
    fn call(tool: String, input_json: String) -> Result<String, String> {
        let input: Value = serde_json::from_str(&input_json)
            .map_err(|e| format!("invalid input JSON: {e}"))?;

        match tool.as_str() {
            "proxmox_list_nodes"            => list_nodes(&input),
            "proxmox_get_node_status"       => get_node_status(&input),
            "proxmox_list_guests"           => list_guests(&input),
            "proxmox_get_guest_status"      => get_guest_status(&input),
            "proxmox_get_guest_config"      => get_guest_config(&input),
            "proxmox_get_cluster_resources" => get_cluster_resources(&input),
            "proxmox_get_storage"           => get_storage(&input),
            "proxmox_get_version"           => get_version(&input),
            _ => Err(format!("unknown tool: {tool}")),
        }
    }
}

export!(Proxmox);

// ── Config ────────────────────────────────────────────────────────────────────

enum AuthMethod {
    Token { header: String },
    Password { username: String, password: String },
}

struct Config {
    host: String,
    auth: AuthMethod,
}

fn load_config() -> Result<Config, String> {
    let host = host::config_read("host")
        .map_err(|_| "Proxmox host URL is not configured. Set it in the plugin settings.".to_string())?;
    let host = host.trim_end_matches('/').to_string();

    // Try API token auth first
    if let (Ok(token_id), Ok(token_secret)) = (
        host::config_read("token_id"),
        host::config_read("token_secret"),
    ) {
        return Ok(Config {
            host,
            auth: AuthMethod::Token {
                header: format!("PVEAPIToken={}={}", token_id, token_secret),
            },
        });
    }

    // Fall back to username/password auth
    if let (Ok(username), Ok(password)) = (
        host::config_read("username"),
        host::config_read("password"),
    ) {
        return Ok(Config {
            host,
            auth: AuthMethod::Password { username, password },
        });
    }

    Err("No authentication configured. Set either:\n\
         • API token (token_id + token_secret), or\n\
         • Username + password\n\
         in the plugin settings.".to_string())
}

// ── Validation ────────────────────────────────────────────────────────────────

fn validate_node(node: &str) -> Result<(), String> {
    if node.is_empty() || node.contains('/') || node.contains("..") || !node.is_ascii() {
        return Err(format!(
            "invalid node name '{node}': must be non-empty ASCII without '/' or '..'"
        ));
    }
    Ok(())
}

// ── URL encoding ──────────────────────────────────────────────────────────────

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

/// Like url_encode but preserves the '@' character (Proxmox expects literal @ in usernames).
fn url_encode_preserve_at(s: &str) -> String {
    let mut result = String::new();
    for byte in s.bytes() {
        match byte {
            b'0'..=b'9' | b'A'..=b'Z' | b'a'..=b'z' | b'-' | b'_' | b'.' | b'~' | b'@' => {
                result.push(byte as char);
            }
            b' ' => result.push_str("%20"),
            _ => result.push_str(&format!("%{:02X}", byte)),
        }
    }
    result
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

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max { s } else { &s[..max] }
}

/// Issue an HTTP request to the PVE host, returning status code and body.
fn http_execute(
    host: &str,
    path: &str,
    method: &str,
    headers: serde_json::Map<String, Value>,
    body: Option<String>,
) -> Result<(u16, String), String> {
    let req = HttpRequest {
        method,
        url: format!("{host}/api2/json{path}"),
        headers,
        body,
    };
    let req_json = serde_json::to_string(&req).map_err(|e| e.to_string())?;
    let resp_json = host::http_request(&req_json)?;
    let resp: HttpResponse = serde_json::from_str(&resp_json)
        .map_err(|e| format!("failed to parse HTTP response: {e}"))?;
    Ok((resp.status, resp.body))
}

/// Obtain a fresh auth ticket from Proxmox (username + password).
/// Username should include the realm, e.g. "root@pam".
fn get_ticket(host: &str, username: &str, password: &str) -> Result<String, String> {
    let body = format!(
        "username={}&password={}",
        url_encode_preserve_at(username),
        url_encode(password),
    );

    let mut headers = serde_json::Map::new();
    headers.insert("Content-Type".into(), Value::String("application/x-www-form-urlencoded".into()));
    headers.insert("Accept".into(), Value::String("application/json".into()));

    let (status, resp_body) = http_execute(host, "/access/ticket", "POST", headers, Some(body))?;

    if status == 401 || status == 403 {
        return Err("Authentication failed — check your username, password, and realm in plugin settings.".into());
    }
    if status >= 400 {
        return Err(format!("Failed to get ticket (HTTP {status}): {}", truncate(&resp_body, 200)));
    }

    let val: Value = serde_json::from_str(&resp_body)
        .map_err(|e| format!("failed to parse ticket response: {e}"))?;
    val.get("data")
        .and_then(|d| d.get("ticket"))
        .and_then(Value::as_str)
        .map(String::from)
        .ok_or_else(|| "No ticket in auth response".to_string())
}

/// Build the authentication headers for a request.
fn pve_auth_headers(config: &Config) -> Result<serde_json::Map<String, Value>, String> {
    let mut headers = serde_json::Map::new();
    headers.insert("Accept".into(), Value::String("application/json".into()));

    match &config.auth {
        AuthMethod::Token { header } => {
            headers.insert("Authorization".into(), Value::String(header.clone()));
        }
        AuthMethod::Password { username, password } => {
            let ticket = get_ticket(&config.host, username, password)?;
            headers.insert("Cookie".into(), Value::String(format!("PVEAuthCookie={ticket}")));
        }
    }

    Ok(headers)
}

/// Make an authenticated PVE API request and return the `data` field.
fn pve_request(config: &Config, method: &str, path: &str) -> Result<Value, String> {
    let headers = pve_auth_headers(config)?;
    let (status, body) = http_execute(&config.host, path, method, headers, None)?;

    if status == 401 || status == 403 {
        return Err("Authentication failed — check your Proxmox credentials in plugin settings.".into());
    }
    if status == 595 {
        return Err("Connection refused — is the Proxmox host reachable and the port correct?".into());
    }
    if status >= 400 {
        if let Ok(val) = serde_json::from_str::<Value>(&body) {
            if let Some(errors) = val.get("errors") {
                return Err(format!(
                    "Proxmox error: {}",
                    serde_json::to_string(errors).unwrap_or_default()
                ));
            }
        }
        return Err(format!(
            "Proxmox returned HTTP {status} for {path}: {}",
            truncate(&body, 200)
        ));
    }

    let val: Value = serde_json::from_str(&body)
        .map_err(|e| format!("failed to parse Proxmox response: {e}"))?;

    Ok(val.get("data").cloned().unwrap_or(Value::Null))
}

// ── Formatting helpers ────────────────────────────────────────────────────────

fn fmt_bytes(bytes: u64) -> String {
    if bytes >= 1_099_511_627_776 {
        format!("{:.1} TB", bytes as f64 / 1_099_511_627_776.0)
    } else if bytes >= 1_073_741_824 {
        format!("{:.1} GB", bytes as f64 / 1_073_741_824.0)
    } else if bytes >= 1_048_576 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

fn fmt_uptime(secs: u64) -> String {
    let days = secs / 86400;
    let hours = (secs % 86400) / 3600;
    let mins = (secs % 3600) / 60;
    let s = secs % 60;
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {mins}m")
    } else if mins > 0 {
        if s > 0 {
            format!("{mins}m {s}s")
        } else {
            format!("{mins}m")
        }
    } else {
        format!("{s}s")
    }
}

fn fmt_percent(fraction: f64) -> String {
    format!("{:.1}%", fraction * 100.0)
}

fn node_str(val: &Value, key: &str) -> String {
    val.get(key).and_then(Value::as_str).unwrap_or("?").to_string()
}

// ── Tools ─────────────────────────────────────────────────────────────────────

// ── proxmox_list_nodes ────────────────────────────────────────────────────────

fn list_nodes(_input: &Value) -> Result<String, String> {
    let config = load_config()?;
    let data = pve_request(&config, "GET", "/nodes")?;

    let nodes: Vec<&Value> = data.as_array()
        .ok_or("unexpected response: expected array of nodes")?
        .iter()
        .collect();

    if nodes.is_empty() {
        return Ok("No nodes found in the cluster.".into());
    }

    let mut result = String::from("Proxmox Nodes:\n");
    for node in &nodes {
        let name = node_str(node, "node");
        let status = node_str(node, "status");
        let maxcpu = node.get("maxcpu").and_then(Value::as_u64).unwrap_or(0);
        let maxmem = node.get("maxmem").and_then(Value::as_u64).unwrap_or(0);
        let cpu_usage = node.get("cpu").and_then(Value::as_f64);
        let uptime = node.get("uptime").and_then(Value::as_u64);

        result.push_str(&format!("\n  {name} ({status})"));
        result.push_str(&format!("\n    CPU: {} cores", maxcpu));
        if let Some(c) = cpu_usage {
            result.push_str(&format!(" ({})", fmt_percent(c)));
        }
        result.push_str(&format!(" | RAM: {}", fmt_bytes(maxmem)));
        if let Some(u) = uptime {
            result.push_str(&format!(" | Up: {}", fmt_uptime(u)));
        }
    }
    result.push_str(&format!("\n\n{} node(s) total.", nodes.len()));
    Ok(result)
}

// ── proxmox_get_node_status ───────────────────────────────────────────────────

fn get_node_status(input: &Value) -> Result<String, String> {
    let config = load_config()?;
    let node = input.get("node").and_then(Value::as_str)
        .ok_or("node is required")?;
    validate_node(node)?;

    let data = pve_request(&config, "GET", &format!("/nodes/{node}/status"))?;

    let cpu_fraction = data.get("cpu").and_then(Value::as_f64).unwrap_or(0.0);
    let wait = data.get("wait").and_then(Value::as_f64);
    let loadavg = data.get("loadavg").and_then(|v| v.as_array());
    let uptime = data.get("uptime").and_then(Value::as_u64);
    let pveversion = data.get("pveversion").and_then(Value::as_str);
    let kversion = data.get("kversion").and_then(Value::as_str);

    let mem = &data["memory"];
    let swap = &data["swap"];
    let rootfs = &data["rootfs"];

    let cpuinfo = &data["cpuinfo"];
    let cpu_model = cpuinfo.get("model").and_then(Value::as_str).unwrap_or("?");
    let cpu_sockets = cpuinfo.get("sockets").and_then(Value::as_u64).unwrap_or(0);
    let cpu_cores = cpuinfo.get("cores").and_then(Value::as_u64).unwrap_or(0);

    let mut result = format!("Status for node '{node}':\n");
    result.push_str(&format!("\n  CPU: {cpu_model}"));
    result.push_str(&format!("\n  Sockets: {cpu_sockets} | Cores: {cpu_cores}"));
    result.push_str(&format!(
        "\n  Usage: {} (IO wait: {})",
        fmt_percent(cpu_fraction),
        wait.map_or_else(|| "n/a".into(), |w| fmt_percent(w))
    ));

    if let Some(la) = loadavg {
        let avg: Vec<&str> = la.iter().filter_map(Value::as_str).collect();
        if !avg.is_empty() {
            result.push_str(&format!("\n  Load: {}", avg.join(", ")));
        }
    }

    let mem_total = mem.get("total").and_then(Value::as_u64).unwrap_or(0);
    let mem_used = mem.get("used").and_then(Value::as_u64).unwrap_or(0);
    if mem_total > 0 {
        let mem_pct = (mem_used as f64 / mem_total as f64) * 100.0;
        result.push_str(&format!(
            "\n  Memory: {} / {} ({:.1}%)",
            fmt_bytes(mem_used),
            fmt_bytes(mem_total),
            mem_pct
        ));
    }

    let swap_total = swap.get("total").and_then(Value::as_u64).unwrap_or(0);
    if swap_total > 0 {
        let swap_used = swap.get("used").and_then(Value::as_u64).unwrap_or(0);
        let swap_pct = (swap_used as f64 / swap_total as f64) * 100.0;
        result.push_str(&format!(
            "\n  Swap: {} / {} ({:.1}%)",
            fmt_bytes(swap_used),
            fmt_bytes(swap_total),
            swap_pct
        ));
    }

    let root_total = rootfs.get("total").and_then(Value::as_u64).unwrap_or(0);
    if root_total > 0 {
        let root_used = rootfs.get("used").and_then(Value::as_u64).unwrap_or(0);
        let root_pct = (root_used as f64 / root_total as f64) * 100.0;
        result.push_str(&format!(
            "\n  Root disk: {} / {} ({:.1}%)",
            fmt_bytes(root_used),
            fmt_bytes(root_total),
            root_pct
        ));
    }

    if let Some(ver) = pveversion {
        result.push_str(&format!("\n  PVE version: {ver}"));
    }
    if let Some(ker) = kversion {
        result.push_str(&format!("\n  Kernel: {ker}"));
    }
    if let Some(u) = uptime {
        result.push_str(&format!("\n  Uptime: {}", fmt_uptime(u)));
    }

    Ok(result)
}

// ── proxmox_list_guests ───────────────────────────────────────────────────────

fn list_guests(input: &Value) -> Result<String, String> {
    let config = load_config()?;
    let filter_type = input.get("type").and_then(Value::as_str);
    let filter_node = input.get("node").and_then(Value::as_str);

    let nodes: Vec<String> = if let Some(node) = filter_node {
        validate_node(node)?;
        vec![node.to_string()]
    } else {
        let data = pve_request(&config, "GET", "/nodes")?;
        data.as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|n| n.get("node").and_then(Value::as_str).map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    };

    if nodes.is_empty() {
        return Ok("No nodes found.".into());
    }

    struct GuestEntry {
        vmid: u64,
        name: String,
        gtype: String,
        status: String,
        node: String,
        cpu: Option<f64>,
        maxcpu: Option<u64>,
        mem: Option<u64>,
        maxmem: Option<u64>,
        uptime: Option<u64>,
    }

    let mut guests: Vec<GuestEntry> = Vec::new();

    let include_qemu = filter_type.map_or(true, |t| t == "qemu");
    let include_lxc = filter_type.map_or(true, |t| t == "lxc");

    for node in &nodes {
        if include_qemu {
            if let Ok(data) = pve_request(&config, "GET", &format!("/nodes/{node}/qemu")) {
                if let Some(arr) = data.as_array() {
                    for vm in arr {
                        guests.push(GuestEntry {
                            vmid: vm.get("vmid").and_then(Value::as_u64).unwrap_or(0),
                            name: vm.get("name").and_then(Value::as_str).unwrap_or("?").into(),
                            gtype: "qemu".into(),
                            status: node_str(vm, "status"),
                            node: node.clone(),
                            cpu: vm.get("cpu").and_then(Value::as_f64),
                            maxcpu: vm.get("cpus").and_then(Value::as_u64),
                            mem: vm.get("mem").and_then(Value::as_u64),
                            maxmem: vm.get("maxmem").and_then(Value::as_u64),
                            uptime: vm.get("uptime").and_then(Value::as_u64),
                        });
                    }
                }
            }
        }
        if include_lxc {
            if let Ok(data) = pve_request(&config, "GET", &format!("/nodes/{node}/lxc")) {
                if let Some(arr) = data.as_array() {
                    for ct in arr {
                        guests.push(GuestEntry {
                            vmid: ct.get("vmid").and_then(Value::as_u64).unwrap_or(0),
                            name: ct.get("name").and_then(Value::as_str).unwrap_or("?").into(),
                            gtype: "lxc".into(),
                            status: node_str(ct, "status"),
                            node: node.clone(),
                            cpu: ct.get("cpu").and_then(Value::as_f64),
                            maxcpu: ct.get("cpus").and_then(Value::as_u64),
                            mem: ct.get("mem").and_then(Value::as_u64),
                            maxmem: ct.get("maxmem").and_then(Value::as_u64),
                            uptime: ct.get("uptime").and_then(Value::as_u64),
                        });
                    }
                }
            }
        }
    }

    if guests.is_empty() {
        let filt = match filter_type {
            Some(t) => format!(" of type '{t}'"),
            None => String::new(),
        };
        return Ok(format!("No guests found{filt}."));
    }

    guests.sort_by(|a, b| a.vmid.cmp(&b.vmid).then_with(|| a.node.cmp(&b.node)));

    let type_label = filter_type.unwrap_or("all");
    let mut result = format!("Proxmox Guests ({type_label}):");

    for g in &guests {
        let icon = if g.gtype == "qemu" { "VM" } else { "CT" };
        result.push_str(&format!("\n\n  [{icon}] {} — {} ({})", g.vmid, g.name, g.status));
        result.push_str(&format!("\n    Node: {}", g.node));
        if let Some(cpu) = g.cpu {
            result.push_str(&format!(
                "\n    CPU: {:.1}% of {} cores",
                cpu * 100.0,
                g.maxcpu.unwrap_or(0)
            ));
        }
        if let Some(mem) = g.mem {
            let maxmem = g.maxmem.unwrap_or(0);
            let pct = if maxmem > 0 {
                (mem as f64 / maxmem as f64) * 100.0
            } else {
                0.0
            };
            result.push_str(&format!(
                "\n    Memory: {} / {} ({:.1}%)",
                fmt_bytes(mem),
                fmt_bytes(maxmem),
                pct
            ));
        }
        if let Some(u) = g.uptime {
            result.push_str(&format!("\n    Uptime: {}", fmt_uptime(u)));
        }
    }

    Ok(result)
}

// ── proxmox_get_guest_status ──────────────────────────────────────────────────

fn get_guest_status(input: &Value) -> Result<String, String> {
    let config = load_config()?;
    let node = input.get("node").and_then(Value::as_str)
        .ok_or("node is required")?;
    let vmid = input.get("vmid").and_then(Value::as_u64)
        .ok_or("vmid is required")?;
    validate_node(node)?;

    let (data, gtype) = match pve_request(
        &config, "GET",
        &format!("/nodes/{node}/qemu/{vmid}/status/current"),
    ) {
        Ok(d) => (d, "qemu"),
        Err(_) => {
            let data = pve_request(
                &config, "GET",
                &format!("/nodes/{node}/lxc/{vmid}/status/current"),
            )
            .map_err(|_| {
                format!(
                    "Guest {vmid} not found on node '{node}'. \
                    Use proxmox_list_guests to find valid VMIDs and nodes."
                )
            })?;
            (data, "lxc")
        }
    };

    let status = node_str(&data, "status");
    let name = data.get("name").and_then(Value::as_str).unwrap_or("?");
    let icon = if gtype == "qemu" { "VM" } else { "CT" };

    let mut result = format!("Status for {icon} {vmid} ({name}) on {node}:\n");
    result.push_str(&format!("\n  State: {status}"));

    if let Some(cpu) = data.get("cpu").and_then(Value::as_f64) {
        let cpus = data.get("cpus").and_then(Value::as_u64);
        if let Some(c) = cpus {
            result.push_str(&format!("\n  CPU: {:.1}% of {} cores", cpu * 100.0, c));
        } else {
            result.push_str(&format!("\n  CPU: {:.1}%", cpu * 100.0));
        }
    }

    if let Some(mem) = data.get("mem").and_then(Value::as_u64) {
        let maxmem = data.get("maxmem").and_then(Value::as_u64).unwrap_or(0);
        let pct = if maxmem > 0 {
            (mem as f64 / maxmem as f64) * 100.0
        } else {
            0.0
        };
        result.push_str(&format!(
            "\n  Memory: {} / {} ({:.1}%)",
            fmt_bytes(mem),
            fmt_bytes(maxmem),
            pct
        ));
    }

    let swap_val = data.get("swap").and_then(Value::as_u64).unwrap_or(0);
    let maxswap = data.get("maxswap").and_then(Value::as_u64).unwrap_or(0);
    if maxswap > 0 {
        let swap_pct = (swap_val as f64 / maxswap as f64) * 100.0;
        result.push_str(&format!(
            "\n  Swap: {} / {} ({:.1}%)",
            fmt_bytes(swap_val),
            fmt_bytes(maxswap),
            swap_pct
        ));
    }

    if let Some(disk) = data.get("disk").and_then(Value::as_u64) {
        let maxdisk = data.get("maxdisk").and_then(Value::as_u64).unwrap_or(0);
        if maxdisk > 0 {
            result.push_str(&format!(
                "\n  Disk: {} / {}",
                fmt_bytes(disk),
                fmt_bytes(maxdisk)
            ));
        }
    }

    if let Some(read) = data.get("diskread").and_then(Value::as_u64) {
        let write = data.get("diskwrite").and_then(Value::as_u64).unwrap_or(0);
        result.push_str(&format!(
            "\n  Disk IO: {} read / {} write",
            fmt_bytes(read),
            fmt_bytes(write)
        ));
    }

    if let Some(netin) = data.get("netin").and_then(Value::as_u64) {
        let netout = data.get("netout").and_then(Value::as_u64).unwrap_or(0);
        result.push_str(&format!(
            "\n  Network: {} in / {} out",
            fmt_bytes(netin),
            fmt_bytes(netout)
        ));
    }

    if let Some(u) = data.get("uptime").and_then(Value::as_u64) {
        result.push_str(&format!("\n  Uptime: {}", fmt_uptime(u)));
    }
    if let Some(pid) = data.get("pid").and_then(Value::as_u64) {
        result.push_str(&format!("\n  PID: {pid}"));
    }
    if let Some(qmp) = data.get("qmpstatus").and_then(Value::as_str) {
        result.push_str(&format!("\n  QMP: {qmp}"));
    }
    if let Some(lock) = data.get("lock").and_then(Value::as_str) {
        result.push_str(&format!("\n  Lock: {lock}"));
    }

    Ok(result)
}

// ── proxmox_get_guest_config ──────────────────────────────────────────────────

fn format_config_value(key: &str, val: &Value) -> String {
    match val {
        Value::Number(n) => {
            let lower = key.to_lowercase();
            if lower == "memory" || lower == "balloon" || lower == "swap" {
                if let Some(mb) = n.as_u64() {
                    return fmt_bytes(mb.saturating_mul(1_048_576));
                }
            }
            n.to_string()
        }
        Value::String(s) => s.clone(),
        // LXC raw config entries come as [[key, val], [key, val], ...]
        Value::Array(arr) if key == "lxc" => {
            let mut lines = String::new();
            for entry in arr {
                if let Some(pair) = entry.as_array().and_then(|a| {
                    let k = a.first()?.as_str()?;
                    let v = a.get(1).and_then(Value::as_str).unwrap_or("?");
                    Some((k, v))
                }) {
                    lines.push_str(&format!("\n      {} = {}", pair.0, pair.1));
                }
            }
            lines
        }
        other => serde_json::to_string(other).unwrap_or_else(|_| "?".into()),
    }
}

fn get_guest_config(input: &Value) -> Result<String, String> {
    let config = load_config()?;
    let node = input.get("node").and_then(Value::as_str)
        .ok_or("node is required")?;
    let vmid = input.get("vmid").and_then(Value::as_u64)
        .ok_or("vmid is required")?;
    validate_node(node)?;

    let (data, gtype) = match pve_request(
        &config, "GET",
        &format!("/nodes/{node}/qemu/{vmid}/config"),
    ) {
        Ok(d) => (d, "qemu"),
        Err(_) => {
            let data = pve_request(
                &config, "GET",
                &format!("/nodes/{node}/lxc/{vmid}/config"),
            )
            .map_err(|_| {
                format!(
                    "Guest {vmid} not found on node '{node}'. \
                    Use proxmox_list_guests to find valid VMIDs and nodes."
                )
            })?;
            (data, "lxc")
        }
    };

    let icon = if gtype == "qemu" { "VM" } else { "CT" };
    let mut result = format!("Configuration for {icon} {vmid} on {node}:\n");

    let obj = match data.as_object() {
        Some(o) => o,
        None => {
            return Ok(format!(
                "Config for {vmid}: {}",
                serde_json::to_string(&data).unwrap_or_default()
            ));
        }
    };

    let priority_keys = [
        "name", "description", "hostname",
        "ostype", "ostemplate",
        "cores", "sockets", "vcpus", "cpuunits", "cpulimit",
        "memory", "balloon", "swap",
        "boot", "bootdisk",
        "onboot", "startup",
        "agent", "bios", "machine",
        "unprivileged", "protection",
        "template", "tags",
        "features", "timezone", "arch",
        "lxc",  // raw LXC config entries (device passthrough, cgroups, etc.)
    ];

    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();

    for key in priority_keys.iter() {
        if let Some(val) = obj.get(*key) {
            seen.insert(key);
            result.push_str(&format!("\n  {key}: {}", format_config_value(key, val)));
        }
    }

    let mut remaining: Vec<&String> = obj.keys().filter(|k| !seen.contains(k.as_str())).collect();
    remaining.sort();

    let disk_prefixes = ["ide", "sata", "scsi", "virtio", "rootfs", "mp", "dev", "unused"];
    let net_prefixes = ["net"];

    let mut disk_keys: Vec<&String> = Vec::new();
    let mut net_keys: Vec<&String> = Vec::new();
    let mut other_keys: Vec<&String> = Vec::new();

    for k in &remaining {
        let lower = k.to_lowercase();
        if disk_prefixes.iter().any(|p| lower.starts_with(p))
            && k.chars().any(|c| c.is_ascii_digit())
        {
            disk_keys.push(k);
        } else if net_prefixes.iter().any(|p| lower.starts_with(p))
            && k.chars().any(|c| c.is_ascii_digit())
        {
            net_keys.push(k);
        } else {
            other_keys.push(k);
        }
    }

    if !disk_keys.is_empty() {
        result.push_str("\n\n  ── Disks & Mounts ──");
        for k in &disk_keys {
            if let Some(val) = obj.get(*k) {
                result.push_str(&format!("\n  {k}: {}", format_config_value(k, val)));
            }
        }
    }

    if !net_keys.is_empty() {
        result.push_str("\n\n  ── Network ──");
        for k in &net_keys {
            if let Some(val) = obj.get(*k) {
                result.push_str(&format!("\n  {k}: {}", format_config_value(k, val)));
            }
        }
    }

    if !other_keys.is_empty() {
        result.push_str("\n\n  ── Other ──");
        for k in &other_keys {
            if let Some(val) = obj.get(*k) {
                result.push_str(&format!("\n  {k}: {}", format_config_value(k, val)));
            }
        }
    }

    Ok(result)
}

// ── proxmox_get_cluster_resources ─────────────────────────────────────────────

fn get_cluster_resources(input: &Value) -> Result<String, String> {
    let config = load_config()?;
    let filter_type = input.get("type").and_then(Value::as_str);

    let path = match filter_type {
        Some(ft) => format!("/cluster/resources?type={ft}"),
        None => "/cluster/resources".into(),
    };

    let data = pve_request(&config, "GET", &path)?;
    let resources: Vec<&Value> = data
        .as_array()
        .map(|a| a.iter().collect())
        .unwrap_or_default();

    if resources.is_empty() {
        return Ok("No cluster resources found.".into());
    }

    let mut nodes: Vec<&&Value> = Vec::new();
    let mut qemu: Vec<&&Value> = Vec::new();
    let mut lxc: Vec<&&Value> = Vec::new();
    let mut storages: Vec<&&Value> = Vec::new();

    for r in &resources {
        match r.get("type").and_then(Value::as_str) {
            Some("node") => nodes.push(r),
            Some("qemu") => qemu.push(r),
            Some("lxc") => lxc.push(r),
            Some("storage") => storages.push(r),
            _ => {}
        }
    }

    let mut result = String::from("Proxmox Cluster Resources:\n");

    if !nodes.is_empty() {
        result.push_str("\n  Nodes:");
        for n in &nodes {
            let name = node_str(n, "node");
            let status = node_str(n, "status");
            let cpu = n.get("cpu").and_then(Value::as_f64)
                .map_or_else(String::new, |c| fmt_percent(c));
            let maxcpu = n.get("maxcpu").and_then(Value::as_u64).unwrap_or(0);
            let mem = n.get("mem").and_then(Value::as_u64).unwrap_or(0);
            let maxmem = n.get("maxmem").and_then(Value::as_u64).unwrap_or(0);
            let uptime = n.get("uptime").and_then(Value::as_u64)
                .map_or_else(String::new, |u| fmt_uptime(u));

            result.push_str(&format!("\n    {name} — {status}"));
            if !cpu.is_empty() {
                result.push_str(&format!(" | CPU: {cpu} ({maxcpu} cores)"));
            }
            result.push_str(&format!(" | RAM: {}", fmt_bytes(maxmem)));
            if maxmem > 0 {
                result.push_str(&format!(
                    " ({:.1}% used)",
                    (mem as f64 / maxmem as f64) * 100.0
                ));
            }
            if !uptime.is_empty() {
                result.push_str(&format!(" | Up: {uptime}"));
            }
        }
    }

    if !qemu.is_empty() {
        result.push_str("\n\n  Virtual Machines (QEMU):");
        let mut sorted: Vec<&&Value> = qemu.iter().copied().collect();
        sorted.sort_by(|a, b| {
            let va = a.get("vmid").and_then(Value::as_u64).unwrap_or(0);
            let vb = b.get("vmid").and_then(Value::as_u64).unwrap_or(0);
            va.cmp(&vb)
        });
        for v in &sorted {
            let vmid = v.get("vmid").and_then(Value::as_u64).unwrap_or(0);
            let name = node_str(v, "name");
            let status = node_str(v, "status");
            let node = node_str(v, "node");
            let cpu = v.get("cpu").and_then(Value::as_f64)
                .map_or_else(String::new, |c| fmt_percent(c));
            let maxcpu = v.get("maxcpu").and_then(Value::as_u64).unwrap_or(0);
            let mem = v.get("mem").and_then(Value::as_u64).unwrap_or(0);
            let maxmem = v.get("maxmem").and_then(Value::as_u64).unwrap_or(0);

            result.push_str(&format!("\n    VM {vmid} ({name}) — {status} @ {node}"));
            if status == "running" {
                if !cpu.is_empty() {
                    result.push_str(&format!(" | CPU: {cpu} ({maxcpu} cores)"));
                }
                result.push_str(&format!(
                    " | RAM: {} / {}",
                    fmt_bytes(mem),
                    fmt_bytes(maxmem)
                ));
            }
        }
    }

    if !lxc.is_empty() {
        result.push_str("\n\n  Containers (LXC):");
        let mut sorted: Vec<&&Value> = lxc.iter().copied().collect();
        sorted.sort_by(|a, b| {
            let va = a.get("vmid").and_then(Value::as_u64).unwrap_or(0);
            let vb = b.get("vmid").and_then(Value::as_u64).unwrap_or(0);
            va.cmp(&vb)
        });
        for c in &sorted {
            let vmid = c.get("vmid").and_then(Value::as_u64).unwrap_or(0);
            let name = node_str(c, "name");
            let status = node_str(c, "status");
            let node = node_str(c, "node");
            let cpu = c.get("cpu").and_then(Value::as_f64)
                .map_or_else(String::new, |c| fmt_percent(c));
            let maxcpu = c.get("maxcpu").and_then(Value::as_u64).unwrap_or(0);
            let mem = c.get("mem").and_then(Value::as_u64).unwrap_or(0);
            let maxmem = c.get("maxmem").and_then(Value::as_u64).unwrap_or(0);

            result.push_str(&format!("\n    CT {vmid} ({name}) — {status} @ {node}"));
            if status == "running" {
                if !cpu.is_empty() {
                    result.push_str(&format!(" | CPU: {cpu} ({maxcpu} cores)"));
                }
                result.push_str(&format!(
                    " | RAM: {} / {}",
                    fmt_bytes(mem),
                    fmt_bytes(maxmem)
                ));
            }
        }
    }

    if !storages.is_empty() {
        result.push_str("\n\n  Storage:");
        for s in &storages {
            let storage = node_str(s, "storage");
            let stype = node_str(s, "type");
            let node = node_str(s, "node");
            let total = s.get("maxdisk").and_then(Value::as_u64).unwrap_or(0);
            let used = s.get("disk").and_then(Value::as_u64).unwrap_or(0);
            let pct = if total > 0 {
                (used as f64 / total as f64) * 100.0
            } else {
                0.0
            };

            result.push_str(&format!(
                "\n    {storage} ({stype}) @ {node} | {} / {} ({pct:.1}%)",
                fmt_bytes(used),
                fmt_bytes(total)
            ));
        }
    }

    Ok(result)
}

// ── proxmox_get_storage ───────────────────────────────────────────────────────

fn get_storage(input: &Value) -> Result<String, String> {
    let config = load_config()?;
    let filter_node = input.get("node").and_then(Value::as_str);

    if let Some(node) = filter_node {
        validate_node(node)?;
        let data = pve_request(&config, "GET", &format!("/nodes/{node}/storage"))?;
        let storages: Vec<&Value> = data
            .as_array()
            .map(|a| a.iter().collect())
            .unwrap_or_default();
        if storages.is_empty() {
            return Ok(format!("No storage found on node '{node}'."));
        }
        return format_storage_list(&storages, &format!("Storage on {node}"));
    }

    // All nodes
    let nodes_data = pve_request(&config, "GET", "/nodes")?;
    let node_list: Vec<String> = nodes_data
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|n| n.get("node").and_then(Value::as_str).map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let mut all_storages: Vec<Value> = Vec::new();
    for node in &node_list {
        if let Ok(data) = pve_request(&config, "GET", &format!("/nodes/{node}/storage")) {
            if let Some(arr) = data.as_array() {
                all_storages.extend(arr.iter().cloned());
            }
        }
    }

    if all_storages.is_empty() {
        return Ok("No storage found.".into());
    }

    let refs: Vec<&Value> = all_storages.iter().collect();
    format_storage_list(&refs, "Cluster Storage")
}

fn format_storage_list(storages: &[&Value], title: &str) -> Result<String, String> {
    let mut result = format!("{title}:\n");

    for s in storages {
        let name = node_str(s, "storage");
        let stype = node_str(s, "type");
        let node = node_str(s, "node");
        let content = s.get("content").and_then(Value::as_str).unwrap_or("-");
        let shared = s.get("shared").and_then(Value::as_u64).unwrap_or(0);
        let status = s.get("status").and_then(Value::as_str).unwrap_or("available");
        let total = s.get("total").and_then(Value::as_u64).unwrap_or(0);
        let used = s.get("used").and_then(Value::as_u64).unwrap_or(0);
        let avail = s.get("avail").and_then(Value::as_u64).unwrap_or(0);

        result.push_str(&format!("\n  {name}"));
        result.push_str(&format!("\n    Type: {stype} @ {node} | Status: {status}"));
        if shared > 0 {
            result.push_str(" | Shared");
        }
        result.push_str(&format!("\n    Content: {content}"));
        if total > 0 {
            let pct = (used as f64 / total as f64) * 100.0;
            result.push_str(&format!(
                "\n    Space: {} / {} ({pct:.1}%)",
                fmt_bytes(used),
                fmt_bytes(total)
            ));
        }
        if avail > 0 && total == 0 {
            result.push_str(&format!("\n    Available: {}", fmt_bytes(avail)));
        }
    }

    result.push_str(&format!("\n\n{} storage pool(s).", storages.len()));
    Ok(result)
}

// ── proxmox_get_version ───────────────────────────────────────────────────────

fn get_version(input: &Value) -> Result<String, String> {
    let config = load_config()?;
    let filter_node = input.get("node").and_then(Value::as_str);

    if let Some(node) = filter_node {
        validate_node(node)?;
        let data = pve_request(&config, "GET", &format!("/nodes/{node}/version"))?;
        let version = node_str(&data, "version");
        let release = data.get("release").and_then(Value::as_str).unwrap_or("?");
        let repoid = data.get("repoid").and_then(Value::as_str).unwrap_or("?");
        return Ok(format!(
            "Proxmox VE on {node}:\n  Version: {version}\n  Release: {release}\n  Repository: {repoid}"
        ));
    }

    // All nodes
    let nodes_data = pve_request(&config, "GET", "/nodes")?;
    let node_list: Vec<String> = nodes_data
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|n| n.get("node").and_then(Value::as_str).map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let mut result = String::from("Proxmox VE Versions:");
    for node in &node_list {
        match pve_request(&config, "GET", &format!("/nodes/{node}/version")) {
            Ok(data) => {
                let ver = node_str(&data, "version");
                let rel = data.get("release").and_then(Value::as_str).unwrap_or("?");
                result.push_str(&format!("\n  {node}: {ver} ({rel})"));
            }
            Err(e) => {
                result.push_str(&format!("\n  {node}: unavailable — {e}"));
            }
        }
    }

    Ok(result)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_node_valid() {
        assert!(validate_node("pve").is_ok());
        assert!(validate_node("proxmox-node-1").is_ok());
        assert!(validate_node("node1").is_ok());
        assert!(validate_node("catacomb").is_ok());
        assert!(validate_node("catacomb2").is_ok());
    }

    #[test]
    fn validate_node_rejects_slash() {
        assert!(validate_node("node/foo").is_err());
        assert!(validate_node("../../etc").is_err());
    }

    #[test]
    fn validate_node_rejects_dotdot() {
        assert!(validate_node("node..evil").is_err());
    }

    #[test]
    fn validate_node_rejects_empty() {
        assert!(validate_node("").is_err());
    }

    #[test]
    fn validate_node_rejects_non_ascii() {
        assert!(validate_node("café").is_err());
    }

    #[test]
    fn url_encode_basic() {
        assert_eq!(url_encode("hello"), "hello");
        assert_eq!(url_encode("hello world"), "hello%20world");
        assert_eq!(url_encode("user@realm"), "user%40realm");
        assert_eq!(url_encode("a!b"), "a%21b");
    }

    #[test]
    fn fmt_bytes_units() {
        assert_eq!(fmt_bytes(0), "0 B");
        assert_eq!(fmt_bytes(1024), "1.0 KB");
        assert_eq!(fmt_bytes(1_048_576), "1.0 MB");
        assert_eq!(fmt_bytes(1_073_741_824), "1.0 GB");
        assert_eq!(fmt_bytes(1_099_511_627_776), "1.0 TB");
        assert_eq!(fmt_bytes(2_147_483_648), "2.0 GB");
        assert_eq!(fmt_bytes(536_870_912), "512.0 MB");
    }

    #[test]
    fn fmt_uptime_values() {
        assert_eq!(fmt_uptime(30), "30s");
        assert_eq!(fmt_uptime(60), "1m");
        assert_eq!(fmt_uptime(90), "1m 30s");
        assert_eq!(fmt_uptime(3600), "1h 0m");
        assert_eq!(fmt_uptime(3660), "1h 1m");
        assert_eq!(fmt_uptime(86400), "1d 0h");
        assert_eq!(fmt_uptime(90000), "1d 1h");
    }

    #[test]
    fn fmt_percent_values() {
        assert_eq!(fmt_percent(0.0), "0.0%");
        assert_eq!(fmt_percent(0.5), "50.0%");
        assert_eq!(fmt_percent(0.046), "4.6%");
        assert_eq!(fmt_percent(0.755), "75.5%");
        assert_eq!(fmt_percent(1.0), "100.0%");
        assert_eq!(fmt_percent(0.001), "0.1%");
    }

    #[test]
    fn url_encode_edge_cases() {
        assert_eq!(url_encode(""), "");
        assert_eq!(url_encode("abc123"), "abc123");
        assert_eq!(url_encode("test/path"), "test%2Fpath");
        assert_eq!(url_encode("colón"), "col%C3%B3n");
        assert_eq!(url_encode("P@ssw0rd!"), "P%40ssw0rd%21");
    }

    #[test]
    fn url_encode_preserve_at_values() {
        assert_eq!(url_encode_preserve_at("root@pam"), "root@pam");
        assert_eq!(url_encode_preserve_at("user@realm"), "user@realm");
        assert_eq!(url_encode_preserve_at("name with spaces@pam"), "name%20with%20spaces@pam");
        assert_eq!(url_encode_preserve_at(""), "");
    }

    #[test]
    fn fmt_bytes_edge_cases() {
        assert_eq!(fmt_bytes(1), "1 B");
        assert_eq!(fmt_bytes(512), "512 B");
        assert_eq!(fmt_bytes(2_048), "2.0 KB");
        assert_eq!(fmt_bytes(5_368_709_120), "5.0 GB");
        assert_eq!(fmt_bytes(17_592_186_044_416), "16.0 TB");
    }

    #[test]
    fn fmt_uptime_edge_cases() {
        assert_eq!(fmt_uptime(0), "0s");
        assert_eq!(fmt_uptime(1), "1s");
        assert_eq!(fmt_uptime(59), "59s");
        assert_eq!(fmt_uptime(61), "1m 1s");
        assert_eq!(fmt_uptime(3599), "59m 59s");
        assert_eq!(fmt_uptime(604800), "7d 0h");
        assert_eq!(fmt_uptime(1393559), "16d 3h");
    }

    #[test]
    fn format_config_value_memory_keys() {
        assert_eq!(
            format_config_value("memory", &Value::from(2048)),
            "2.0 GB"
        );
        assert_eq!(
            format_config_value("balloon", &Value::from(1024)),
            "1.0 GB"
        );
        assert_eq!(
            format_config_value("swap", &Value::from(512)),
            "512.0 MB"
        );
        // Non-memory keys stay as raw numbers
        assert_eq!(
            format_config_value("cores", &Value::from(4)),
            "4"
        );
    }

    #[test]
    fn format_config_value_strings() {
        assert_eq!(
            format_config_value("hostname", &Value::String("my-ct".into())),
            "my-ct"
        );
        assert_eq!(
            format_config_value("ostype", &Value::String("debian".into())),
            "debian"
        );
    }

    #[test]
    fn format_config_value_lxc_array() {
        let lxc_val = serde_json::json!([
            ["lxc.cgroup2.devices.allow", "c 226:* rwm"],
            ["lxc.mount.entry", "/dev/dri/renderD129 /dev/dri/renderD129 none bind,optional,create=file"]
        ]);
        let result = format_config_value("lxc", &lxc_val);
        assert!(result.contains("lxc.cgroup2.devices.allow = c 226:* rwm"));
        assert!(result.contains("lxc.mount.entry"));
        assert!(result.contains("renderD129"));
        // Leading newline before first entry; 2 content lines + empty leading = 3
        assert_eq!(result.lines().count(), 3);
    }

    #[test]
    fn format_config_value_non_lxc_array() {
        // A non-lxc array should be serialized as JSON
        let arr_val = serde_json::json!(["a", "b", "c"]);
        let result = format_config_value("tags", &arr_val);
        assert_eq!(result, r#"["a","b","c"]"#);
    }

    #[test]
    fn format_config_value_features() {
        assert_eq!(
            format_config_value("features", &Value::String("keyctl=1,nesting=1".into())),
            "keyctl=1,nesting=1"
        );
    }
}
