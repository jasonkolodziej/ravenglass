#!/usr/bin/env bash
# Local Stalwart mail server (https://stalw.art, pure Rust) for the mail-imap and mail-jmap
# integration tests, without Docker. Fetches one pinned release binary for Linux x86_64 into
# tools/.cache/ (gitignored), checks its sha256, and runs it from a temp dir.
#
#   tools/stalwart.sh fetch    download and verify the binary
#   tools/stalwart.sh start    configure a fresh server in a temp dir and start it
#   tools/stalwart.sh stop     stop it and delete the temp dir
#   tools/stalwart.sh status   say whether it runs, and where
#
# The binary comes from the project's official container image (ghcr.io/stalwartlabs/stalwart),
# whose layers are content-addressed: the layer holding /usr/local/bin/stalwart is fetched by its
# sha256 digest and the extracted binary is checked against a second pinned sha256. Stalwart's
# GitHub release tarball has the same binary; the image is used because its digest pins it.
#
# Stalwart 1.x keeps its configuration in its own data store, so `start` writes a one-line
# config.json (RocksDB in the temp dir), starts the server once in recovery mode, applies the
# configuration below through its JMAP management API, and restarts it normally.
#
# Ports (127.0.0.1): IMAP 3145 (STARTTLS) / 3995 (TLS), submission 3587 (STARTTLS) / 3466 (TLS),
# HTTPS 3443 (JMAP: https://localhost:3443/.well-known/jmap). Test mail goes in by IMAP APPEND.
# Recovery mode briefly opens a plain HTTP port (18080) on all interfaces; it closes on restart.
# Users: `tools/stalwart.sh user NAME` creates NAME@ravenglass.test with password "correct-horse-battery-staple-42" (Stalwart refuses weak ones).
# The test certificate is <dir>/cert.pem; the temp dir's path is in tools/.cache/stalwart.dir.
# Then: RAVENGLASS_STALWART_TEST=1 cargo test -p mail-imap --test stalwart (and -p mail-jmap).
set -euo pipefail

VERSION="${STALWART_VERSION:-1.0.3}"
# ghcr.io/stalwartlabs/stalwart:v1.0.3, linux/amd64: the image layer that adds the binary
# (zstd-compressed tar), and the binary itself.
LAYER_1_0_3="sha256:f8b5c77ab92b736c6b6ce1041c4f965fbaa33bc40fd104f39d17039aea9e3902"
BINARY_SHA256_1_0_3="70a59885ce3bf48116a41f635b2040f1881162b6d3d6b6c72c1865a66045a4f5"
IMAGE="stalwartlabs/stalwart"

TOOLS="$(cd "$(dirname "$0")" && pwd)"
CACHE="$TOOLS/.cache"
BIN="$CACHE/stalwart-$VERSION"
POINTER="$CACHE/stalwart.dir"
DOMAIN="ravenglass.test"
ADMIN="admin:ravenglass-admin"
PASSWORD="correct-horse-battery-staple-42"
RECOVERY_PORT="${STALWART_RECOVERY_PORT:-18080}"
HTTPS_PORT=3443

fetch() {
    mkdir -p "$CACHE"
    [ -x "$BIN" ] && return 0
    if [ "$VERSION" != "1.0.3" ]; then echo "only 1.0.3 is pinned; add its digests to $0" >&2; exit 1; fi
    command -v zstd >/dev/null || { echo "zstd is needed to unpack the image layer (apt-get install zstd)" >&2; exit 1; }
    local work token
    work="$(mktemp -d)"
    trap 'rm -rf "$work"' RETURN
    echo "fetching Stalwart $VERSION from ghcr.io" >&2
    token="$(curl --fail --silent --show-error "https://ghcr.io/token?scope=repository:$IMAGE:pull" | sed -n 's/.*"token":"\([^"]*\)".*/\1/p')"
    curl --fail --silent --show-error --location -H "Authorization: Bearer $token" \
        --output "$work/layer" "https://ghcr.io/v2/$IMAGE/blobs/$LAYER_1_0_3"
    echo "${LAYER_1_0_3#sha256:}  $work/layer" | sha256sum --check --quiet || { echo "layer checksum mismatch" >&2; exit 1; }
    mkdir "$work/root"
    tar --zstd -xf "$work/layer" -C "$work/root" usr/local/bin/stalwart
    echo "$BINARY_SHA256_1_0_3  $work/root/usr/local/bin/stalwart" | sha256sum --check --quiet || { echo "binary checksum mismatch" >&2; exit 1; }
    mv "$work/root/usr/local/bin/stalwart" "$BIN"
    chmod 755 "$BIN"
}

current_dir() { [ -f "$POINTER" ] && cat "$POINTER"; }

running() {
    local dir
    dir="$(current_dir)" || return 1
    [ -f "$dir/stalwart.pid" ] && kill -0 "$(cat "$dir/stalwart.pid")" 2>/dev/null
}

# A JSON string literal holding the contents of a file.
json_file() { awk 'BEGIN { ORS = ""; print "\"" } { gsub(/\\/, "\\\\"); gsub(/"/, "\\\""); print $0 "\\n" } END { print "\"" }' "$1"; }

# POST one JMAP management request; fails on any error in the answer.
api() {
    local url="$1" body="$2" answer
    answer="$(curl --fail --silent --show-error -u "$ADMIN" -H 'Content-Type: application/json' "$url" -d "$body")"
    if echo "$answer" | grep -q '"notCreated"\|"notUpdated"\|"error"'; then
        echo "Stalwart refused the configuration: $answer" >&2
        return 1
    fi
    echo "$answer"
}

using='"using":["urn:ietf:params:jmap:core","urn:stalwart:jmap"]'

listener() { # name port protocol implicit-tls
    printf '"%s":{"name":"%s","bind":{"127.0.0.1:%s":true},"protocol":"%s","useTls":true,"implicitTls":%s}' "$1" "$1" "$2" "$3" "$4"
}

configure() {
    local dir="$1" url="http://127.0.0.1:$RECOVERY_PORT/jmap/"
    api "$url" "{$using,\"methodCalls\":[
        [\"x:Domain/set\",{\"create\":{\"d\":{\"name\":\"$DOMAIN\",\"certificateManagement\":{\"@type\":\"Manual\"},\"dkimManagement\":{\"@type\":\"Manual\"},\"dnsManagement\":{\"@type\":\"Manual\"}}}},\"c1\"],
        [\"x:Certificate/set\",{\"create\":{\"c\":{\"certificate\":{\"@type\":\"Text\",\"value\":$(json_file "$dir/cert.pem")},\"privateKey\":{\"@type\":\"Text\",\"secret\":$(json_file "$dir/key.pem")}}}},\"c2\"],
        [\"x:NetworkListener/set\",{\"create\":{$(listener imap 3145 imap false),$(listener imaps 3995 imap true),$(listener submission 3587 smtp false),$(listener submissions 3466 smtp true),$(listener https $HTTPS_PORT http true)}},\"c3\"]
    ]}" >/dev/null
}

wait_port() {
    for _ in $(seq 1 100); do
        if (exec 3<>"/dev/tcp/127.0.0.1/$1") 2>/dev/null; then return 0; fi
        sleep 0.1
    done
    return 1
}

launch() { # dir, extra env…
    local dir="$1"; shift
    env "$@" nohup "$BIN" --config "$dir/config.json" >>"$dir/stalwart.log" 2>&1 &
    echo $! >"$dir/stalwart.pid"
}

halt() {
    local dir="$1"
    [ -f "$dir/stalwart.pid" ] || return 0
    kill "$(cat "$dir/stalwart.pid")" 2>/dev/null || true
    for _ in $(seq 1 100); do kill -0 "$(cat "$dir/stalwart.pid")" 2>/dev/null || break; sleep 0.1; done
    rm -f "$dir/stalwart.pid"
}

start() {
    if running; then echo "already running from $(current_dir)" >&2; return 0; fi
    fetch
    local dir
    dir="$(mktemp -d "${TMPDIR:-/tmp}/ravenglass-stalwart.XXXXXX")"
    echo "$dir" >"$POINTER"
    openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 3650 \
        -subj "/CN=localhost" -addext "subjectAltName=DNS:localhost,IP:127.0.0.1" \
        -addext "basicConstraints=critical,CA:FALSE" -addext "extendedKeyUsage=serverAuth" \
        -keyout "$dir/key.pem" -out "$dir/cert.pem" >/dev/null 2>&1
    printf '{"@type":"RocksDb","path":"%s/data/"}\n' "$dir" >"$dir/config.json"

    launch "$dir" STALWART_RECOVERY_MODE=1 STALWART_RECOVERY_MODE_PORT="$RECOVERY_PORT" STALWART_RECOVERY_ADMIN="$ADMIN"
    wait_port "$RECOVERY_PORT" || { echo "Stalwart did not start; see $dir/stalwart.log" >&2; exit 1; }
    configure "$dir"
    halt "$dir"

    launch "$dir" STALWART_RECOVERY_ADMIN="$ADMIN" STALWART_PUBLIC_URL="https://localhost:$HTTPS_PORT"
    for port in 3145 3995 3587 3466 "$HTTPS_PORT"; do
        wait_port "$port" || { echo "Stalwart is not listening on $port; see $dir/stalwart.log" >&2; exit 1; }
    done
    echo "started from $dir, log in $dir/stalwart.log" >&2
}

user() {
    local dir name="$1"
    dir="$(current_dir)" || { echo "not running" >&2; exit 1; }
    local url="https://localhost:$HTTPS_PORT/jmap/" domain
    domain="$(curl --fail --silent --show-error --cacert "$dir/cert.pem" -u "$ADMIN" -H 'Content-Type: application/json' "$url" \
        -d "{$using,\"methodCalls\":[[\"x:Domain/query\",{},\"c1\"]]}" | sed -n 's/.*"ids":\["\([^"]*\)".*/\1/p')"
    curl --fail --silent --show-error --cacert "$dir/cert.pem" -u "$ADMIN" -H 'Content-Type: application/json' "$url" \
        -d "{$using,\"methodCalls\":[[\"x:Account/set\",{\"create\":{\"u\":{\"@type\":\"User\",\"name\":\"$name\",\"domainId\":\"$domain\",\"credentials\":{\"0\":{\"@type\":\"Password\",\"secret\":\"$PASSWORD\"}}}}},\"c1\"]]}"
    echo
}

stop() {
    local dir
    dir="$(current_dir)" || return 0
    halt "$dir"
    case "$dir" in */ravenglass-stalwart.*) rm -rf "$dir" ;; esac
    rm -f "$POINTER"
}

case "${1:-}" in
    fetch) fetch ;;
    start) start ;;
    stop) stop ;;
    status) if running; then echo "running from $(current_dir)"; else echo "not running"; exit 1; fi ;;
    user) user "${2:?usage: $0 user NAME}" ;;
    *) echo "usage: $0 fetch|start|stop|status|user NAME" >&2; exit 2 ;;
esac
