#!/usr/bin/env bash
# Checks that every copy of the values in packaging/identity.toml agrees with it.
#   packaging/check-identity.sh          exit 1 and name each file that disagrees
#   packaging/check-identity.sh --list   print where each value must appear
set -euo pipefail
cd "$(dirname "$0")/.."

value() { # value <section> <key>
  awk -v s="[$1]" -v k="$2" '
    /^\[/ { in_s = ($0 == s) }
    in_s && $1 == k { sub(/^[^=]*= *"/, ""); sub(/".*$/, ""); print; exit }
  ' packaging/identity.toml
}

id=$(value app identifier)
name=$(value publisher name)
copyright=$(value publisher copyright)
license=$(value publisher license)

# Each line: the value, then a file that must contain it verbatim.
places() {
  printf '%s\t%s\n' \
    "identifier = \"$id\"" app/Cargo.toml \
    "pub const APP_ID: &str = \"$id\";" app/src/links_in.rs \
    "id: $id" "packaging/flatpak/$id.yml" \
    "<id>$id</id>" "packaging/linux/$id.metainfo.xml" \
    "\"../packaging/linux/$id.metainfo.xml\"" app/Cargo.toml \
    "publisher = \"$name\"" app/Cargo.toml \
    "authors = [\"$name\"]" app/Cargo.toml \
    "<name>$name</name>" "packaging/linux/$id.metainfo.xml" \
    "copyright = \"$copyright\"" app/Cargo.toml \
    "<string>$copyright</string>" packaging/macos/Info.plist \
    "license = \"$license\"" Cargo.toml \
    "<project_license>$license</project_license>" "packaging/linux/$id.metainfo.xml" \
    "$license License" LICENSE
}

if [ "${1:-}" = "--list" ]; then
  places | awk -F'\t' '{ printf "%-60s %s\n", $2, $1 }'
  exit 0
fi

bad=0
while IFS=$'\t' read -r want file; do
  if [ ! -f "$file" ]; then
    echo "identity: $file is missing (expected to hold: $want)"; bad=1
  elif ! grep -qF -- "$want" "$file"; then
    echo "identity: $file does not contain: $want"; bad=1
  fi
done < <(places)
[ "$bad" = 0 ] && echo "identity: every copy matches packaging/identity.toml"
exit "$bad"
