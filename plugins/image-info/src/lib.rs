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

struct ImageInfo;

impl Guest for ImageInfo {
    fn call(tool: String, input_json: String) -> Result<String, String> {
        let input: Value = serde_json::from_str(&input_json)
            .map_err(|e| format!("invalid input JSON: {e}"))?;

        match tool.as_str() {
            "image_info" => image_info(&input),
            _ => Err(format!("unknown tool: {tool}")),
        }
    }
}

export!(ImageInfo);

fn get_str<'a>(input: &'a Value, key: &str) -> Result<&'a str, String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("{key} is required"))
}

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

fn http_get(url: &str) -> Result<String, String> {
    let req = HttpRequest {
        method: "GET",
        url: url.to_string(),
        headers: serde_json::Map::new(),
        body: None,
    };
    let req_json =
        serde_json::to_string(&req).map_err(|e| format!("failed to serialize request: {e}"))?;
    let resp_json = host::http_request(&req_json)?;
    let resp: HttpResponse =
        serde_json::from_str(&resp_json).map_err(|e| format!("failed to parse HTTP response: {e}"))?;
    if resp.status >= 400 {
        return Err(format!("Request failed with HTTP {}: {}", resp.status, resp.body));
    }
    Ok(resp.body)
}

fn image_info(input: &Value) -> Result<String, String> {
    let inp = get_str(input, "input")?;

    // Check if it's a URL
    let bytes = if inp.starts_with("http://") || inp.starts_with("https://") {
        let resp = http_get(inp)?;
        // HTTP responses are strings in this host interface, so this won't work for binary
        // Return what we can
        return Err("URL-based image fetching not supported: the host returns text, not binary data. Use base64-encoded data instead.".to_string());
    } else {
        // Treat as base64
        let cleaned = if let Some(idx) = inp.find(";base64,") {
            inp[idx + 8..].to_string()
        } else if inp.contains(',') {
            inp.split(',').last().unwrap_or(inp).to_string()
        } else {
            inp.to_string()
        };

        match base64_decode_bytes(&cleaned) {
            Ok(b) => b,
            Err(e) => return Err(format!("invalid base64: {e}")),
        }
    };

    if bytes.len() < 8 {
        return Err("Image data too short (less than 8 bytes) — not a valid image.".to_string());
    }

    let info = parse_image(&bytes)?;
    Ok(format!(
        "Format: {}\nDimensions: {} x {}\nColor mode: {}\nFile size: {} bytes",
        info.format, info.width, info.height, info.color_mode, bytes.len()
    ))
}

fn base64_decode_bytes(s: &str) -> Result<Vec<u8>, String> {
    let s = s.trim().trim_end_matches('=');
    let mut bytes = Vec::with_capacity(s.len() * 3 / 4);
    let mut buf: u32 = 0;
    let mut bits = 0;

    for ch in s.chars() {
        let val = match ch {
            'A'..='Z' => ch as u8 - b'A',
            'a'..='z' => ch as u8 - b'a' + 26,
            '0'..='9' => ch as u8 - b'0' + 52,
            '+' => 62,
            '/' => 63,
            _ => continue,
        };
        buf = (buf << 6) | (val as u32);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            bytes.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }

    Ok(bytes)
}

struct ImageInfoResult {
    format: String,
    width: u32,
    height: u32,
    color_mode: String,
}

fn parse_image(bytes: &[u8]) -> Result<ImageInfoResult, String> {
    // PNG: 89 50 4E 47 0D 0A 1A 0A
    if bytes.len() >= 24 && &bytes[0..8] == b"\x89PNG\r\n\x1a\n" {
        return parse_png(bytes);
    }
    // JPEG: FF D8 FF
    if bytes.len() >= 3 && bytes[0] == 0xFF && bytes[1] == 0xD8 && bytes[2] == 0xFF {
        return parse_jpeg(bytes);
    }
    // GIF: 47 49 46 38 (GIF8)
    if bytes.len() >= 6 && &bytes[0..4] == b"GIF8" {
        return parse_gif(bytes);
    }
    // WebP: 52 49 46 46 ... 57 45 42 50 (RIFF....WEBP)
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return parse_webp(bytes);
    }
    // BMP: 42 4D (BM)
    if bytes.len() >= 26 && &bytes[0..2] == b"BM" {
        return parse_bmp(bytes);
    }
    // TIFF: 49 49 2A 00 or 4D 4D 00 2A
    if bytes.len() >= 8 {
        if &bytes[0..4] == b"II\x2a\x00" || &bytes[0..4] == b"MM\x00\x2a" {
            return Ok(ImageInfoResult {
                format: "TIFF".to_string(),
                width: 0,
                height: 0,
                color_mode: "unknown".to_string(),
            });
        }
    }
    // ICO: 00 00 01 00
    if bytes.len() >= 6 && &bytes[0..4] == b"\x00\x00\x01\x00" {
        let count = u16_le(bytes, 4);
        let w = if count > 0 { bytes.get(6).map(|b| *b as u32).unwrap_or(0) } else { 0 };
        let h = if count > 0 { bytes.get(7).map(|b| *b as u32).unwrap_or(0) } else { 0 };
        // ICO dimensions 0 means 256
        let w = if w == 0 { 256 } else { w };
        let h = if h == 0 { 256 } else { h };
        return Ok(ImageInfoResult {
            format: "ICO".to_string(),
            width: w,
            height: h,
            color_mode: "RGBA".to_string(),
        });
    }

    Err("Unknown image format. Supported formats: PNG, JPEG, GIF, WebP, BMP, ICO, TIFF.".to_string())
}

fn parse_png(bytes: &[u8]) -> Result<ImageInfoResult, String> {
    let width = u32_be(bytes, 16);
    let height = u32_be(bytes, 20);
    let bit_depth = bytes[24];
    let color_type = bytes[25];

    let color_mode = match color_type {
        0 => "Grayscale".to_string(),
        2 => "RGB".to_string(),
        3 => "Indexed".to_string(),
        4 => "Grayscale+Alpha".to_string(),
        6 => "RGBA".to_string(),
        _ => format!("Unknown ({color_type})"),
    };

    Ok(ImageInfoResult {
        format: "PNG".to_string(),
        width,
        height,
        color_mode: format!("{color_mode} ({bit_depth}-bit)"),
    })
}

fn parse_jpeg(bytes: &[u8]) -> Result<ImageInfoResult, String> {
    let mut pos = 2;
    let mut width = 0u32;
    let mut height = 0u32;

    while pos + 4 <= bytes.len() {
        let marker = bytes[pos];
        if marker != 0xFF {
            break;
        }
        let marker_type = bytes[pos + 1];
        if marker_type == 0xD9 || marker_type == 0xDA {
            // End of image or start of scan
            break;
        }

        // SOS marker — we'd need to scan for next marker, just stop
        if marker_type == 0xDA {
            break;
        }

        let length = u16_be(bytes, pos + 2) as usize;
        if pos + 2 + length > bytes.len() {
            break;
        }

        // SOF0 (baseline), SOF1 (extended), SOF2 (progressive)
        if marker_type >= 0xC0 && marker_type <= 0xC2 && length >= 7 {
            if width == 0 || height == 0 {
                height = u16_be(bytes, pos + 5) as u32;
                width = u16_be(bytes, pos + 7) as u32;
                let components = bytes[pos + 9];
                let color_mode = match components {
                    1 => "Grayscale".to_string(),
                    3 => "YCbCr (RGB)".to_string(),
                    4 => "CMYK".to_string(),
                    _ => format!("{components} components"),
                };
                return Ok(ImageInfoResult {
                    format: "JPEG".to_string(),
                    width,
                    height,
                    color_mode,
                });
            }
        }

        pos += 2 + length;
    }

    Err("Could not find JPEG dimensions. File may be corrupted.".to_string())
}

fn parse_gif(bytes: &[u8]) -> Result<ImageInfoResult, String> {
    let width = u16_le(bytes, 6) as u32;
    let height = u16_le(bytes, 8) as u32;
    let color_table_info = bytes[10];
    let has_global_color_table = (color_table_info & 0x80) != 0;
    let color_mode = if has_global_color_table { "Indexed (palette)" } else { "Grayscale" };

    Ok(ImageInfoResult {
        format: "GIF".to_string(),
        width,
        height,
        color_mode: color_mode.to_string(),
    })
}

fn parse_webp(bytes: &[u8]) -> Result<ImageInfoResult, String> {
    let chunk = &bytes[12..16];
    let (format_name, width, height, color_mode) = match chunk {
        b"VP8 " => {
            if bytes.len() < 30 { return Err("WebP VP8 header too short".to_string()); }
            let w = u16_le(bytes, 26) as u32 & 0x3FFF;
            let h = u16_le(bytes, 28) as u32 & 0x3FFF;
            ("WebP (VP8)", w, h, "YCbCr (lossy)")
        }
        b"VP8L" => {
            if bytes.len() < 25 { return Err("WebP VP8L header too short".to_string()); }
            let bits = u32_le(bytes, 21);
            let w = (bits & 0x3FFF) + 1;
            let h = ((bits >> 14) & 0x3FFF) + 1;
            ("WebP (VP8L)", w, h, "RGBA (lossless)")
        }
        b"VP8X" => {
            if bytes.len() < 24 { return Err("WebP VP8X header too short".to_string()); }
            let w = u32_le(bytes, 24) + 1;
            let h = u32_le(bytes, 27) + 1;
            ("WebP (VP8X)", w, h, "RGBA (extended)")
        }
        _ => return Err("Unknown WebP variant".to_string()),
    };

    Ok(ImageInfoResult {
        format: format_name.to_string(),
        width,
        height,
        color_mode: color_mode.to_string(),
    })
}

fn parse_bmp(bytes: &[u8]) -> Result<ImageInfoResult, String> {
    let width = u32_le(bytes, 18) as u32;
    let height = u32_le(bytes, 22) as u32;
    let bpp = u16_le(bytes, 28) as u32;

    let color_mode = match bpp {
        1 => "1-bit (monochrome)".to_string(),
        4 => "4-bit (16 colors)".to_string(),
        8 => "8-bit (256 colors)".to_string(),
        16 => "16-bit (high color)".to_string(),
        24 => "24-bit (RGB)".to_string(),
        32 => "32-bit (RGBA)".to_string(),
        _ => format!("{bpp}-bit"),
    };

    Ok(ImageInfoResult {
        format: "BMP".to_string(),
        width,
        height,
        color_mode,
    })
}

fn u16_be(bytes: &[u8], offset: usize) -> u16 {
    ((bytes.get(offset).copied().unwrap_or(0) as u16) << 8)
        | (bytes.get(offset + 1).copied().unwrap_or(0) as u16)
}

fn u16_le(bytes: &[u8], offset: usize) -> u16 {
    (bytes.get(offset).copied().unwrap_or(0) as u16)
        | ((bytes.get(offset + 1).copied().unwrap_or(0) as u16) << 8)
}

fn u32_be(bytes: &[u8], offset: usize) -> u32 {
    ((bytes.get(offset).copied().unwrap_or(0) as u32) << 24)
        | ((bytes.get(offset + 1).copied().unwrap_or(0) as u32) << 16)
        | ((bytes.get(offset + 2).copied().unwrap_or(0) as u32) << 8)
        | (bytes.get(offset + 3).copied().unwrap_or(0) as u32)
}

fn u32_le(bytes: &[u8], offset: usize) -> u32 {
    (bytes.get(offset).copied().unwrap_or(0) as u32)
        | ((bytes.get(offset + 1).copied().unwrap_or(0) as u32) << 8)
        | ((bytes.get(offset + 2).copied().unwrap_or(0) as u32) << 16)
        | ((bytes.get(offset + 3).copied().unwrap_or(0) as u32) << 24)
}
