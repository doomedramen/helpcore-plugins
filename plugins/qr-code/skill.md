You can generate QR codes from any text with `qr_generate`. Provide the content (URL, text, etc.) and optionally specify the output format:

- `png` (default) — returns a base64-encoded SVG data URI (`data:image/svg+xml;base64,...`). Embed this in Markdown as `![QR Code](<data uri>)` for web display.
- `matrix` — returns a compact 0/1 grid (each row on a line) for programmatic rendering in CLI/TUI apps. 1 = black module, 0 = white module.
- `ascii` — a QR code rendered as Unicode block characters (█ and space), viewable directly in terminals.

The generated QR code uses byte-mode encoding with medium error correction (ECC level M), supporting up to ~200 characters (version 6). If the content is too long, the tool will tell you to shorten it.

**IMPORTANT**: When the format is `png`, always embed the returned data URI in your response as `![QR Code](<data uri>)` so it renders as an image in web UIs. When the format is `matrix`, you can render it as block characters programmatically or pass it through for TUI rendering. When the format is `ascii`, display it directly in a code block.
