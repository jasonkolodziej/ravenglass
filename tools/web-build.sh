#!/usr/bin/env bash
# Build the web app (architecture §8, §14) into a static folder: the `ravenglass` crate for
# wasm32 through Rune's own web build (scripts/web-build.sh in the pinned Rune checkout: cargo,
# wasm-bindgen, optional wasm-opt, Rune's fonts), then the page around it: index.html with a
# strict Content-Security-Policy, boot.js, the web app manifest, icons and the service worker.
#
#   tools/web-build.sh [--relay ORIGIN] [--out DIR] [--dev]
#
#   --relay ORIGIN  the Relay's origin, e.g. https://relay.example.org (default: the page's own)
#   --out DIR       the static folder (default: web/dist)
#   --dev           a fast, unoptimized build (twice the size; never ship it)
#
# Needs python3, and rsvg-convert or ImageMagick for the PNG icons (without either, the manifest
# names only the SVG). The wasm-bindgen CLI matching Cargo.lock is installed into
# tools/.cache/ when missing; wasm-opt -Oz runs when it is on PATH.
set -euo pipefail
usage="usage: tools/web-build.sh [--relay ORIGIN] [--out DIR] [--dev]"
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
relay=
out=web/dist
dev=false
while (($#)); do
    case "$1" in
        --relay) relay=${2:?--relay needs an origin}; shift 2 ;;
        --out) out=${2:?--out needs a directory}; shift 2 ;;
        --dev) dev=true; shift ;;
        -h|--help) echo "$usage"; exit 0 ;;
        *) echo "unknown option: $1" >&2; echo "$usage" >&2; exit 2 ;;
    esac
done
cd "$root"
relay=${relay%/}
if [[ -n $relay && ! $relay =~ ^https?://[^/[:space:]]+$ ]]; then
    echo "--relay takes an origin like https://relay.example.org (no path)" >&2
    exit 2
fi

# The wasm-bindgen CLI must be the exact version of the crate in Cargo.lock.
version=$(python3 -c 'import sys,tomllib; print(next(p["version"] for p in tomllib.load(open(sys.argv[1],"rb"))["package"] if p["name"]=="wasm-bindgen"))' Cargo.lock)
if ! command -v wasm-bindgen >/dev/null || [[ $(wasm-bindgen --version) != "wasm-bindgen $version" ]]; then
    tools="$root/tools/.cache/wasm-bindgen-$version"
    if [[ ! -x $tools/bin/wasm-bindgen ]]; then
        echo "Installing wasm-bindgen-cli $version into $tools"
        cargo install wasm-bindgen-cli --version "$version" --locked --root "$tools"
    fi
    export PATH="$tools/bin:$PATH"
fi

# Rune's web build, from the Rune this workspace pins.
rune=$(cargo metadata --format-version 1 --filter-platform wasm32-unknown-unknown | python3 -c '
import json,os,sys
m=json.load(sys.stdin)
p=next(p for p in m["packages"] if p["name"]=="rune")
print(os.path.dirname(os.path.dirname(os.path.dirname(p["manifest_path"]))))')
args=(ravenglass --out "$out" --no-html)
if $dev; then args+=(--dev)
elif command -v wasm-opt >/dev/null; then args+=(--opt)
else echo "wasm-opt is not on PATH: skipping wasm-opt -Oz (the wasm stays larger)"; fi
"$rune/scripts/web-build.sh" "${args[@]}"
# Rune fetches its full Inter faces from beside app.js once the first frame is up. Its script
# copies them when it finds them in the build's dependency file; make sure they are there.
for face in "$rune"/crates/rune/assets/web/Inter-*.ttf; do
    [[ -e $out/$(basename "$face") ]] || cp "$face" "$out/"
done

# The page around the wasm.
web="$root/web"
theme=$(python3 -c 'import sys,tomllib; t=tomllib.load(open(sys.argv[1],"rb")); print(t["palette"][t["roles"]["ui.bg"]])' assets/themes/ravenglass-night.toml)
cp "$web/boot.js" "$web/app.css" "$out/"
cp assets/icon/ravenglass-icon-D_braille.svg "$out/icon.svg"
pngs=()
for size in 192 512; do
    if command -v rsvg-convert >/dev/null; then
        rsvg-convert -w "$size" -h "$size" assets/icon/ravenglass-icon-D_braille.svg -o "$out/icon-$size.png" && pngs+=("$size")
    elif command -v convert >/dev/null; then
        convert -background none -resize "${size}x$size" assets/icon/ravenglass-icon-D_braille.svg "$out/icon-$size.png" && pngs+=("$size")
    fi
done
((${#pngs[@]})) || echo "Neither rsvg-convert nor ImageMagick found: the manifest names only the SVG icon."
connect=
if [[ -n $relay ]]; then
    connect="$relay ${relay/#http/ws}"
fi
stamp=$(python3 -c 'import hashlib,sys; print(hashlib.sha256(open(sys.argv[1],"rb").read()).hexdigest()[:12])' "$out/app_bg.wasm")
# Rune adds a few constant <style> elements; the policy allows exactly those, by hash.
styles=$(python3 -c '
import base64,hashlib,pathlib,re,sys
found=set()
for js in pathlib.Path(sys.argv[1]).rglob("*.js"):
    for css in re.findall(r"style\.textContent = \x27([^\x27]*)\x27", js.read_text()):
        found.add("\x27sha256-" + base64.b64encode(hashlib.sha256(css.encode()).digest()).decode() + "\x27")
print(" ".join(sorted(found)))' "$out/snippets")
OUT="$out" STYLES="$styles" RELAY="$relay" CONNECT="$connect" THEME="$theme" STAMP="$stamp" PNGS="${pngs[*]-}" WEB="$web" python3 - <<'PY'
import json, os, pathlib
out, web = pathlib.Path(os.environ["OUT"]), pathlib.Path(os.environ["WEB"])
theme = os.environ["THEME"]
icons = [{"src": "icon.svg", "sizes": "any", "type": "image/svg+xml", "purpose": "any"}]
icons += [{"src": f"icon-{s}.png", "sizes": f"{s}x{s}", "type": "image/png", "purpose": "any maskable"} for s in os.environ["PNGS"].split()]
manifest = {
    "name": "Ravenglass", "short_name": "Ravenglass", "description": "A keyboard-first mail client.",
    "start_url": "./", "scope": "./", "display": "standalone",
    "background_color": theme, "theme_color": theme, "icons": icons,
}
(out / "manifest.webmanifest").write_text(json.dumps(manifest, indent=2) + "\n")
page = (web / "index.html").read_text()
for key, value in {"RELAY": os.environ["RELAY"], "CONNECT": os.environ["CONNECT"], "STYLES": os.environ["STYLES"], "THEME_COLOR": theme}.items():
    page = page.replace("{{" + key + "}}", value)
(out / "index.html").write_text(page)
# Everything in the folder (wasm-bindgen's `snippets/` included) but the worker itself.
shell = sorted(p.relative_to(out).as_posix() for p in out.rglob("*") if p.is_file() and not any(part.startswith(".") for part in p.relative_to(out).parts) and p.name != "sw.js")
worker = (web / "sw.js").read_text().replace("{{VERSION}}", os.environ["STAMP"]).replace("{{SHELL}}", json.dumps(shell))
(out / "sw.js").write_text(worker)
PY
echo "Web app in $out (relay: ${relay:-this origin})"
