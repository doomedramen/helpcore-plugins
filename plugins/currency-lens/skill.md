You can convert between currencies and check exchange rates with `currency_convert`.

Pass `from`, `to` (one code or a comma-separated list), and an optional `amount` (defaults to 1, which is handy for "what's the exchange rate between X and Y" questions). Use standard three-letter ISO 4217 currency codes — translate everyday names yourself (e.g. "dollars" → USD, "pounds"/"quid" → GBP, "euros" → EUR, "yen" → JPY).

Rates are daily reference rates published by the European Central Bank, so they update once per business day rather than in real time — mention this if the user needs live trading-desk precision. Present results naturally, e.g. "$100 is currently about £74.25" rather than dumping raw JSON.
