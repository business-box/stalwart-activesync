#!/usr/bin/env bash
# End-to-end test against a real Stalwart: starts a throwaway Stalwart
# container, creates a domain and a user, runs the gateway binary against it
# and plays a phone with `probe --full` (sync, send to self with push,
# contact and event writes).
#
#   tests/integration/run.sh [path/to/stalwart-activesync]
#
# Needs docker, python3 and curl. STALWART_IMAGE overrides the image.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
BIN="${1:-$ROOT/target/debug/stalwart-activesync}"
IMAGE="${STALWART_IMAGE:-stalwartlabs/stalwart:v0.16.27}"
NAME="eas-it-stalwart-$$"
STALWART_PORT="${STALWART_PORT:-18080}"
GATEWAY_PORT="${GATEWAY_PORT:-18081}"
ADMIN_PASSWORD="it-admin-$$"
USER_PASSWORD="it-alice-pass"
WORK="$(mktemp -d)"
GW_PID=""

cleanup() {
    status=$?
    [ -n "$GW_PID" ] && kill "$GW_PID" 2>/dev/null || true
    if [ $status -ne 0 ]; then
        echo "--- gateway log (last 80 lines)"; tail -n 80 "$WORK/gateway.log" 2>/dev/null || true
        echo "--- stalwart log (last 40 lines)"; docker logs --tail 40 "$NAME" 2>&1 || true
    fi
    docker rm -f "$NAME" >/dev/null 2>&1 || true
    docker volume rm -f "$NAME-etc" >/dev/null 2>&1 || true
    rm -rf "$WORK"
    exit $status
}
trap cleanup EXIT

wait_for() {
    local what="$1" url="$2"
    for _ in $(seq 1 60); do
        if curl -fs -o /dev/null "$url"; then return 0; fi
        sleep 1
    done
    echo "$what did not come up at $url" >&2
    return 1
}

[ -x "$BIN" ] || { echo "gateway binary not found: $BIN (cargo build first)" >&2; exit 1; }

echo "=== Stalwart ($IMAGE)"
docker volume create "$NAME-etc" >/dev/null
docker run --rm -v "$NAME-etc:/etc/stalwart" busybox:1.36 sh -c \
    'echo "{\"@type\":\"RocksDb\",\"path\":\"/var/lib/stalwart/db\"}" > /etc/stalwart/config.json && chown -R 2000:2000 /etc/stalwart'
start_stalwart() {
    docker run -d --name "$NAME" \
        -e STALWART_HOSTNAME=mail.example.test \
        -e STALWART_PUBLIC_URL=https://mail.example.test \
        -e "STALWART_RECOVERY_ADMIN=admin:$ADMIN_PASSWORD" \
        -v "$NAME-etc:/etc/stalwart" \
        -p "127.0.0.1:$STALWART_PORT:8080" \
        "$IMAGE" >/dev/null
    wait_for Stalwart "http://127.0.0.1:$STALWART_PORT/healthz/live"
}
start_stalwart

MAIL_HOST=mail.example.test python3 "$ROOT/deploy/bootstrap.py" \
    --url "http://127.0.0.1:$STALWART_PORT" \
    --admin-password "$ADMIN_PASSWORD" \
    --domain example.test \
    --users "alice:$USER_PASSWORD"
# HTTP settings apply on restart; the data volume is the container's own.
docker restart "$NAME" >/dev/null
wait_for Stalwart "http://127.0.0.1:$STALWART_PORT/healthz/live"

echo "=== gateway"
STALWART_URL="http://127.0.0.1:$STALWART_PORT" \
EAS_PUBLIC_URL=https://mail.example.test \
EAS_LISTEN="127.0.0.1:$GATEWAY_PORT" \
EAS_DB_URL="sqlite://$WORK/eas.db" \
EAS_ADMIN_TOKEN=it-token \
RUST_LOG=info,stalwart_activesync=debug \
    "$BIN" >"$WORK/gateway.log" 2>&1 &
GW_PID=$!
wait_for gateway "http://127.0.0.1:$GATEWAY_PORT/healthz"

echo "=== probe"
"$BIN" probe "http://127.0.0.1:$GATEWAY_PORT" alice@example.test "$USER_PASSWORD" --full

echo "=== admin API"
curl -fs -H "Authorization: Bearer it-token" "http://127.0.0.1:$GATEWAY_PORT/admin/devices" | grep -q EasProbe01
echo "device listed"
echo "integration test passed"
