# GitHub Skill

The GitHub skill allows you to search for repositories, explore their file structure, and read file contents directly from GitHub. Use this when the user asks about open-source projects, wants to see code examples, or needs information about specific repositories.

## Capabilities

### 1. Searching for Repositories
Use `github_search_repositories` to find repositories matching a query. You can filter by language, user, or topic using GitHub's search syntax.

**Example Queries:**
- `topic:rust`
- `user:google machine learning`
- `helpcore`

### 2. Getting Repository Details
Use `github_get_repository` to get metadata like stars, forks, primary language, and the default branch for a specific repository.

### 3. Exploring Repository Structure
Use `github_list_repository_contents` to see the files and directories at any path within a repository. This is useful for finding where source code, documentation, or configuration files are located.

### 4. Reading File Contents
Use `github_get_file_content` to retrieve structured, paginated raw text for a specific file. The result includes the returned content, line range, total line count, and continuation metadata. This is perfect for reading `README.md`, `Cargo.toml`, source code files, or documentation.

## Guidelines
- **Rate Limits:** Public API requests are rate-limited. If you encounter errors, suggest the user configure a GitHub token in the plugin settings.
- **File Paths:** When listing or reading, use relative paths from the repository root (e.g., `src/lib.rs`, `README.md`).
- **Refs:** By default, tools use the repository's default branch. You can specify a branch, tag, or commit hash using the `ref` parameter.
- **Long Files:** Follow `next_start_line` and `next_start_column` while `truncated` is true. You can request an inclusive `start_line`/`end_line` range, with at most 200 lines per call. `start_column` is normally 1 and only changes for an unusually long single line.
