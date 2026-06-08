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

struct ColorTools;

impl Guest for ColorTools {
    fn call(tool: String, input_json: String) -> Result<String, String> {
        let input: Value = serde_json::from_str(&input_json)
            .map_err(|e| format!("invalid input JSON: {e}"))?;
        match tool.as_str() {
            "color_convert" => color_convert(&input),
            "color_scheme" => color_scheme(&input),
            "color_name" => color_name(&input),
            _ => Err(format!("unknown tool: {tool}")),
        }
    }
}

export!(ColorTools);

fn get_str<'a>(input: &'a Value, key: &str) -> Result<&'a str, String> {
    input.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
        .ok_or_else(|| format!("{key} is required"))
}

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
#[derive(Clone, Copy)]
struct Rgb { r: f64, g: f64, b: f64 }
#[derive(Clone, Copy)]
struct Hsl { h: f64, s: f64, l: f64 }
#[derive(Clone, Copy)]
struct Cmyk { c: f64, m: f64, y: f64, k: f64 }
fn rgb_from_hex(hex: &str) -> Result<Rgb, String> {
    let h = hex.trim_start_matches('#');
    if h.len() != 6 { return Err("hex must be 6 characters (e.g. ff5733)".to_string()); }
    let r = u8::from_str_radix(&h[0..2], 16).map_err(|e| format!("invalid hex: {e}"))? as f64;
    let g = u8::from_str_radix(&h[2..4], 16).map_err(|e| format!("invalid hex: {e}"))? as f64;
    let b = u8::from_str_radix(&h[4..6], 16).map_err(|e| format!("invalid hex: {e}"))? as f64;
    Ok(Rgb { r, g, b })
}

fn rgb_to_hex(rgb: &Rgb) -> String {
    format!("#{:02X}{:02X}{:02X}", rgb.r as u8, rgb.g as u8, rgb.b as u8)
}

fn rgb_to_hsl(rgb: &Rgb) -> Hsl {
    let r = rgb.r / 255.0;
    let g = rgb.g / 255.0;
    let b = rgb.b / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    if (max - min).abs() < 0.0001 { return Hsl { h: 0.0, s: 0.0, l }; }
    let d = max - min;
    let s = if l > 0.5 { d / (2.0 - max - min) } else { d / (max + min) };
    let h = if (max - r).abs() < 0.0001 {
        (g - b) / d + (if g < b { 6.0 } else { 0.0 })
    } else if (max - g).abs() < 0.0001 {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    } * 60.0;
    Hsl { h, s, l }
}

fn hsl_to_rgb(hsl: &Hsl) -> Rgb {
    let h = hsl.h / 360.0;
    let s = hsl.s;
    let l = hsl.l;
    if s < 0.0001 {
        let v = (l * 255.0) as u8 as f64;
        return Rgb { r: v, g: v, b: v };
    }
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let r = hue_to_rgb(p, q, h + 1.0 / 3.0) * 255.0;
    let g = hue_to_rgb(p, q, h) * 255.0;
    let b = hue_to_rgb(p, q, h - 1.0 / 3.0) * 255.0;
    Rgb { r: r.round(), g: g.round(), b: b.round() }
}

fn hue_to_rgb(p: f64, q: f64, t: f64) -> f64 {
    let t = if t < 0.0 { t + 1.0 } else if t > 1.0 { t - 1.0 } else { t };
    if t < 1.0 / 6.0 { p + (q - p) * 6.0 * t }
    else if t < 1.0 / 2.0 { q }
    else if t < 2.0 / 3.0 { p + (q - p) * (2.0 / 3.0 - t) * 6.0 }
    else { p }
}

fn rgb_to_cmyk(rgb: &Rgb) -> Cmyk {
    let r = rgb.r / 255.0;
    let g = rgb.g / 255.0;
    let b = rgb.b / 255.0;
    let k = 1.0 - r.max(g).max(b);
    if k >= 0.999 { return Cmyk { c: 0.0, m: 0.0, y: 0.0, k: 1.0 }; }
    let c = (1.0 - r - k) / (1.0 - k);
    let m = (1.0 - g - k) / (1.0 - k);
    let y = (1.0 - b - k) / (1.0 - k);
    Cmyk { c, m, y, k }
}

fn parse_color(input: &str) -> Result<(Rgb, String), String> {
    let s = input.trim();
    // Hex
    if s.starts_with('#') || s.len() == 6 && s.chars().all(|c| c.is_ascii_hexdigit()) {
        let rgb = rgb_from_hex(s)?;
        return Ok((rgb, rgb_to_hex(&rgb)));
    }
    // rgb(r, g, b)
    if s.to_lowercase().starts_with("rgb") {
        let inner = extract_parens(s);
        let parts: Vec<f64> = inner.split(',').filter_map(|p| p.trim().parse().ok()).collect();
        if parts.len() == 3 {
            let rgb = Rgb { r: parts[0], g: parts[1], b: parts[2] };
            return Ok((rgb, rgb_to_hex(&rgb)));
        }
    }
    // hsl(h, s%, l%)
    if s.to_lowercase().starts_with("hsl") {
        let inner = extract_parens(s);
        let parts: Vec<f64> = inner.split(',').map(|p| p.trim().trim_end_matches('%').parse().unwrap_or(0.0)).collect();
        if parts.len() == 3 {
            let hsl = Hsl { h: parts[0], s: parts[1] / 100.0, l: parts[2] / 100.0 };
            let rgb = hsl_to_rgb(&hsl);
            return Ok((rgb, rgb_to_hex(&rgb)));
        }
    }
    // Comma-separated numbers (assume RGB)
    let parts: Vec<f64> = s.split(',').filter_map(|p| p.trim().parse().ok()).collect();
    if parts.len() == 3 {
        let rgb = Rgb { r: parts[0], g: parts[1], b: parts[2] };
        return Ok((rgb, rgb_to_hex(&rgb)));
    }
    // Try as a CSS color name via API
    Err(format!("Could not parse color: {s}. Use hex (#ff5733), rgb(255,87,51), or hsl(15,100%,60%)."))
}

fn extract_parens(s: &str) -> String {
    s.chars().skip_while(|c| *c != '(').skip(1).take_while(|c| *c != ')').collect()
}

fn color_convert(input: &Value) -> Result<String, String> {
    let color_str = get_str(input, "color")?;
    let (rgb, hex) = parse_color(color_str)?;
    let hsl = rgb_to_hsl(&rgb);
    let cmyk = rgb_to_cmyk(&rgb);
    let name = lookup_color_name(&hex).unwrap_or_else(|_| "unknown".to_string());

    Ok(format!(
        "Hex: {hex}\n\
         RGB: rgb({}, {}, {})\n\
         HSL: hsl({:.0}°, {:.0}%, {:.0}%)\n\
         CMYK: cmyk({:.0}%, {:.0}%, {:.0}%, {:.0}%)\n\
         Closest named color: {name}",
        rgb.r as u8, rgb.g as u8, rgb.b as u8,
        hsl.h, hsl.s * 100.0, hsl.l * 100.0,
        cmyk.c * 100.0, cmyk.m * 100.0, cmyk.y * 100.0, cmyk.k * 100.0,
    ))
}

fn color_scheme(input: &Value) -> Result<String, String> {
    let color_str = get_str(input, "color")?;
    let scheme = get_str_opt(input, "scheme").unwrap_or("all");
    let (rgb, hex) = parse_color(color_str)?;
    let hsl = rgb_to_hsl(&rgb);

    let schemes = vec![
        ("Complementary", vec![(hsl.h + 180.0) % 360.0]),
        ("Triadic", vec![(hsl.h + 120.0) % 360.0, (hsl.h + 240.0) % 360.0]),
        ("Analogous", vec![(hsl.h + 30.0) % 360.0, (hsl.h - 30.0 + 360.0) % 360.0]),
        ("Split Complementary", vec![(hsl.h + 150.0) % 360.0, (hsl.h + 210.0) % 360.0]),
        ("Square", vec![(hsl.h + 90.0) % 360.0, (hsl.h + 180.0) % 360.0, (hsl.h + 270.0) % 360.0]),
        ("Monochromatic", vec![]),
    ];

    let mut out = format!("Base: {hex}\n\n");
    for (label, hues) in &schemes {
        if scheme != "all" && !label.eq_ignore_ascii_case(scheme) { continue; }
        if *label == "Monochromatic" {
            let light = Hsl { h: hsl.h, s: hsl.s, l: (hsl.l + 0.2).min(1.0) };
            let dark = Hsl { h: hsl.h, s: hsl.s, l: (hsl.l - 0.2).max(0.0) };
            out.push_str(&format!("{label}:\n  {hex} (base)\n  {} (lighter)\n  {} (darker)\n\n",
                rgb_to_hex(&hsl_to_rgb(&light)), rgb_to_hex(&hsl_to_rgb(&dark))));
        } else {
            out.push_str(&format!("{label}:\n"));
            for h in hues {
                let c = Hsl { h: *h, s: hsl.s, l: hsl.l };
                out.push_str(&format!("  {}\n", rgb_to_hex(&hsl_to_rgb(&c))));
            }
            out.push('\n');
        }
    }
    Ok(out)
}

fn color_name(input: &Value) -> Result<String, String> {
    let color_str = get_str(input, "color")?;
    let (_rgb, hex) = parse_color(color_str)?;
    let name = lookup_color_name(&hex)?;
    Ok(format!("{hex} is closest to: {name}"))
}

fn lookup_color_name(hex: &str) -> Result<String, String> {
    let clean = hex.trim_start_matches('#');
    let url = format!("https://www.thecolorapi.com/id?hex={clean}");
    let body = http_get(&url).map_err(|_| "colorapi.com unavailable".to_string())?;
    let data: Value = serde_json::from_str(&body).map_err(|_| "colorapi.com unavailable".to_string())?;
    let name = data["name"]["value"].as_str().unwrap_or("Unknown");
    Ok(name.to_string())
}
