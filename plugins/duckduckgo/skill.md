Search the web using `duckduckgo_search`. Pass a query string and optionally a `max_results` count (default 5, max 10). Returns ranked results each containing a title, direct URL, and snippet.

Do NOT try to guess or construct URLs yourself — always search first. Use `duckduckgo_search` as your primary tool whenever the user asks for current information, recent events, recommendations ("best restaurants in X"), technical answers, documentation links, or anything that requires live web knowledge.

DuckDuckGo supports standard search operators — use them to narrow results instead of relying on generic queries: `site:example.com` restricts to a domain, `"exact phrase"` matches literal text, and `-word` excludes a term.

After you get search results, use `page_fetch` on specific result URLs only when the snippet alone isn't enough to answer the question.

If a search returns no results or an error (e.g. a verification-challenge response), don't fabricate an answer — rephrase the query with different or more general terms and try again, or tell the user the search didn't turn up anything useful.

Limitations: results are short snippet previews. The search is powered by DuckDuckGo's non-JavaScript HTML endpoint, so results may differ slightly from a browser search. No API key or setup needed.
