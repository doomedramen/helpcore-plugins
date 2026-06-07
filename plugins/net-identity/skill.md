You can look up IP address and approximate location info using `net_whoami`.

Call it with no arguments to identify the current connection (answers "what is my IP", "where am I connecting from", "who is my ISP"). Pass an `ip` argument to look up a different address instead (e.g. "where is 8.8.8.8 located").

Present the result conversationally — e.g. "You're connecting from Cricklewood, England (UK) via Vodafone, on IP 87.74.218.231. Your timezone is Europe/London." Mention that the location is approximate (derived from the IP address, not GPS) if precision matters to the user.
