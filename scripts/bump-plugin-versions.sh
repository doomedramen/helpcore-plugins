#!/usr/bin/env bash
set -euo pipefail

# Get staged files that changed (excluding version files themselves)
staged=$(git diff --cached --name-only --diff-filter=ACMR | grep '^plugins/' | grep -v -E '(manifest\.toml|Cargo\.toml)$' || true)
[ -z "$staged" ] && exit 0

# Extract unique plugin directories
dirs=$(echo "$staged" | cut -d/ -f1-2 | sort -u)

bumped=()

for dir in $dirs; do
  manifest="$dir/manifest.toml"
  cargo="$dir/Cargo.toml"

  # Bump manifest.toml
  if [ -f "$manifest" ]; then
    awk -F'"' '/^version = "/ { split($2, v, "."); $0 = "version = \"" v[1] "." v[2] "." v[3]+1 "\"" } 1' "$manifest" > "$manifest.tmp" && mv "$manifest.tmp" "$manifest"
    bumped+=("$manifest")
  fi

  # Bump Cargo.toml
  if [ -f "$cargo" ]; then
    awk -F'"' '/^version = "/ { split($2, v, "."); $0 = "version = \"" v[1] "." v[2] "." v[3]+1 "\"" } 1' "$cargo" > "$cargo.tmp" && mv "$cargo.tmp" "$cargo"
    bumped+=("$cargo")
  fi
done

# Re-stage bumped version files so they're included in the commit
if [ ${#bumped[@]} -gt 0 ]; then
  git add "${bumped[@]}"
fi
