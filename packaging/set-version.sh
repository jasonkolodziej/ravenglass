#!/usr/bin/env bash
# Stamp one SemVer on the app and the Relay (architecture §14: one version for both) before a
# release build: `packaging/set-version.sh 1.2.3` (a leading `v` is dropped). The release
# workflow runs it on tags; the change is never committed. Cargo.lock is refreshed for the two
# workspace packages only.
set -euo pipefail
version="${1:?usage: set-version.sh X.Y.Z[-pre]}"
version="${version#v}"
if ! [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]]; then
  echo "not a SemVer version: $version" >&2
  exit 1
fi
root="$(cd "$(dirname "$0")/.." && pwd)"
for manifest in app/Cargo.toml crates/ravenglass-relay/Cargo.toml; do
  # The first `version = "…"` line is the [package] version in both manifests.
  python3 - "$root/$manifest" "$version" <<'PY'
import re, sys
path, version = sys.argv[1], sys.argv[2]
text = open(path, encoding="utf-8").read()
new, n = re.subn(r'(?m)^version = "[^"]*"', f'version = "{version}"', text, count=1)
if n != 1:
    sys.exit(f"{path}: no package version line")
open(path, "w", encoding="utf-8").write(new)
PY
done
cargo update --manifest-path "$root/Cargo.toml" --workspace --quiet
echo "version set to $version"
