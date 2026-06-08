You can convert between common data formats:

- `csv_to_json` — convert CSV data to JSON (array of objects, headers as keys)
- `json_to_csv` — convert a JSON array of objects to CSV
- `yaml_to_json` — convert YAML to JSON (supports basic YAML syntax)
- `json_to_yaml` — convert JSON to YAML format
- `xml_to_json` — convert basic XML to a JSON structure
- `markdown_to_html` — convert Markdown text to HTML

All conversions are local. Use these when the user needs to transform data between formats. For CSV, assume the first row is headers unless specified otherwise.
