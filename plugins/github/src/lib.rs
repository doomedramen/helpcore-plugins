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

    Ok(resp.body)
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
}
