#!/usr/bin/env bash
# Local IMAP/SMTP server for the mail-imap integration tests, without Docker: fetches the GreenMail
# standalone jar from Maven Central into tools/.cache/ (gitignored) and runs it with Java 11+.
#
#   tools/greenmail.sh fetch    download the jar and make the test TLS certificate
#   tools/greenmail.sh run      run in the foreground
#   tools/greenmail.sh start    run in the background (pid in tools/.cache/greenmail.pid)
#   tools/greenmail.sh stop     stop the background server
#
# Ports match tools/docker-compose.yml: IMAP 3143 (plain) / 3993 (TLS), SMTP 3025 / 3465 (TLS).
# Authentication is on. An account is created the first time mail is delivered to an address; its
# login and its password are both that address. The tests rely on this.
# Then: RAVENGLASS_IMAP_TEST=1 cargo test -p mail-imap
set -euo pipefail

VERSION="${GREENMAIL_VERSION:-2.1.15}"
SHA256_2_1_15="91d58798698cac357f015365240383b936ced2f2a1a9f7db37d9e3e1a31391b1"
CACHE="$(cd "$(dirname "$0")" && pwd)/.cache"
JAR="$CACHE/greenmail-standalone-$VERSION.jar"
KEYSTORE="$CACHE/greenmail-tls.p12"
CERT="$CACHE/greenmail-tls.pem"
PIDFILE="$CACHE/greenmail.pid"
LOG="$CACHE/greenmail.log"
STOREPASS="ravenglass-test"

fetch() {
    mkdir -p "$CACHE"
    if [ ! -f "$JAR" ]; then
        local url="https://repo1.maven.org/maven2/com/icegreen/greenmail-standalone/$VERSION/greenmail-standalone-$VERSION.jar"
        echo "fetching $url" >&2
        curl --fail --silent --show-error --location --output "$JAR.part" "$url"
        if [ "$VERSION" = "2.1.15" ]; then
            echo "$SHA256_2_1_15  $JAR.part" | sha256sum --check --quiet || { rm -f "$JAR.part"; echo "checksum mismatch" >&2; exit 1; }
        fi
        mv "$JAR.part" "$JAR"
    fi
    # A throwaway self-signed certificate for localhost, so the TLS tests can trust exactly it.
    if [ ! -f "$KEYSTORE" ] || [ ! -f "$CERT" ]; then
        rm -f "$KEYSTORE" "$CERT"
        keytool -genkeypair -alias greenmail -keyalg EC -groupname secp256r1 -validity 3650 -storetype PKCS12 \
            -keystore "$KEYSTORE" -storepass "$STOREPASS" -keypass "$STOREPASS" \
            -dname "CN=localhost" -ext "SAN=dns:localhost,ip:127.0.0.1" >/dev/null 2>&1
        keytool -exportcert -rfc -alias greenmail -keystore "$KEYSTORE" -storepass "$STOREPASS" -file "$CERT" >/dev/null 2>&1
    fi
}

run() {
    fetch
    exec java -Dgreenmail.setup.test.all -Dgreenmail.hostname=127.0.0.1 \
        -Dgreenmail.tls.keystore.file="$KEYSTORE" -Dgreenmail.tls.keystore.password="$STOREPASS" \
        ${GREENMAIL_OPTS:-} -jar "$JAR"
}

running() { [ -f "$PIDFILE" ] && kill -0 "$(cat "$PIDFILE")" 2>/dev/null; }

case "${1:-run}" in
    fetch) fetch ;;
    run) run ;;
    start)
        if running; then echo "already running (pid $(cat "$PIDFILE"))" >&2; exit 0; fi
        fetch
        nohup "$0" run >"$LOG" 2>&1 &
        echo $! >"$PIDFILE"
        echo "started (pid $!), log in $LOG" >&2
        ;;
    stop)
        if running; then kill "$(cat "$PIDFILE")"; fi
        rm -f "$PIDFILE"
        ;;
    *) echo "usage: $0 [fetch|run|start|stop]" >&2; exit 2 ;;
esac
