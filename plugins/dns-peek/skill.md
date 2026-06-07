You can look up DNS records for any domain with `dns_lookup`.

Pass a `domain` and optionally a record `type` (defaults to "A" — the IPv4 address). Use "MX" for mail server questions, "TXT" for SPF/DKIM/domain-verification records, "NS" for name servers, "AAAA" for IPv6, "CNAME" for aliases, and so on.

This is a read-only diagnostic lookup via a public DNS resolver — results reflect what's currently published, which may differ slightly from a user's local resolver due to caching/propagation. Present records plainly (e.g. "example.com points to 93.184.216.34") and explain technical record types in plain language for non-technical users (e.g. "MX records are mail-delivery server, lower priority numbers are preferred").
