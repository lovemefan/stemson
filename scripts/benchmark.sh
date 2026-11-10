#!/usr/bin/env bash
# Load-test one sanicboot/stemson-compatible server with the in-tree client.
#
#   scripts/benchmark.sh http://127.0.0.1:8080 [concurrency] [duration]
#
# Optional:
#   TOKEN        bearer token; when set, the authenticated `/getall` scenario runs
#   LOGIN_BODY   JSON body for the login scenario, e.g. '{"username":"admin","password":"admin123"}'
#
# Scenarios:
#   404       route miss + unified error body (no database)
#   validate  parameter validation failure (no database)
#   getall    LEFT JOIN + JSON serialization (requires TOKEN)
#   login     authentication (requires LOGIN_BODY)
set -euo pipefail

BASE="${1:-http://127.0.0.1:8080}"
CONCURRENCY="${2:-32}"
DURATION="${3:-10}"

cd "$(dirname "$0")/.."
cargo build --release --example loadgen >/dev/null
LG=("./target/release/examples/loadgen")

run() {
  local label="$1"; shift
  printf '%-10s ' "$label"
  "${LG[@]}" "$@" --concurrency "$CONCURRENCY" --duration "$DURATION" --warmup 3
}

run 404 --url "$BASE/nope"
run validate --url "$BASE/user/user_controller/validate_password" \
  --method POST --body '{"username":"x"}' --header 'Content-Type:application/json'

if [[ -n "${TOKEN:-}" ]]; then
  run getall --url "$BASE/getall" --method POST --body '{}' \
    --header 'Content-Type:application/json' \
    --header "Authorization:Bearer ${TOKEN}"
else
  echo "getall     skipped (set TOKEN=<bearer token> to run it)"
fi

if [[ -n "${LOGIN_BODY:-}" ]]; then
  run login --url "$BASE/v1/api/auth" --method POST --body "$LOGIN_BODY" \
    --header 'Content-Type:application/json'
else
  echo "login      skipped (set LOGIN_BODY='{\"username\":\"...\",\"password\":\"...\"}')"
fi
