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

struct MarkdownUtils;

impl Guest for MarkdownUtils {
    fn call(tool: String, input_json: String) -> Result<String, String> {
        let input: Value = serde_json::from_str(&input_json)
            .map_err(|e| format!("invalid input JSON: {e}"))?;

        match tool.as_str() {
            "md_to_text" => md_to_text(&input),
            "md_extract_links" => md_extract_links(&input),
            "md_extract_headings" => md_extract_headings(&input),
            "md_extract_code_blocks" => md_extract_code_blocks(&input),
            _ => Err(format!("unknown tool: {tool}")),
        }
    }
}

export!(MarkdownUtils);

fn get_str<'a>(input: &'a Value, key: &str) -> Result<&'a str, String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("{key} is required"))
}

fn md_to_text(input: &Value) -> Result<String, String> {
    let md = get_str(input, "markdown")?;
    let text = strip_markdown(md);
    Ok(text)
}

fn strip_markdown(md: &str) -> String {
    let mut result = String::with_capacity(md.len());
    let chars: Vec<char> = md.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        let ch = chars[i];

        // Fenced code blocks
        if ch == '`' && i + 2 < len && chars[i+1] == '`' && chars[i+2] == '`' {
            // Skip opening fence
            while i < len && chars[i] != '\n' { i += 1; }
            if i < len { i += 1; } // skip newline
            // Skip until closing fence
            while i < len {
                if chars[i] == '`' && i + 2 < len && chars[i+1] == '`' && chars[i+2] == '`' {
                    while i < len && chars[i] != '\n' { i += 1; }
                    if i < len { i += 1; }
                    break;
                }
                i += 1;
            }
            continue;
        }

        // Inline code
        if ch == '`' && i + 1 < len {
            let mut j = i + 1;
            let mut found = false;
            while j < len {
                if chars[j] == '`' {
                    let content: String = chars[i+1..j].iter().collect();
                    result.push_str(&content);
                    i = j + 1;
                    found = true;
                    break;
                }
                j += 1;
            }
            if found { continue; }
        }

        // Images: ![alt](url)
        if ch == '!' && i + 1 < len && chars[i+1] == '[' {
            let mut j = i + 2;
            while j < len && chars[j] != ']' { j += 1; }
            let alt: String = chars[i+2..j].iter().collect();
            if !alt.is_empty() {
                result.push_str(&format!("[Image: {alt}]"));
            }
            // Skip past ](url)
            if j + 1 < len && chars.get(j+1) == Some(&'(') {
                let mut k = j + 2;
                while k < len && chars[k] != ')' { k += 1; }
                i = k + 1;
                continue;
            }
            i = j + 1;
            continue;
        }

        // Links: [text](url)
        if ch == '[' {
            let mut j = i + 1;
            let mut found_close = false;
            while j < len && chars[j] != ']' { j += 1; }
            if j < len { found_close = true; }
            if found_close && j + 1 < len && chars[j+1] == '(' {
                let text: String = chars[i+1..j].iter().collect();
                let mut k = j + 2;
                while k < len && chars[k] != ')' { k += 1; }
                if k < len {
                    result.push_str(&text);
                    i = k + 1;
                    continue;
                }
            }
        }

        // Bold/italic markers
        if (ch == '*' || ch == '_') && i + 1 < len {
            let marker = ch;
            let next = chars[i+1];
            if next == marker {
                // Bold ** or __
                let mut j = i + 2;
                while j + 1 < len {
                    if chars[j] == marker && chars[j+1] == marker {
                        let content: String = chars[i+2..j].iter().collect();
                        result.push_str(&content);
                        i = j + 2;
                        break;
                    }
                    j += 1;
                }
                if j + 1 >= len { result.push(ch); result.push(ch); i += 2; }
                continue;
            } else if next != ' ' && next != '\n' {
                // Italic * or _
                let mut j = i + 1;
                while j < len {
                    if chars[j] == marker && (j == len - 1 || chars[j+1] != marker) {
                        let content: String = chars[i+1..j].iter().collect();
                        result.push_str(&content);
                        i = j + 1;
                        break;
                    }
                    j += 1;
                }
                if j >= len { result.push(ch); i += 1; }
                continue;
            }
        }

        // Strikethrough ~~
        if ch == '~' && i + 1 < len && chars[i+1] == '~' {
            let mut j = i + 2;
            while j + 1 < len {
                if chars[j] == '~' && chars[j+1] == '~' {
                    let content: String = chars[i+2..j].iter().collect();
                    result.push_str(&content);
                    i = j + 2;
                    break;
                }
                j += 1;
            }
            if j + 1 >= len { result.push_str("~~"); i += 2; }
            continue;
        }

        // Headers
        if ch == '#' && (i == 0 || chars[i-1] == '\n') {
            let mut j = i;
            while j < len && chars[j] == '#' { j += 1; }
            if j < len && chars[j] == ' ' {
                // Skip to end of line (don't include header text, just skip it)
                while j < len && chars[j] != '\n' { j += 1; }
                i = j;
                if i < len && chars[i] == '\n' {
                    result.push('\n');
                    i += 1;
                }
                continue;
            }
        }

        // Horizontal rules
        if (ch == '-' || ch == '*' || ch == '_') && i + 2 < len && (i == 0 || chars[i-1] == '\n') {
            let hr_char = ch;
            let mut count = 1;
            let mut j = i + 1;
            while j < len && (chars[j] == hr_char || chars[j] == ' ') { j += if chars[j] == hr_char { count += 1; 1 } else { 1 }; }
            if count >= 3 && (j >= len || chars[j] == '\n') {
                result.push_str("\n---\n");
                i = j;
                if i < len && chars[i] == '\n' { i += 1; }
                continue;
            }
        }

        // Blockquotes
        if (i == 0 || chars[i-1] == '\n') && ch == '>' {
            let mut j = i + 1;
            if j < len && chars[j] == ' ' { j += 1; }
            while j < len && chars[j] != '\n' { j += 1; }
            // Continue to handle consecutive blockquote lines
            i = j;
            if i < len && chars[i] == '\n' { i += 1; }
            continue;
        }

        // List markers
        if (i == 0 || chars[i-1] == '\n') && (ch == '-' || ch == '*' || ch == '+') && i + 1 < len && chars[i+1] == ' ' {
            result.push_str("  • ");
            i += 2;
            continue;
        }
        if (i == 0 || chars[i-1] == '\n') && ch.is_ascii_digit() {
            let mut j = i;
            while j < len && chars[j].is_ascii_digit() { j += 1; }
            if j < len && chars[j] == '.' && j + 1 < len && chars[j+1] == ' ' {
                result.push_str("  • ");
                i = j + 2;
                continue;
            }
        }

        // HTML tags
        if ch == '<' {
            let mut j = i + 1;
            while j < len && chars[j] != '>' { j += 1; }
            if j < len {
                i = j + 1;
                continue;
            }
        }

        // HTML entities
        if ch == '&' {
            let mut j = i + 1;
            while j < len && chars[j] != ';' && j - i < 10 { j += 1; }
            if j < len && chars[j] == ';' {
                let entity: String = chars[i+1..j].iter().collect();
                match entity.as_str() {
                    "amp" | "AMP" => result.push('&'),
                    "lt" | "LT" => result.push('<'),
                    "gt" | "GT" => result.push('>'),
                    "quot" | "QUOT" => result.push('"'),
                    "apos" | "APOS" => result.push('\''),
                    "nbsp" => result.push(' '),
                    _ => {}
                }
                i = j + 1;
                continue;
            }
        }

        // Table rows - skip pipe-based tables
        if (i == 0 || chars[i-1] == '\n') && ch == '|' {
            while i < len && chars[i] != '\n' { i += 1; }
            if i < len && chars[i] == '\n' { i += 1; }
            continue;
        }

        // Normal character
        result.push(ch);
        i += 1;
    }

    // Clean up multiple newlines
    let mut cleaned = String::with_capacity(result.len());
    let mut newline_count = 0;
    for ch in result.chars() {
        if ch == '\n' {
            newline_count += 1;
            if newline_count <= 2 {
                cleaned.push(ch);
            }
        } else if !ch.is_whitespace() || ch != ' ' || !cleaned.ends_with(' ') {
            newline_count = 0;
            cleaned.push(ch);
        } else {
            newline_count = 0;
        }
    }

    cleaned.trim().to_string()
}

fn md_extract_links(input: &Value) -> Result<String, String> {
    let md = get_str(input, "markdown")?;
    let mut links = Vec::new();
    let chars: Vec<char> = md.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        // Inline links: [text](url)
        if chars[i] == '[' {
            let mut j = i + 1;
            while j < len && chars[j] != ']' { j += 1; }
            if j < len && j + 1 < len && chars[j+1] == '(' {
                let text: String = chars[i+1..j].iter().collect();
                let mut k = j + 2;
                while k < len && chars[k] != ')' { k += 1; }
                if k < len {
                    let url: String = chars[j+2..k].iter().collect();
                    links.push(format!("{text} → {url}"));
                    i = k + 1;
                    continue;
                }
            }
        }

        // Bare URLs
        if i + 7 < len {
            let slice: String = chars[i..i+7].iter().collect();
            let prefix = if i + 8 < len && chars[i..i+8].iter().collect::<String>() == "https://" { "https://" } else if &slice == "http://" { "http://" } else { "" };

            if !prefix.is_empty() {
                let start = i;
                i += prefix.len();
                while i < len && !chars[i].is_whitespace() && chars[i] != ')' && chars[i] != ']' && chars[i] != '>' && chars[i] != '"' {
                    i += 1;
                }
                let url: String = chars[start..i].iter().collect();
                links.push(url);
                continue;
            }
        }

        i += 1;
    }

    if links.is_empty() {
        Ok("No links found.".to_string())
    } else {
        Ok(links.join("\n"))
    }
}

fn md_extract_headings(input: &Value) -> Result<String, String> {
    let md = get_str(input, "markdown")?;
    let mut headings = Vec::new();
    let lines: Vec<&str> = md.lines().collect();

    for line in &lines {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            let level = trimmed.chars().take_while(|c| *c == '#').count();
            if level <= 6 && level < trimmed.len() {
                let text = trimmed[level..].trim();
                if !text.is_empty() {
                    // Strip any trailing # from setext-style
                    let text = text.trim_end_matches('#').trim();
                    let indent = "  ".repeat(level - 1);
                    headings.push(format!("{indent}H{level}: {text}"));
                }
            }
        }
    }

    // Also check for underlined headings (=== or ---)
    let mut i = 0;
    while i + 1 < lines.len() {
        let line = lines[i];
        let next = lines[i + 1].trim();
        if !line.is_empty() && (next.chars().all(|c| c == '=') || next.chars().all(|c| c == '-')) && !next.is_empty() {
            let level = if next.starts_with('=') { 1 } else { 2 };
            headings.push(format!("H{level}: {line}", level = level, line = line));
            i += 1;
        }
        i += 1;
    }

    if headings.is_empty() {
        Ok("No headings found.".to_string())
    } else {
        Ok(headings.join("\n"))
    }
}

fn md_extract_code_blocks(input: &Value) -> Result<String, String> {
    let md = get_str(input, "markdown")?;
    let mut blocks = Vec::new();
    let lines: Vec<&str> = md.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let trimmed = lines[i].trim();
        if trimmed.starts_with("```") {
            let language = trimmed[3..].trim().to_string();
            let lang_label = if language.is_empty() { "(no language)".to_string() } else { language.clone() };
            let mut code_lines = Vec::new();
            i += 1;
            while i < lines.len() {
                if lines[i].trim() == "```" {
                    break;
                }
                code_lines.push(lines[i]);
                i += 1;
            }
            let code = code_lines.join("\n");
            // Truncate very long code blocks
            let snippet = if code.lines().count() > 20 {
                let first: Vec<&str> = code.lines().take(15).collect();
                format!("{}\n  ... ({} total lines, truncated)", first.join("\n  "), code.lines().count())
            } else {
                code.lines().map(|l| format!("  {l}")).collect::<Vec<_>>().join("\n")
            };
            blocks.push(format!("[{lang_label}]\n{snippet}"));
        }
        i += 1;
    }

    if blocks.is_empty() {
        Ok("No code blocks found.".to_string())
    } else {
        Ok(blocks.join("\n\n"))
    }
}
