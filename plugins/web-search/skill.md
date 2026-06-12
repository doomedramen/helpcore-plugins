Search the live web using `web_search`. Pass a query and optionally `max_results` (default 5, max 10) or a `freshness` limit (`day`, `week`, `month`, or `year`). Results contain a title, direct URL, and snippet when the configured provider supplies one.

Use `web_search` whenever the user asks for current information, recent events, recommendations, technical answers, documentation links, or anything else requiring live web knowledge. Do not guess URLs when a search can locate the source.

Use standard search operators to narrow results: `site:example.com` restricts results to a domain, `"exact phrase"` matches literal text, `-word` excludes a term, and `filetype:pdf` restricts file type.

After searching, use `page_fetch` on specific result URLs when snippets do not provide enough information. Prefer authoritative and primary sources, and compare multiple sources for claims where accuracy matters.

The plugin can use Brave Search, SearXNG, or Whoogle. Brave requires an API key. Self-hosted SearXNG and Whoogle require a base URL; SearXNG must have JSON enabled in `search.formats`. The `week` freshness filter is available only with Brave. If authentication, blocking, or rate limiting fails, explain the error rather than fabricating search results.

Prefer SearXNG for self-hosting. Whoogle depends on scraping Google and may be blocked, so treat it as an experimental fallback.
