use serde_json::{Map, Value};

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

struct DataFormat;

impl Guest for DataFormat {
    fn call(tool: String, input_json: String) -> Result<String, String> {
        let input: Value = serde_json::from_str(&input_json)
            .map_err(|e| format!("invalid input JSON: {e}"))?;

        match tool.as_str() {
            "csv_to_json" => csv_to_json(&input),
            "json_to_csv" => json_to_csv(&input),
            "yaml_to_json" => yaml_to_json(&input),
            "json_to_yaml" => json_to_yaml(&input),
            "xml_to_json" => xml_to_json(&input),
            "markdown_to_html" => markdown_to_html(&input),
            _ => Err(format!("unknown tool: {tool}")),
        }
    }
}

export!(DataFormat);

fn get_str<'a>(input: &'a Value, key: &str) -> Result<&'a str, String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("{key} is required"))
}

// ── CSV → JSON ─────────────────────────────────────────────────────────────────

fn csv_to_json(input: &Value) -> Result<String, String> {
    let csv = get_str(input, "csv")?;
    let mut rows = parse_csv_rows(csv);
    if rows.is_empty() {
        return Err("CSV has no data".to_string());
    }
    let headers = rows.remove(0);
    let mut arr = Vec::with_capacity(rows.len());
    for row in &rows {
        let mut obj = Map::new();
        for (i, h) in headers.iter().enumerate() {
            let val = row.get(i).map(|s| s.as_str()).unwrap_or("");
            obj.insert(h.clone(), Value::String(val.to_string()));
        }
        arr.push(Value::Object(obj));
    }
    serde_json::to_string_pretty(&Value::Array(arr)).map_err(|e| format!("serialize: {e}"))
}

fn parse_csv_rows(input: &str) -> Vec<Vec<String>> {
    let chars: Vec<char> = input.chars().collect();
    let len = chars.len();
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut i = 0;

    while i < len {
        let ch = chars[i];
        if in_quotes {
            if ch == '"' {
                if i + 1 < len && chars[i + 1] == '"' {
                    field.push('"');
                    i += 2;
                    continue;
                }
                in_quotes = false;
                i += 1;
            } else {
                field.push(ch);
                i += 1;
            }
        } else {
            match ch {
                '"' if field.is_empty() => {
                    in_quotes = true;
                    i += 1;
                }
                ',' => {
                    row.push(std::mem::take(&mut field));
                    i += 1;
                }
                '\n' => {
                    row.push(std::mem::take(&mut field));
                    rows.push(std::mem::take(&mut row));
                    i += 1;
                }
                '\r' => {
                    row.push(std::mem::take(&mut field));
                    if i + 1 < len && chars[i + 1] == '\n' {
                        i += 1;
                    }
                    rows.push(std::mem::take(&mut row));
                    i += 1;
                }
                _ => {
                    field.push(ch);
                    i += 1;
                }
            }
        }
    }
    // Trailing field/row
    field = field.trim_end_matches(&['\r', '\n'][..]).to_string();
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    } else if !row.is_empty() {
        rows.push(row);
    }
    rows
}

// ── JSON → CSV ─────────────────────────────────────────────────────────────────

fn json_to_csv(input: &Value) -> Result<String, String> {
    let json_str = get_str(input, "json")?;
    let val: Value = serde_json::from_str(json_str).map_err(|e| format!("invalid JSON: {e}"))?;
    let arr = val.as_array().ok_or("JSON must be an array of objects")?;
    if arr.is_empty() {
        return Ok(String::new());
    }

    let mut headers: Vec<String> = Vec::new();
    if let Some(first) = arr[0].as_object() {
        for k in first.keys() {
            headers.push(k.clone());
        }
    }
    if headers.is_empty() {
        return Err("first element has no keys".to_string());
    }

    let mut out = String::new();
    out.push_str(&csv_quote_row(&headers));
    out.push('\n');

    for item in arr {
        let obj = item.as_object().ok_or("array items must be objects")?;
        let mut fields = Vec::with_capacity(headers.len());
        for h in &headers {
            let val = match obj.get(h) {
                Some(Value::String(s)) => s.clone(),
                Some(Value::Number(n)) => n.to_string(),
                Some(Value::Bool(b)) => b.to_string(),
                Some(Value::Null) | None => String::new(),
                Some(v) => v.to_string(),
            };
            fields.push(val);
        }
        out.push_str(&csv_quote_row(&fields));
        out.push('\n');
    }

    Ok(out.trim_end().to_string())
}

fn csv_quote_row(fields: &[String]) -> String {
    fields
        .iter()
        .map(|f| {
            let needs_quote =
                f.contains(',') || f.contains('"') || f.contains('\n') || f.contains('\r');
            if needs_quote {
                format!("\"{}\"", f.replace('"', "\"\""))
            } else {
                f.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}

// ── YAML → JSON ────────────────────────────────────────────────────────────────

fn yaml_to_json(input: &Value) -> Result<String, String> {
    let yaml = get_str(input, "yaml")?;
    let val = parse_yaml(yaml)?;
    serde_json::to_string_pretty(&val).map_err(|e| format!("serialize: {e}"))
}

fn parse_yaml(input: &str) -> Result<Value, String> {
    let lines: Vec<&str> = input.lines().collect();
    let mut parser = YamlParser {
        lines,
        pos: 0,
    };
    parser.parse_document()
}

struct YamlParser<'a> {
    lines: Vec<&'a str>,
    pos: usize,
}

impl<'a> YamlParser<'a> {
    fn trimmed(&self) -> String {
        self.lines[self.pos].trim().to_string()
    }

    fn indent_of(&self, idx: usize) -> usize {
        self.lines[idx].len() - self.lines[idx].trim_start().len()
    }

    fn current_indent(&self) -> usize {
        self.indent_of(self.pos)
    }

    fn skip_blank(&mut self) {
        while self.pos < self.lines.len() {
            let t = self.lines[self.pos].trim();
            if t.is_empty() || t.starts_with('#') || t == "---" || t == "..." {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn parse_document(&mut self) -> Result<Value, String> {
        self.skip_blank();
        if self.pos >= self.lines.len() {
            return Ok(Value::Null);
        }

        let indent = self.current_indent();
        let trimmed = self.trimmed();

        if trimmed.starts_with("- ") {
            self.parse_sequence(indent)
        } else if is_mapping_key(&trimmed) {
            self.parse_mapping_or_scalar(indent)
        } else {
            // Top-level scalar
            let s = self.collect_scalar(indent)?;
            Ok(parse_scalar_literal(&s))
        }
    }

    fn parse_mapping_or_scalar(&mut self, indent: usize) -> Result<Value, String> {
        // Check if the first line is a mapping key
        let trimmed = self.trimmed();
        if is_mapping_key(&trimmed) {
            self.parse_mapping(indent)
        } else {
            let s = self.collect_scalar(indent)?;
            Ok(parse_scalar_literal(&s))
        }
    }

    fn parse_mapping(&mut self, indent: usize) -> Result<Value, String> {
        let mut map = Map::new();
        loop {
            self.skip_blank();
            if self.pos >= self.lines.len() {
                break;
            }
            let line_indent = self.current_indent();
            if line_indent < indent {
                break;
            }
            if line_indent != indent {
                break;
            }

            let trimmed = self.trimmed();
            if !is_mapping_key(&trimmed) {
                break;
            }

            let (key, inline_value) = split_key_value(&trimmed);
            self.pos += 1;

            if inline_value.is_empty() {
                self.skip_blank();
                if self.pos < self.lines.len() {
                    let next_indent = self.current_indent();
                    if next_indent > indent {
                        let next_trimmed = self.trimmed();
                        if next_trimmed.starts_with("- ") {
                            map.insert(key, self.parse_sequence(next_indent)?);
                        } else if is_mapping_key(&next_trimmed) {
                            map.insert(key, self.parse_mapping(next_indent)?);
                        } else {
                            map.insert(
                                key,
                                Value::String(self.collect_scalar(next_indent)?),
                            );
                        }
                    } else {
                        map.insert(key, Value::Null);
                    }
                } else {
                    map.insert(key, Value::Null);
                }
            } else {
                map.insert(key, parse_scalar_literal(&inline_value));
            }
        }
        Ok(Value::Object(map))
    }

    fn parse_sequence(&mut self, indent: usize) -> Result<Value, String> {
        let mut arr = Vec::new();
        loop {
            self.skip_blank();
            if self.pos >= self.lines.len() {
                break;
            }
            let line_indent = self.current_indent();
            if line_indent < indent {
                break;
            }
            if line_indent != indent {
                break;
            }

            let trimmed = self.trimmed();
            if !trimmed.starts_with("- ") {
                break;
            }

            let item = trimmed[2..].trim().to_string();
            self.pos += 1;

            if item.is_empty() {
                self.skip_blank();
                if self.pos < self.lines.len() {
                    let next_indent = self.current_indent();
                    if next_indent > indent {
                        let next_trimmed = self.trimmed();
                        if next_trimmed.starts_with("- ") {
                            arr.push(self.parse_sequence(next_indent)?);
                        } else if is_mapping_key(&next_trimmed) {
                            arr.push(self.parse_mapping(next_indent)?);
                        } else {
                            arr.push(Value::String(self.collect_scalar(next_indent)?));
                        }
                    } else {
                        arr.push(Value::Null);
                    }
                } else {
                    arr.push(Value::Null);
                }
            } else if is_mapping_key(&item) {
                let (k, v) = split_key_value(&item);
                let mut obj = Map::new();
                if v.is_empty() {
                    self.skip_blank();
                    if self.pos < self.lines.len() {
                        let next_indent = self.current_indent();
                        if next_indent > indent {
                            let next_trimmed = self.trimmed();
                            if next_trimmed.starts_with("- ") {
                                obj.insert(k, self.parse_sequence(next_indent)?);
                            } else if is_mapping_key(&next_trimmed) {
                                obj.insert(k, self.parse_mapping(next_indent)?);
                            } else {
                                obj.insert(
                                    k,
                                    Value::String(self.collect_scalar(next_indent)?),
                                );
                            }
                        } else {
                            obj.insert(k, Value::Null);
                        }
                    } else {
                        obj.insert(k, Value::Null);
                    }
                } else {
                    obj.insert(k, parse_scalar_literal(&v));
                }

                // Gather additional keys belonging to this array-item object
                loop {
                    self.skip_blank();
                    if self.pos >= self.lines.len() {
                        break;
                    }
                    let ni = self.current_indent();
                    if ni <= indent {
                        break;
                    }
                    let nt = self.trimmed();
                    if nt.starts_with("- ") || !is_mapping_key(&nt) {
                        break;
                    }
                    let (k2, v2) = split_key_value(&nt);
                    self.pos += 1;
                    if v2.is_empty() {
                        self.skip_blank();
                        if self.pos < self.lines.len() {
                            let nni = self.current_indent();
                            if nni > ni {
                                let nnt = self.trimmed();
                                if nnt.starts_with("- ") {
                                    obj.insert(k2, self.parse_sequence(nni)?);
                                } else if is_mapping_key(&nnt) {
                                    obj.insert(k2, self.parse_mapping(nni)?);
                                } else {
                                    obj.insert(
                                        k2,
                                        Value::String(self.collect_scalar(nni)?),
                                    );
                                }
                            } else {
                                obj.insert(k2, Value::Null);
                            }
                        } else {
                            obj.insert(k2, Value::Null);
                        }
                    } else {
                        obj.insert(k2, parse_scalar_literal(&v2));
                    }
                }

                arr.push(Value::Object(obj));
            } else {
                arr.push(parse_scalar_literal(&item));
            }
        }
        Ok(Value::Array(arr))
    }

    fn collect_scalar(&mut self, indent: usize) -> Result<String, String> {
        let mut parts = Vec::new();
        loop {
            if self.pos >= self.lines.len() {
                break;
            }
            let line_indent = self.current_indent();
            if line_indent < indent {
                break;
            }
            let trimmed = self.trimmed();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                self.pos += 1;
                continue;
            }
            if line_indent != indent {
                break;
            }
            if trimmed.starts_with("- ") || is_mapping_key(&trimmed) {
                break;
            }
            parts.push(trimmed);
            self.pos += 1;
        }
        if parts.is_empty() {
            Ok(String::new())
        } else {
            Ok(parts.join(" "))
        }
    }
}

fn is_mapping_key(s: &str) -> bool {
    if s.is_empty() || s.starts_with('#') {
        return false;
    }
    // Find first colon outside of quotes
    let chars: Vec<char> = s.chars().collect();
    let mut in_quote = false;
    let mut quote_char = '"';
    for (i, &ch) in chars.iter().enumerate() {
        if in_quote {
            if ch == quote_char {
                in_quote = false;
            }
        } else if ch == '"' || ch == '\'' {
            in_quote = true;
            quote_char = ch;
        } else if ch == ':' {
            // Colon found - check that it's followed by space or end
            if i + 1 >= chars.len() || chars[i + 1] == ' ' {
                // Also check that there's content before the colon (key not empty)
                let before: String = chars[..i].iter().collect();
                return !before.trim().is_empty();
            }
        }
    }
    false
}

fn split_key_value(s: &str) -> (String, String) {
    let chars: Vec<char> = s.chars().collect();
    let mut in_quote = false;
    let mut quote_char = '"';
    for (i, &ch) in chars.iter().enumerate() {
        if in_quote {
            if ch == quote_char {
                in_quote = false;
            }
        } else if ch == '"' || ch == '\'' {
            in_quote = true;
            quote_char = ch;
        } else if ch == ':' {
            if i + 1 >= chars.len() || chars[i + 1] == ' ' {
                let key: String = chars[..i].iter().collect();
                let key = key.trim().trim_matches('"').trim_matches('\'').to_string();
                let value = if i + 1 < chars.len() {
                    chars[i + 1..].iter().collect::<String>().trim().to_string()
                } else {
                    String::new()
                };
                return (key, value);
            }
        }
    }
    (s.to_string(), String::new())
}

fn parse_scalar_literal(s: &str) -> Value {
    let trimmed = s.trim();
    // Remove surrounding quotes
    let unquoted = if (trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() >= 2)
        || (trimmed.starts_with('\'') && trimmed.ends_with('\'') && trimmed.len() >= 2)
    {
        &trimmed[1..trimmed.len() - 1]
    } else {
        trimmed
    };

    if unquoted.is_empty() && trimmed.len() >= 2 {
        // It was empty quotes: "" or ''
        return Value::String(String::new());
    }

    // Check for null
    if unquoted.eq_ignore_ascii_case("null") || unquoted == "~" {
        return Value::Null;
    }
    // Check for booleans
    if unquoted.eq_ignore_ascii_case("true") || unquoted.eq_ignore_ascii_case("yes") || unquoted.eq_ignore_ascii_case("on") {
        return Value::Bool(true);
    }
    if unquoted.eq_ignore_ascii_case("false") || unquoted.eq_ignore_ascii_case("no") || unquoted.eq_ignore_ascii_case("off") {
        return Value::Bool(false);
    }
    // Check for numbers
    if let Ok(n) = unquoted.parse::<i64>() {
        return Value::Number(n.into());
    }
    if let Ok(n) = unquoted.parse::<f64>() {
        if n.is_finite() {
            // Use serde_json's number conversion
            if let Ok(v) = serde_json::from_str::<Value>(unquoted) {
                if v.is_number() {
                    return v;
                }
            }
        }
    }
    Value::String(unquoted.to_string())
}

// ── JSON → YAML ────────────────────────────────────────────────────────────────

fn json_to_yaml(input: &Value) -> Result<String, String> {
    let json_str = get_str(input, "json")?;
    let val: Value = serde_json::from_str(json_str).map_err(|e| format!("invalid JSON: {e}"))?;
    Ok(value_to_yaml(&val, 0))
}

fn value_to_yaml(val: &Value, indent: usize) -> String {
    let pad = " ".repeat(indent);

    match val {
        Value::Object(map) => {
            if map.is_empty() {
                return "{}".to_string();
            }
            let mut out = String::new();
            for (i, (k, v)) in map.iter().enumerate() {
                if i > 0 {
                    out.push('\n');
                }
                out.push_str(&pad);
                out.push_str(&yaml_key(k));
                out.push_str(": ");

                match v {
                    Value::Object(m) if !m.is_empty() => {
                        out.push('\n');
                        out.push_str(&value_to_yaml(v, indent + 2));
                    }
                    Value::Array(a) if !a.is_empty() => {
                        out.push('\n');
                        out.push_str(&value_to_yaml(v, indent + 2));
                    }
                    _ => {
                        out.push_str(&yaml_scalar(v));
                    }
                }
            }
            out
        }
        Value::Array(arr) => {
            if arr.is_empty() {
                return "[]".to_string();
            }
            let mut out = String::new();
            for (i, v) in arr.iter().enumerate() {
                if i > 0 {
                    out.push('\n');
                }
                out.push_str(&pad);
                match v {
                    Value::Object(map) if !map.is_empty() => {
                        // Format: "- key: value" on first line, rest indented
                        let mut first = true;
                        for (k, val) in map {
                            if first {
                                out.push_str("- ");
                                out.push_str(&yaml_key(k));
                                out.push_str(": ");
                                first = false;
                                match val {
                                    Value::Object(m) if !m.is_empty() => {
                                        out.push('\n');
                                        out.push_str(&value_to_yaml(val, indent + 4));
                                    }
                                    Value::Array(a) if !a.is_empty() => {
                                        out.push('\n');
                                        out.push_str(&value_to_yaml(val, indent + 4));
                                    }
                                    _ => {
                                        out.push_str(&yaml_scalar(val));
                                    }
                                }
                            } else {
                                out.push('\n');
                                out.push_str(&" ".repeat(indent + 2));
                                out.push_str(&yaml_key(k));
                                out.push_str(": ");
                                match val {
                                    Value::Object(m) if !m.is_empty() => {
                                        out.push('\n');
                                        out.push_str(&value_to_yaml(val, indent + 4));
                                    }
                                    Value::Array(a) if !a.is_empty() => {
                                        out.push('\n');
                                        out.push_str(&value_to_yaml(val, indent + 4));
                                    }
                                    _ => {
                                        out.push_str(&yaml_scalar(val));
                                    }
                                }
                            }
                        }
                    }
                    Value::Array(_) => {
                        out.push_str("- ");
                        out.push('\n');
                        out.push_str(&value_to_yaml(v, indent + 2));
                    }
                    _ => {
                        out.push_str("- ");
                        out.push_str(&yaml_scalar(v));
                    }
                }
            }
            out
        }
        _ => yaml_scalar(val),
    }
}

fn yaml_key(k: &str) -> String {
    if k.contains(':')
        || k.contains('#')
        || k.contains(' ')
        || k.starts_with('-')
        || k.starts_with('"')
        || k.starts_with('\'')
        || k.is_empty()
    {
        format!("\"{}\"", k.replace('"', "\\\""))
    } else {
        k.to_string()
    }
}

fn yaml_scalar(val: &Value) -> String {
    match val {
        Value::Null => "null".to_string(),
        Value::Bool(b) => {
            if *b {
                "true".to_string()
            } else {
                "false".to_string()
            }
        }
        Value::Number(n) => n.to_string(),
        Value::String(s) => {
            if s.is_empty() {
                return "\"\"".to_string();
            }
            if s == "null" || s == "true" || s == "false" || s == "yes" || s == "no" {
                return format!("\"{s}\"");
            }
            // Check if it looks like a number or needs quoting
            let needs_quote = s.contains(':')
                || s.contains('#')
                || s.starts_with(' ')
                || s.ends_with(' ')
                || s.starts_with('-')
                || s.starts_with('[')
                || s.starts_with('{')
                || s.starts_with('"')
                || s.starts_with('\'')
                || s.starts_with('&')
                || s.starts_with('*')
                || s.starts_with('!')
                || s.starts_with('|')
                || s.starts_with('>')
                || s.contains('\n')
                || s.parse::<f64>().is_ok();
            if needs_quote {
                if s.contains('"') && !s.contains('\'') {
                    format!("'{}'", s)
                } else {
                    format!("\"{}\"", s.replace('"', "\\\""))
                }
            } else {
                s.clone()
            }
        }
        Value::Object(m) if m.is_empty() => "{}".to_string(),
        Value::Array(a) if a.is_empty() => "[]".to_string(),
        Value::Object(_) => value_to_yaml(val, 0),
        Value::Array(_) => value_to_yaml(val, 0),
    }
}

// ── XML → JSON ─────────────────────────────────────────────────────────────────

fn xml_to_json(input: &Value) -> Result<String, String> {
    let xml = get_str(input, "xml")?;
    let chars: Vec<char> = xml.chars().collect();
    let mut pos = 0;

    // Skip XML declaration if present
    if pos + 2 < chars.len() && chars[pos] == '<' && chars[pos + 1] == '?' {
        while pos < chars.len() {
            if chars[pos] == '>' {
                pos += 1;
                break;
            }
            pos += 1;
        }
    }

    let (name, val, _) = parse_xml_element(&chars, &mut pos)?;
    let mut root = Map::new();
    root.insert(name, val);
    let result = Value::Object(root);
    serde_json::to_string_pretty(&result).map_err(|e| format!("serialize: {e}"))
}

fn skip_xml_whitespace(chars: &[char], pos: &mut usize) {
    while *pos < chars.len() && chars[*pos].is_whitespace() {
        *pos += 1;
    }
}

fn parse_xml_element(chars: &[char], pos: &mut usize) -> Result<(String, Value, bool), String> {
    // Expect '<'
    skip_xml_whitespace(chars, pos);
    if *pos >= chars.len() || chars[*pos] != '<' {
        return Err("expected <".to_string());
    }
    *pos += 1; // skip <

    // Parse tag name
    skip_xml_whitespace(chars, pos);
    let tag_name = read_xml_name(chars, pos);
    if tag_name.is_empty() {
        return Err("empty tag name".to_string());
    }

    // Parse attributes
    let mut attrs = Map::new();
    loop {
        skip_xml_whitespace(chars, pos);
        if *pos >= chars.len() {
            return Err("unexpected end of XML".to_string());
        }

        match chars[*pos] {
            '/' => {
                *pos += 1; // skip /
                skip_xml_whitespace(chars, pos);
                if *pos < chars.len() && chars[*pos] == '>' {
                    *pos += 1; // skip >
                }
                // Self-closing tag
                let mut obj = Map::new();
                for (k, v) in attrs {
                    obj.insert(format!("@{k}"), v);
                }
                return Ok((tag_name, Value::Object(obj), true));
            }
            '>' => {
                *pos += 1; // skip >
                break;
            }
            _ => {
                // Attribute name="value"
                let attr_name = read_xml_name(chars, pos);
                if attr_name.is_empty() {
                    break;
                }
                skip_xml_whitespace(chars, pos);
                if *pos < chars.len() && chars[*pos] == '=' {
                    *pos += 1; // skip =
                    skip_xml_whitespace(chars, pos);
                    let quote = if *pos < chars.len()
                        && (chars[*pos] == '"' || chars[*pos] == '\'')
                    {
                        let q = chars[*pos];
                        *pos += 1;
                        q
                    } else {
                        '"'
                    };
                    let mut val = String::new();
                    while *pos < chars.len() && chars[*pos] != quote {
                        val.push(chars[*pos]);
                        *pos += 1;
                    }
                    if *pos < chars.len() {
                        *pos += 1; // skip closing quote
                    }
                    attrs.insert(attr_name, Value::String(val));
                }
            }
        }
    }

    // Parse children (text and elements)
    let mut children: Vec<(String, Value)> = Vec::new();
    let mut text = String::new();

    loop {
        skip_xml_whitespace(chars, pos);
        if *pos >= chars.len() {
            break;
        }

        if chars[*pos] == '<' {
            // Check for closing tag
            if *pos + 1 < chars.len() && chars[*pos + 1] == '/' {
                *pos += 2; // skip </
                skip_xml_whitespace(chars, pos);
                let close_name = read_xml_name(chars, pos);
                skip_xml_whitespace(chars, pos);
                if *pos < chars.len() && chars[*pos] == '>' {
                    *pos += 1; // skip >
                }
                // Ignore mismatched close tags gracefully
                let _ = close_name;
                break;
            }

            // Child element
            if !text.trim().is_empty() {
                children.push(("#text".to_string(), Value::String(text.trim().to_string())));
                text.clear();
            }
            let (child_name, child_val, _) = parse_xml_element(chars, pos)?;
            children.push((child_name, child_val));
        } else {
            text.push(chars[*pos]);
            *pos += 1;
        }
    }

    // Handle remaining text
    if !text.trim().is_empty() {
        children.push(("#text".to_string(), Value::String(text.trim().to_string())));
    }

    // Build result value
    let mut obj = Map::new();
    let had_attrs = !attrs.is_empty();

    // Add attributes as @key
    for (k, v) in attrs {
        obj.insert(format!("@{k}"), v);
    }

    // Group children by name
    let mut grouped: Map<String, Value> = Map::new();
    for (child_name, child_val) in children {
        match grouped.get_mut(&child_name) {
            Some(Value::Array(arr)) => {
                arr.push(child_val);
            }
            Some(existing) => {
                let existing_val = existing.clone();
                let arr = Value::Array(vec![existing_val, child_val]);
                grouped.insert(child_name, arr);
            }
            None => {
                grouped.insert(child_name, child_val);
            }
        }
    }

    for (k, v) in grouped {
        obj.insert(k, v);
    }

    // Simplify: if it's just #text and no attrs, return just the text
    if obj.len() == 1 && obj.contains_key("#text") && !had_attrs {
        let text_val = obj.remove("#text").unwrap();
        return Ok((tag_name, text_val, false));
    }

    // If object is empty (no attrs, no children), use {}?
    // Keep as empty object

    Ok((tag_name, Value::Object(obj), false))
}

fn read_xml_name(chars: &[char], pos: &mut usize) -> String {
    let mut name = String::new();
    while *pos < chars.len() {
        let ch = chars[*pos];
        if ch.is_alphanumeric() || ch == '_' || ch == '-' || ch == '.' || ch == ':' {
            name.push(ch);
            *pos += 1;
        } else {
            break;
        }
    }
    name
}

// ── Markdown → HTML ────────────────────────────────────────────────────────────

fn markdown_to_html(input: &Value) -> Result<String, String> {
    let md = get_str(input, "markdown")?;
    let mut out = String::new();
    let lines: Vec<&str> = md.lines().collect();
    let mut i = 0;
    let mut in_ul = false;
    let mut in_ol = false;
    let mut para_buf: Vec<String> = Vec::new();

    macro_rules! flush_para {
        () => {
            if !para_buf.is_empty() {
                out.push_str("<p>");
                out.push_str(&para_buf.join("\n"));
                out.push_str("</p>\n");
                para_buf.clear();
            }
        };
    }

    macro_rules! flush_lists {
        () => {
            if in_ul {
                out.push_str("</ul>\n");
                in_ul = false;
            }
            if in_ol {
                out.push_str("</ol>\n");
                in_ol = false;
            }
        };
    }

    let mut in_fence = false;
    let mut fence_lang = String::new();
    let mut fence_buf: Vec<String> = Vec::new();

    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();

        // Fenced code blocks
        if trimmed.starts_with("```") {
            if !in_fence {
                flush_para!();
                flush_lists!();
                in_fence = true;
                fence_lang = trimmed[3..].trim().to_string();
                fence_buf.clear();
                i += 1;
                continue;
            } else {
                // Closing fence
                out.push_str("<pre><code");
                if !fence_lang.is_empty() {
                    out.push_str(&format!(
                        " class=\"language-{}\"",
                        escape_html(&fence_lang)
                    ));
                }
                out.push('>');
                out.push_str(&escape_html(&fence_buf.join("\n")));
                out.push_str("</code></pre>\n");
                in_fence = false;
                fence_lang.clear();
                fence_buf.clear();
                i += 1;
                continue;
            }
        }

        if in_fence {
            fence_buf.push(line.to_string());
            i += 1;
            continue;
        }

        // Empty line
        if trimmed.is_empty() {
            flush_para!();
            flush_lists!();
            i += 1;
            continue;
        }

        // Horizontal rule
        if is_hr(trimmed) {
            flush_para!();
            flush_lists!();
            out.push_str("<hr>\n");
            i += 1;
            continue;
        }

        // Headings
        if trimmed.starts_with('#') {
            flush_para!();
            flush_lists!();
            let level = trimmed.chars().take_while(|c| *c == '#').count();
            if level <= 6 && level < trimmed.len() && &trimmed[level..level + 1] == " " {
                let text = trimmed[level..].trim();
                let text = text.trim_end_matches('#').trim();
                out.push_str(&format!("<h{level}>{}</h{level}>\n", inline_html(text)));
                i += 1;
                continue;
            }
        }

        // Unordered list
        if is_ul_item(trimmed) {
            flush_para!();
            if !in_ul {
                if in_ol {
                    out.push_str("</ol>\n");
                    in_ol = false;
                }
                out.push_str("<ul>\n");
                in_ul = true;
            }
            let text = trimmed[1..].trim();
            out.push_str(&format!("<li>{}</li>\n", inline_html(text)));
            i += 1;
            continue;
        }

        // Ordered list
        if let Some(text) = parse_ol_item(trimmed) {
            flush_para!();
            if !in_ol {
                if in_ul {
                    out.push_str("</ul>\n");
                    in_ul = false;
                }
                out.push_str("<ol>\n");
                in_ol = true;
            }
            out.push_str(&format!("<li>{}</li>\n", inline_html(&text)));
            i += 1;
            continue;
        }

        // Not a list item → close lists
        flush_lists!();

        // Regular paragraph text
        para_buf.push(inline_html(trimmed));
        i += 1;
    }

    flush_para!();
    if in_ul {
        out.push_str("</ul>\n");
    }
    if in_ol {
        out.push_str("</ol>\n");
    }

    // Close any open fence
    if in_fence {
        out.push_str("<pre><code");
        if !fence_lang.is_empty() {
            out.push_str(&format!(
                " class=\"language-{}\"",
                escape_html(&fence_lang)
            ));
        }
        out.push('>');
        out.push_str(&escape_html(&fence_buf.join("\n")));
        out.push_str("</code></pre>\n");
    }

    Ok(out.trim_end().to_string())
}

fn is_hr(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.len() < 3 {
        return false;
    }
    let ch = trimmed.chars().next().unwrap();
    if ch != '-' && ch != '*' && ch != '_' {
        return false;
    }
    trimmed.chars().all(|c| c == ch || c == ' ')
}

fn is_ul_item(line: &str) -> bool {
    let trimmed = line.trim();
    (trimmed.starts_with("- ") || trimmed.starts_with("* ") || trimmed.starts_with("+ "))
        && trimmed.len() > 2
}

fn parse_ol_item(line: &str) -> Option<String> {
    let trimmed = line.trim();
    let dot_pos = trimmed.find('.')?;
    let prefix = &trimmed[..dot_pos];
    if prefix.chars().all(|c| c.is_ascii_digit()) && dot_pos + 1 < trimmed.len() && &trimmed[dot_pos..dot_pos + 2] == ". "
    {
        Some(trimmed[dot_pos + 2..].to_string())
    } else {
        None
    }
}

fn inline_html(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let mut out = String::with_capacity(len);
    let mut i = 0;

    while i < len {
        let ch = chars[i];

        // Images: ![alt](url)
        if ch == '!' && i + 1 < len && chars[i + 1] == '[' {
            let mut j = i + 2;
            while j < len && chars[j] != ']' {
                j += 1;
            }
            if j < len && j + 1 < len && chars[j + 1] == '(' {
                let alt: String = chars[i + 2..j].iter().collect();
                let mut k = j + 2;
                while k < len && chars[k] != ')' {
                    k += 1;
                }
                if k < len {
                    let url: String = chars[j + 2..k].iter().collect();
                    out.push_str(&format!(
                        "<img src=\"{}\" alt=\"{}\">",
                        escape_html(&url),
                        escape_html(&alt)
                    ));
                    i = k + 1;
                    continue;
                }
            }
        }

        // Links: [text](url)
        if ch == '[' {
            let mut j = i + 1;
            while j < len && chars[j] != ']' {
                j += 1;
            }
            if j < len && j + 1 < len && chars[j + 1] == '(' {
                let link_text: String = chars[i + 1..j].iter().collect();
                let mut k = j + 2;
                while k < len && chars[k] != ')' {
                    k += 1;
                }
                if k < len {
                    let url: String = chars[j + 2..k].iter().collect();
                    out.push_str(&format!(
                        "<a href=\"{}\">{}</a>",
                        escape_html(&url),
                        inline_html(&link_text)
                    ));
                    i = k + 1;
                    continue;
                }
            }
        }

        // Strikethrough: ~~text~~
        if ch == '~' && i + 1 < len && chars[i + 1] == '~' {
            let mut j = i + 2;
            while j + 1 < len {
                if chars[j] == '~' && chars[j + 1] == '~' {
                    let inner: String = chars[i + 2..j].iter().collect();
                    out.push_str(&format!("<del>{}</del>", inline_html(&inner)));
                    i = j + 2;
                    break;
                }
                j += 1;
            }
            if j + 1 < len {
                continue;
            }
        }

        // Bold: **text** or __text__
        if (ch == '*' || ch == '_') && i + 1 < len && chars[i + 1] == ch {
            let marker = ch;
            let mut j = i + 2;
            while j + 1 < len {
                if chars[j] == marker && chars[j + 1] == marker {
                    let inner: String = chars[i + 2..j].iter().collect();
                    out.push_str(&format!("<strong>{}</strong>", inline_html(&inner)));
                    i = j + 2;
                    break;
                }
                j += 1;
            }
            if j + 1 < len {
                continue;
            }
        }

        // Italic: *text* or _text_ (but not ** or __)
        if (ch == '*' || ch == '_') && i + 1 < len && chars[i + 1] != ch && chars[i + 1] != ' ' {
            let marker = ch;
            let mut j = i + 1;
            while j < len {
                if chars[j] == marker && (j + 1 >= len || chars[j + 1] != marker) {
                    let inner: String = chars[i + 1..j].iter().collect();
                    if !inner.is_empty() {
                        out.push_str(&format!("<em>{}</em>", inline_html(&inner)));
                        i = j + 1;
                        break;
                    }
                }
                j += 1;
            }
            if i < len && chars.get(i) == Some(&marker) && j < len {
                continue;
            }
        }

        // Inline code: `code`
        if ch == '`' {
            let mut j = i + 1;
            while j < len {
                if chars[j] == '`' {
                    let code: String = chars[i + 1..j].iter().collect();
                    out.push_str(&format!("<code>{}</code>", escape_html(&code)));
                    i = j + 1;
                    break;
                }
                j += 1;
            }
            if j < len {
                continue;
            }
        }

        // Line breaks
        if ch == ' ' && i + 1 < len && chars[i + 1] == ' '
            && i + 2 < len && chars[i + 2] == ' '
        {
            // Trailing two spaces = line break (only at end of line content)
            if i + 3 >= len || chars[i + 3] == '\n' || (i + 3 < len && chars[i + 3] == ' ') {
                // Skip to end
                while i < len && chars[i] == ' ' {
                    i += 1;
                }
                out.push_str("<br>\n");
                continue;
            }
        }

        if ch == '\\' && i + 1 < len {
            // Backslash escape
            out.push(chars[i + 1]);
            i += 2;
            continue;
        }

        out.push(ch);
        i += 1;
    }

    out
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
