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

struct TextUtils;

impl Guest for TextUtils {
    fn call(tool: String, input_json: String) -> Result<String, String> {
        let input: Value = serde_json::from_str(&input_json)
            .map_err(|e| format!("invalid input JSON: {e}"))?;

        match tool.as_str() {
            "base64_encode" => base64_encode(&input),
            "base64_decode" => base64_decode(&input),
            "hash" => hash(&input),
            "uuid_v4" => uuid_v4(),
            "json_format" => json_format(&input),
            "json_validate" => json_validate(&input),
            "count_text" => count_text(&input),
            "case_convert" => case_convert(&input),
            "slugify" => slugify(&input),
            "extract_urls" => extract_urls(&input),
            "extract_emails" => extract_emails(&input),
            _ => Err(format!("unknown tool: {tool}")),
        }
    }
}

export!(TextUtils);

fn get_str<'a>(input: &'a Value, key: &str) -> Result<&'a str, String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("{key} is required"))
}

fn _get_str_opt<'a>(input: &'a Value, key: &str) -> Option<&'a str> {
    let _ = key;
    input.get(key).and_then(Value::as_str)
}

// ── Base64 ────────────────────────────────────────────────────────────────────

const BASE64_CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_encode(input: &Value) -> Result<String, String> {
    let text = get_str(input, "text")?;
    let bytes = text.as_bytes();
    let mut result = String::with_capacity((bytes.len() + 2) / 3 * 4);

    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;

        result.push(BASE64_CHARS[((triple >> 18) & 0x3F) as usize] as char);
        result.push(BASE64_CHARS[((triple >> 12) & 0x3F) as usize] as char);

        if chunk.len() > 1 {
            result.push(BASE64_CHARS[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }

        if chunk.len() > 2 {
            result.push(BASE64_CHARS[(triple & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
    }

    Ok(result)
}

fn base64_decode(input: &Value) -> Result<String, String> {
    let text = get_str(input, "text")?;
    let text = text.trim_end_matches('=');
    let mut bytes = Vec::with_capacity(text.len() * 3 / 4);

    let mut buf: u32 = 0;
    let mut bits = 0;

    for ch in text.chars() {
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

    String::from_utf8(bytes).map_err(|e| format!("invalid UTF-8: {e}"))
}

// ── SHA256 ────────────────────────────────────────────────────────────────────

fn sha256(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
        0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
        0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
        0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
        0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
        0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
        0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];

    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
        0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
    ];

    let msg = pad_sha256(data);
    for chunk in msg.chunks(64) {
        let mut w = [0u32; 64];
        for (i, word) in chunk.chunks(4).enumerate() {
            w[i] = ((word[0] as u32) << 24) | ((word[1] as u32) << 16) | ((word[2] as u32) << 8) | (word[3] as u32);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }

        let mut a = h[0];
        let mut b = h[1];
        let mut c = h[2];
        let mut d = h[3];
        let mut e = h[4];
        let mut f = h[5];
        let mut g = h[6];
        let mut hh = h[7];

        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }

    let mut result = [0u8; 32];
    for (i, val) in h.iter().enumerate() {
        result[i * 4] = (val >> 24) as u8;
        result[i * 4 + 1] = (val >> 16) as u8;
        result[i * 4 + 2] = (val >> 8) as u8;
        result[i * 4 + 3] = *val as u8;
    }
    result
}

fn pad_sha256(data: &[u8]) -> Vec<u8> {
    let bit_len = (data.len() as u64) * 8;
    let mut msg = data.to_vec();
    msg.push(0x80);
    while (msg.len() % 64) != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());
    msg
}

// ── MD5 ───────────────────────────────────────────────────────────────────────

fn md5(data: &[u8]) -> [u8; 16] {
    let s: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22,
        5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20,
        4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23,
        6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];

    let k: [u32; 64] = [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613, 0xfd469501,
        0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193, 0xa679438e, 0x49b40821,
        0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d, 0x02441453, 0xd8a1e681, 0xe7d3fbc8,
        0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed, 0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a,
        0xfffa3942, 0x8771f681, 0x6d9d6122, 0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70,
        0x289b7ec6, 0xeaa127fa, 0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665,
        0xf4292244, 0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
        0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb, 0xeb86d391,
    ];

    let mut a: u32 = 0x67452301;
    let mut b: u32 = 0xefcdab89;
    let mut c: u32 = 0x98badcfe;
    let mut d: u32 = 0x10325476;

    let msg = pad_md5(data);
    for chunk in msg.chunks(64) {
        let mut m = [0u32; 16];
        for (i, word) in chunk.chunks(4).enumerate() {
            m[i] = (word[0] as u32) | ((word[1] as u32) << 8) | ((word[2] as u32) << 16) | ((word[3] as u32) << 24);
        }

        let mut aa = a;
        let mut bb = b;
        let mut cc = c;
        let mut dd = d;

        for i in 0..64 {
            let (f, g) = if i < 16 {
                ((bb & cc) | ((!bb) & dd), i)
            } else if i < 32 {
                ((bb & dd) | (cc & (!dd)), (5 * i + 1) % 16)
            } else if i < 48 {
                (bb ^ cc ^ dd, (3 * i + 5) % 16)
            } else {
                (cc ^ (bb | (!dd)), (7 * i) % 16)
            };

            let temp = dd;
            dd = cc;
            cc = bb;
            bb = bb.wrapping_add(aa.wrapping_add(f).wrapping_add(k[i]).wrapping_add(m[g]).rotate_left(s[i]));
            aa = temp;
        }

        a = a.wrapping_add(aa);
        b = b.wrapping_add(bb);
        c = c.wrapping_add(cc);
        d = d.wrapping_add(dd);
    }

    let mut result = [0u8; 16];
    for (i, val) in [a, b, c, d].iter().enumerate() {
        result[i * 4] = *val as u8;
        result[i * 4 + 1] = (val >> 8) as u8;
        result[i * 4 + 2] = (val >> 16) as u8;
        result[i * 4 + 3] = (val >> 24) as u8;
    }
    result
}

fn pad_md5(data: &[u8]) -> Vec<u8> {
    let bit_len = (data.len() as u64) * 8;
    let mut msg = data.to_vec();
    msg.push(0x80);
    while (msg.len() % 64) != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_le_bytes());
    msg
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn hash(input: &Value) -> Result<String, String> {
    let text = get_str(input, "text")?;
    let algo = get_str(input, "algorithm")?;
    let data = text.as_bytes();
    match algo {
        "sha256" => Ok(bytes_to_hex(&sha256(data))),
        "md5" => Ok(bytes_to_hex(&md5(data))),
        _ => Err(format!("unknown algorithm: {algo}")),
    }
}

// ── UUID v4 ───────────────────────────────────────────────────────────────────

fn uuid_v4() -> Result<String, String> {
    let bytes = prng_bytes(16);
    let version = (bytes[6] & 0x0f) | 0x40;
    let variant = (bytes[8] & 0x3f) | 0x80;
    let result = format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3],
        bytes[4], bytes[5],
        version, bytes[7],
        variant, bytes[9],
        bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15],
    );
    Ok(result)
}

static mut PRNG_STATE: u64 = 0xDEADBEEF_CAFEBABE;

fn prng_bytes(n: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let byte = prng_next() as u8;
        out.push(byte);
    }
    out
}

fn prng_next() -> u64 {
    unsafe {
        PRNG_STATE = PRNG_STATE.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        PRNG_STATE ^ (PRNG_STATE >> 33)
    }
}

// ── JSON ──────────────────────────────────────────────────────────────────────

fn json_format(input: &Value) -> Result<String, String> {
    let json_str = get_str(input, "json")?;
    let val: Value =
        serde_json::from_str(json_str).map_err(|e| format!("invalid JSON: {e}"))?;
    serde_json::to_string_pretty(&val).map_err(|e| format!("failed to format: {e}"))
}

fn json_validate(input: &Value) -> Result<String, String> {
    let json_str = get_str(input, "json")?;
    let val: Value =
        serde_json::from_str(json_str).map_err(|e| format!("invalid JSON: {e}"))?;
    let summary = describe_json(&val, 0);
    Ok(summary)
}

fn describe_json(val: &Value, depth: usize) -> String {
    let indent = "  ".repeat(depth);
    match val {
        Value::Object(map) => {
            let mut parts = Vec::new();
            for (k, v) in map {
                parts.push(format!("{}  {k}: {}", indent, describe_json(v, depth + 1)));
            }
            format!("object ({len} keys):\n{}", parts.join("\n"), len = map.len())
        }
        Value::Array(arr) => {
            let sample = if arr.is_empty() {
                "(empty)".to_string()
            } else {
                describe_json(&arr[0], depth + 1)
            };
            format!("array ({len} items) — first item: {sample}", len = arr.len())
        }
        Value::String(s) => {
            if s.len() <= 80 {
                format!("string: \"{s}\"")
            } else {
                format!("string (len={}): \"{}…\"", s.len(), &s[..77])
            }
        }
        Value::Number(n) => format!("number: {n}"),
        Value::Bool(b) => format!("bool: {b}"),
        Value::Null => "null".to_string(),
    }
}

// ── Text counting ─────────────────────────────────────────────────────────────

fn count_text(input: &Value) -> Result<String, String> {
    let text = get_str(input, "text")?;
    let chars = text.chars().count();
    let chars_no_spaces = text.chars().filter(|c| !c.is_whitespace()).count();
    let words = text.split_whitespace().count();
    let lines = text.lines().count();
    Ok(format!(
        "Words: {words}\nCharacters: {chars}\nCharacters (no spaces): {chars_no_spaces}\nLines: {lines}"
    ))
}

// ── Case conversion ───────────────────────────────────────────────────────────

fn case_convert(input: &Value) -> Result<String, String> {
    let text = get_str(input, "text")?;
    let style = get_str(input, "style")?;

    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return Ok(String::new());
    }

    let result = match style {
        "upper" => text.to_uppercase(),
        "lower" => text.to_lowercase(),
        "title" => words.iter().map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                Some(c) => c.to_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        }).collect::<Vec<_>>().join(" "),
        "snake" => words.iter().map(|w| w.to_lowercase()).collect::<Vec<_>>().join("_"),
        "kebab" => words.iter().map(|w| w.to_lowercase()).collect::<Vec<_>>().join("-"),
        "camel" => {
            let mut iter = words.iter();
            let first = iter.next().map(|w| w.to_lowercase()).unwrap_or_default();
            let rest: String = iter.map(|w| {
                let mut chars = w.chars();
                match chars.next() {
                    Some(c) => c.to_uppercase().to_string() + &chars.as_str().to_lowercase(),
                    None => String::new(),
                }
            }).collect();
            first + &rest
        }
        "pascal" => words.iter().map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                Some(c) => c.to_uppercase().to_string() + &chars.as_str().to_lowercase(),
                None => String::new(),
            }
        }).collect::<Vec<_>>().join(""),
        _ => return Err(format!("unknown style: {style}. Use: upper, lower, title, snake, kebab, camel, pascal")),
    };

    Ok(result)
}

// ── Slugify ───────────────────────────────────────────────────────────────────

fn slugify(input: &Value) -> Result<String, String> {
    let text = get_str(input, "text")?;
    let mut slug = String::new();
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
        } else if ch == ' ' || ch == '-' || ch == '_' {
            if !slug.ends_with('-') {
                slug.push('-');
            }
        }
    }
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        Ok(String::new())
    } else {
        Ok(slug.to_string())
    }
}

// ── Extract URLs ──────────────────────────────────────────────────────────────

fn extract_urls(input: &Value) -> Result<String, String> {
    let text = get_str(input, "text")?;
    let mut urls = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Look for http:// or https://
        if i + 7 < bytes.len() && &bytes[i..i+7] == b"http://" {
            let start = i;
            i += 7;
            urls.push(extract_url_span(text, start, &mut i));
        } else if i + 8 < bytes.len() && &bytes[i..i+8] == b"https://" {
            let start = i;
            i += 8;
            urls.push(extract_url_span(text, start, &mut i));
        } else {
            i += 1;
        }
    }

    if urls.is_empty() {
        Ok("No URLs found.".to_string())
    } else {
        Ok(urls.join("\n"))
    }
}

fn extract_url_span(text: &str, _start: usize, pos: &mut usize) -> String {
    let bytes = text.as_bytes();
    let start = *pos - if bytes.get(*pos - 8..*pos).map_or(false, |b| b == b"https://") { 8 } else { 7 };
    while *pos < bytes.len() {
        let b = bytes[*pos];
        if b.is_ascii_whitespace() || b == b'"' || b == b'\'' || b == b'>' || b == b')' || b == b']' {
            break;
        }
        *pos += 1;
    }
    text[start..*pos].to_string()
}

// ── Extract emails ────────────────────────────────────────────────────────────

fn extract_emails(input: &Value) -> Result<String, String> {
    let text = get_str(input, "text")?;
    let mut emails = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        // Find @ symbol preceded by allowed chars
        if bytes[i] == b'@' && i > 0 && i + 1 < bytes.len() {
            // Scan backward for local part start
            let at_pos = i;
            let mut local_start = i - 1;
            loop {
                let b = bytes[local_start];
                if b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-' || b == b'+' {
                    if local_start == 0 { break; }
                    local_start -= 1;
                } else {
                    local_start += 1;
                    break;
                }
            }

            // Scan forward for domain
            let mut domain_end = i + 1;
            while domain_end < bytes.len() {
                let b = bytes[domain_end];
                if b.is_ascii_alphanumeric() || b == b'.' || b == b'-' {
                    domain_end += 1;
                } else {
                    break;
                }
            }

            if local_start < at_pos && domain_end > at_pos + 1 {
                let email = &text[local_start..domain_end];
                // Basic validation: must have a dot in domain after @
                let after_at = &text[at_pos + 1..domain_end];
                if after_at.contains('.') && !after_at.starts_with('.') && !after_at.ends_with('.') {
                    if !emails.contains(&email) {
                        emails.push(email);
                    }
                }
            }
            i = domain_end;
        } else {
            i += 1;
        }
    }

    if emails.is_empty() {
        Ok("No email addresses found.".to_string())
    } else {
        Ok(emails.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("\n"))
    }
}
