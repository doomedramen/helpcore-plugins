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

struct GitHub;

impl Guest for GitHub {
    fn call(tool: String, input_json: String) -> Result<String, String> {
        let input: Value =
            serde_json::from_str(&input_json).map_err(|e| format!("invalid input JSON: {e}"))?;

        match tool.as_str() {
            "github_search_repositories" => search_repositories(&input),
            "github_get_repository" => get_repository(&input),
            "github_list_repository_contents" => list_repository_contents(&input),
            "github_get_file_content" => get_file_content(&input),
            _ => Err(format!("unknown tool: {tool}")),
        }
    }
}

export!(GitHub);

const DEFAULT_PAGE_LINES: usize = 100;
const MAX_PAGE_LINES: usize = 200;
const MAX_CONTENT_JSON_BYTES: usize = 7_500;

// ── HTTP Helpers ──────────────────────────────────────────────────────────────

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

#[derive(Serialize)]
struct PaginatedText {
    content: String,
    start_line: usize,
    start_column: usize,
    end_line: usize,
    end_column: usize,
    total_lines: usize,
    truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_start_line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_start_column: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hint: Option<String>,
}

#[derive(Serialize)]
struct FileContentResult<'a> {
    repository: String,
    path: &'a str,
    reference: &'a str,
    #[serde(flatten)]
    page: PaginatedText,
}

fn positive_usize(input: &Value, key: &str, default: usize) -> Result<usize, String> {
    match input.get(key) {
        None => Ok(default),
        Some(value) => {
            let raw = value
                .as_u64()
                .ok_or_else(|| format!("{key} must be a positive integer"))?;
            let parsed = usize::try_from(raw)
                .map_err(|_| format!("{key} is too large for this platform"))?;
            if parsed == 0 {
                return Err(format!("{key} must be at least 1"));
            }
            Ok(parsed)
        }
    }
}

fn json_escaped_char_bytes(c: char) -> usize {
    serde_json::to_string(&c.to_string())
        .map(|encoded| encoded.len().saturating_sub(2))
        .unwrap_or(c.len_utf8())
}

fn paginate_text(text: &str, input: &Value) -> Result<PaginatedText, String> {
    let lines: Vec<&str> = text.lines().collect();
    let total_lines = lines.len();
    let start_line = positive_usize(input, "start_line", 1)?;
    let start_column = positive_usize(input, "start_column", 1)?;

    if total_lines == 0 {
        return Ok(PaginatedText {
            content: String::new(),
            start_line: 1,
            start_column: 1,
            end_line: 0,
            end_column: 0,
            total_lines: 0,
            truncated: false,
            next_start_line: None,
            next_start_column: None,
            hint: None,
        });
    }
    if start_line > total_lines {
        return Err(format!(
            "start_line {start_line} exceeds the content's {total_lines} lines"
        ));
    }

    let first_line_chars = lines[start_line - 1].chars().count();
    let max_start_column = first_line_chars.max(1);
    if start_column > max_start_column {
        return Err(format!(
            "start_column {start_column} exceeds line {start_line}'s {first_line_chars} characters"
        ));
    }

    let default_end = start_line.saturating_add(DEFAULT_PAGE_LINES - 1);
    let requested_end = positive_usize(input, "end_line", default_end)?.max(start_line);
    let max_end = start_line.saturating_add(MAX_PAGE_LINES - 1);
    let selected_end = requested_end.min(max_end).min(total_lines);

    let mut content = String::new();
    let mut escaped_bytes = 0;
    let mut end_line = start_line;
    let mut end_column = start_column.saturating_sub(1);
    let mut next_position = None;

    'lines: for line_number in start_line..=selected_end {
        let line = lines[line_number - 1];
        let column_offset = if line_number == start_line {
            start_column - 1
        } else {
            0
        };

        if line_number > start_line {
            if escaped_bytes + 2 > MAX_CONTENT_JSON_BYTES {
                next_position = Some((line_number, 1));
                break;
            }
            content.push('\n');
            escaped_bytes += 2;
        }

        let mut consumed_column = column_offset;
        for c in line.chars().skip(column_offset) {
            let encoded_len = json_escaped_char_bytes(c);
            if escaped_bytes + encoded_len > MAX_CONTENT_JSON_BYTES {
                next_position = Some((line_number, consumed_column + 1));
                end_line = line_number;
                end_column = consumed_column;
                break 'lines;
            }
            content.push(c);
            escaped_bytes += encoded_len;
            consumed_column += 1;
        }
        end_line = line_number;
        end_column = consumed_column;
    }

    if next_position.is_none() && selected_end < total_lines {
        next_position = Some((selected_end + 1, 1));
    }

    let (next_start_line, next_start_column) = next_position
        .map(|(line, column)| (Some(line), Some(column)))
        .unwrap_or((None, None));
    let hint = next_position.map(|(line, column)| {
        format!(
            "Showing line {start_line}, column {start_column} through line {end_line}, column \
             {end_column} of {total_lines}. Continue with start_line={line}, start_column={column}."
        )
    });

    Ok(PaginatedText {
        content,
        start_line,
        start_column,
        end_line,
        end_column,
        total_lines,
        truncated: next_position.is_some(),
        next_start_line,
        next_start_column,
        hint,
    })
}

fn github_request<T: for<'de> Deserialize<'de>>(
    method: &str,
    url: &str,
    body: Option<Value>,
) -> Result<T, String> {
    let mut headers = serde_json::Map::new();
    headers.insert(
        "User-Agent".into(),
        Value::String("helpcore-github-plugin/0.1.0".into()),
    );
    headers.insert(
        "Accept".into(),
        Value::String("application/vnd.github+json".into()),
    );
    headers.insert(
        "X-GitHub-Api-Version".into(),
        Value::String("2022-11-28".into()),
    );

    match host::secret_read("token") {
        Ok(token) if !token.is_empty() => {
            headers.insert(
                "Authorization".into(),
                Value::String(format!("Bearer {}", token.trim())),
            );
        }
        Err(e) if e.contains("not approved") => return Err(e),
        _ => {} // Token not configured or empty — proceed unauthenticated
    }

    let req = HttpRequest {
        method,
        url: url.to_string(),
        headers,
        body: body.map(|b| serde_json::to_string(&b).unwrap_or_default()),
    };

    let req_json = serde_json::to_string(&req).map_err(|e| e.to_string())?;
    let resp_json = host::http_request(&req_json)?;
    let resp: HttpResponse = serde_json::from_str(&resp_json)
        .map_err(|e| format!("failed to parse HTTP response: {e}"))?;

    if resp.status >= 400 {
        return Err(format!(
            "GitHub API returned HTTP {}: {}",
            resp.status, resp.body
        ));
    }

    serde_json::from_str(&resp.body).map_err(|e| format!("failed to parse GitHub response: {e}"))
}

// ── Search ────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct SearchResponse {
    items: Vec<RepoBrief>,
}

#[derive(Deserialize, Serialize)]
struct RepoBrief {
    full_name: String,
    description: Option<String>,
    stargazers_count: u32,
    html_url: String,
    language: Option<String>,
}

fn search_repositories(input: &Value) -> Result<String, String> {
    let query = input
        .get("query")
        .and_then(Value::as_str)
        .ok_or("query is required")?;
    let sort = input.get("sort").and_then(Value::as_str).unwrap_or("stars");
    let order = input.get("order").and_then(Value::as_str).unwrap_or("desc");

    let url = format!(
        "https://api.github.com/search/repositories?q={}&sort={}&order={}",
        url_encode(query),
        sort,
        order
    );

    let resp: SearchResponse = github_request("GET", &url, None)?;

    if resp.items.is_empty() {
        return Ok(format!("No repositories found for query: {query}"));
    }

    let mut result = format!("Search results for '{query}':\n");
    for repo in resp.items.iter().take(10) {
        let desc = repo.description.as_deref().unwrap_or("No description");
        let lang = repo.language.as_deref().unwrap_or("Unknown");
        result.push_str(&format!(
            "\n• {} ({} stars, language: {})\n  {}\n  URL: {}\n",
            repo.full_name, repo.stargazers_count, lang, desc, repo.html_url
        ));
    }

    Ok(result)
}

// ── Repository ────────────────────────────────────────────────────────────────

#[derive(Deserialize, Serialize)]
struct RepoDetail {
    full_name: String,
    description: Option<String>,
    stargazers_count: u32,
    forks_count: u32,
    subscribers_count: Option<u32>,
    language: Option<String>,
    default_branch: String,
    license: Option<License>,
    topics: Vec<String>,
}

#[derive(Deserialize, Serialize)]
struct License {
    name: String,
}

fn get_repository(input: &Value) -> Result<String, String> {
    let owner = input
        .get("owner")
        .and_then(Value::as_str)
        .ok_or("owner is required")?;
    let repo = input
        .get("repo")
        .and_then(Value::as_str)
        .ok_or("repo is required")?;

    validate_path_segment(owner, "owner")?;
    validate_path_segment(repo, "repo")?;

    let url = format!("https://api.github.com/repos/{owner}/{repo}");
    let resp: RepoDetail = github_request("GET", &url, None)?;

    let mut result = format!("Repository: {}\n", resp.full_name);
    result.push_str(&format!(
        "Description: {}\n",
        resp.description.as_deref().unwrap_or("None")
    ));
    result.push_str(&format!(
        "Stars: {}, Forks: {}\n",
        resp.stargazers_count, resp.forks_count
    ));
    result.push_str(&format!(
        "Primary Language: {}\n",
        resp.language.as_deref().unwrap_or("Unknown")
    ));
    result.push_str(&format!("Default Branch: {}\n", resp.default_branch));
    if let Some(license) = resp.license {
        result.push_str(&format!("License: {}\n", license.name));
    }
    if !resp.topics.is_empty() {
        result.push_str(&format!("Topics: {}\n", resp.topics.join(", ")));
    }

    Ok(result)
}

// ── Contents ──────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct ContentItem {
    name: String,
    #[serde(rename = "type")]
    item_type: String, // "file" or "dir"
    size: u64,
}

fn list_repository_contents(input: &Value) -> Result<String, String> {
    let owner = input
        .get("owner")
        .and_then(Value::as_str)
        .ok_or("owner is required")?;
    let repo = input
        .get("repo")
        .and_then(Value::as_str)
        .ok_or("repo is required")?;
    let path = input.get("path").and_then(Value::as_str).unwrap_or("");
    let reference = input.get("ref").and_then(Value::as_str);

    validate_path_segment(owner, "owner")?;
    validate_path_segment(repo, "repo")?;

    let mut url = format!("https://api.github.com/repos/{owner}/{repo}/contents/{path}");
    if let Some(r) = reference {
        url.push_str(&format!("?ref={}", url_encode(r)));
    }

    let items: Vec<ContentItem> = github_request("GET", &url, None)?;

    if items.is_empty() {
        return Ok(format!("Path '{path}' is empty or not found."));
    }

    let mut dirs = Vec::new();
    let mut files = Vec::new();

    for item in items {
        if item.item_type == "dir" {
            dirs.push(item.name);
        } else {
            files.push(format!("{} ({} bytes)", item.name, item.size));
        }
    }

    dirs.sort();
    files.sort();

    let mut result = format!("Contents of {owner}/{repo} at '{path}':\n");
    if !dirs.is_empty() {
        result.push_str("\nDirectories:\n");
        for d in dirs {
            result.push_str(&format!("  / {}\n", d));
        }
    }
    if !files.is_empty() {
        result.push_str("\nFiles:\n");
        for f in files {
            result.push_str(&format!("  • {}\n", f));
        }
    }

    Ok(result)
}

// ── File Content ──────────────────────────────────────────────────────────────

fn get_file_content(input: &Value) -> Result<String, String> {
    let owner = input
        .get("owner")
        .and_then(Value::as_str)
        .ok_or("owner is required")?;
    let repo = input
        .get("repo")
        .and_then(Value::as_str)
        .ok_or("repo is required")?;
    let path = input
        .get("path")
        .and_then(Value::as_str)
        .ok_or("path is required")?;
    let reference = input.get("ref").and_then(Value::as_str);

    validate_path_segment(owner, "owner")?;
    validate_path_segment(repo, "repo")?;

    // Try to get default branch if ref is not provided to construct raw URL
    let branch = match reference {
        Some(r) => r.to_string(),
        None => {
            let url = format!("https://api.github.com/repos/{owner}/{repo}");
            let repo_info: RepoDetail = github_request("GET", &url, None)?;
            repo_info.default_branch
        }
    };

    let raw_url = format!("https://raw.githubusercontent.com/{owner}/{repo}/{branch}/{path}");

    let mut headers = serde_json::Map::new();
    headers.insert(
        "User-Agent".into(),
        Value::String("helpcore-github-plugin/0.1.0".into()),
    );

    match host::secret_read("token") {
        Ok(token) if !token.is_empty() => {
            headers.insert(
                "Authorization".into(),
                Value::String(format!("Bearer {}", token.trim())),
            );
        }
        Err(e) if e.contains("not approved") => return Err(e),
        _ => {}
    }

    let req = HttpRequest {
        method: "GET",
        url: raw_url,
        headers,
        body: None,
    };

    let req_json = serde_json::to_string(&req).map_err(|e| e.to_string())?;
    let resp_json = host::http_request(&req_json)?;
    let resp: HttpResponse = serde_json::from_str(&resp_json)
        .map_err(|e| format!("failed to parse HTTP response: {e}"))?;

    if resp.status >= 400 {
        return Err(format!(
            "Failed to fetch file: HTTP {}: {}",
            resp.status, resp.body
        ));
    }

    let page = paginate_text(&resp.body, input)?;
    serde_json::to_string(&FileContentResult {
        repository: format!("{owner}/{repo}"),
        path,
        reference: &branch,
        page,
    })
    .map_err(|e| format!("failed to serialize file content: {e}"))
}

// ── Utils ─────────────────────────────────────────────────────────────────────

fn url_encode(s: &str) -> String {
    let mut encoded = String::with_capacity(s.len());
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(*b as char);
            }
            b' ' => encoded.push('+'),
            _ => {
                encoded.push('%');
                encoded.push(
                    char::from_digit((b >> 4) as u32, 16)
                        .unwrap()
                        .to_ascii_uppercase(),
                );
                encoded.push(
                    char::from_digit((b & 0xf) as u32, 16)
                        .unwrap()
                        .to_ascii_uppercase(),
                );
            }
        }
    }
    encoded
}

fn validate_path_segment(value: &str, field: &str) -> Result<(), String> {
    if value.is_empty() || value.contains('/') || value.contains("..") || !value.is_ascii() {
        return Err(format!(
            "invalid {field} '{value}': must be non-empty ASCII without '/' or '..'"
        ));
    }
    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_url_encode() {
        assert_eq!(url_encode("rust-lang/rust"), "rust-lang%2Frust");
        assert_eq!(url_encode("hello world"), "hello+world");
        assert_eq!(
            url_encode("special!@#$%^&*()"),
            "special%21%40%23%24%25%5E%26%2A%28%29"
        );
    }

    #[test]
    fn test_validate_path_segment() {
        assert!(validate_path_segment("rust-lang", "owner").is_ok());
        assert!(validate_path_segment("rust", "repo").is_ok());
        assert!(validate_path_segment("owner/repo", "owner").is_err());
        assert!(validate_path_segment("..", "repo").is_err());
        assert!(validate_path_segment("", "owner").is_err());
    }

    #[test]
    fn test_paginate_text_returns_continuation() {
        let input = serde_json::json!({"start_line": 2, "end_line": 3});
        let page = paginate_text("one\ntwo\nthree\nfour", &input).unwrap();

        assert_eq!(page.content, "two\nthree");
        assert_eq!(page.total_lines, 4);
        assert_eq!(page.next_start_line, Some(4));
        assert_eq!(page.next_start_column, Some(1));
        assert!(page.truncated);
    }

    #[test]
    fn test_paginate_text_continues_long_line() {
        let text = "x".repeat(MAX_CONTENT_JSON_BYTES + 100);
        let first = paginate_text(&text, &serde_json::json!({})).unwrap();
        let second = paginate_text(
            &text,
            &serde_json::json!({
                "start_line": first.next_start_line.unwrap(),
                "start_column": first.next_start_column.unwrap()
            }),
        )
        .unwrap();

        assert_eq!(format!("{}{}", first.content, second.content), text);
        assert!(!second.truncated);
    }

    #[test]
    fn test_serialized_file_stays_below_helpcore_limit() {
        let text = "\"".repeat(MAX_CONTENT_JSON_BYTES);
        let page = paginate_text(&text, &serde_json::json!({})).unwrap();
        let result = serde_json::to_string(&FileContentResult {
            repository: "owner/repository".to_string(),
            path: "path/to/file.json",
            reference: "main",
            page,
        })
        .unwrap();

        assert!(result.len() < 10_000);
    }
}
