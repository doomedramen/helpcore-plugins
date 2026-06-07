Search the web using `duckduckgo_search`. Pass a query string and optionally a `max_results` count (default 5, clamped to 1–10). Returns ranked plain-text results each containing a title, the direct URL, and a short text snippet.

Use this whenever you need current information, recent events, technical answers, documentation links, or any topic that benefits from live web results.

To read the full content of a promising result, use `page_fetch` with the returned URL.

Limitations: results are short snippet previews — use page_fetch for full article text. The search is powered by DuckDuckGo's non-JavaScript HTML endpoint, so not all results may match those from a browser search. No API key or setup is required.
