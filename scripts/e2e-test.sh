#!/usr/bin/env bash
# End-to-end check against a running server and a real MySQL.
#
#   STEMSON_URL=http://127.0.0.1:19090 scripts/e2e-test.sh
#
# Requires the schema from mysql/create_database.sql (seed users admin/admin123
# and lovemefan/lovemefan123).
set -euo pipefail

BASE_URL="${STEMSON_URL:-http://127.0.0.1:8080}"
ADMIN_USER="${ADMIN_USER:-admin}"
ADMIN_PASSWORD="${ADMIN_PASSWORD:-admin123}"
NEW_USER="test_$(date +%s)"
NEW_PASSWORD="secret123"

pass=0
fail=0

json_field() {
  python3 -c 'import json,sys
data = json.load(sys.stdin)
for key in sys.argv[1].split("."):
    data = data.get(key) if isinstance(data, dict) else None
print("" if data is None else data)' "$1"
}

check() {
  local label="$1" expected="$2" actual="$3"
  if [[ "$expected" == "$actual" ]]; then
    printf '  ok   %-52s %s\n' "$label" "$actual"
    pass=$((pass + 1))
  else
    printf '  FAIL %-52s expected=%s actual=%s\n' "$label" "$expected" "$actual"
    fail=$((fail + 1))
  fi
}

request() {
  local method="$1" path="$2" body="${3:-}" token="${4:-}"
  local args=(-sS -m 20 -X "$method" "${BASE_URL}${path}" -H 'Content-Type: application/json')
  [[ -n "$token" ]] && args+=(-H "Authorization: Bearer ${token}")
  [[ -n "$body" ]] && args+=(-d "$body")
  curl "${args[@]}"
}

echo "== health =="
check "GET /health code" "0000" "$(request GET /health | json_field code)"

echo "== authentication =="
login_body="{\"username\":\"${ADMIN_USER}\",\"password\":\"${ADMIN_PASSWORD}\"}"
login_response="$(request POST /v1/api/auth "$login_body")"
check "POST /v1/api/auth code" "0000" "$(json_field code <<<"$login_response")"
TOKEN="$(json_field data.access_token <<<"$login_response")"
[[ -n "$TOKEN" ]] || { echo "no token, aborting"; exit 1; }

check "GET /v1/api/auth/verify code" "0000" "$(request GET /v1/api/auth/verify "" "$TOKEN" | json_field code)"
check "GET /v1/api/auth/me username" "$ADMIN_USER" "$(request GET /v1/api/auth/me "" "$TOKEN" | json_field data.me.username)"
check "GET /v1/api/auth/me hides password" "" "$(request GET /v1/api/auth/me "" "$TOKEN" | json_field data.me.password)"

check "wrong password -> 0006" "0006" \
  "$(request POST /v1/api/auth "{\"username\":\"${ADMIN_USER}\",\"password\":\"nope\"}" | json_field code)"
check "unknown user -> 0013" "0013" \
  "$(request POST /v1/api/auth '{"username":"nobody","password":"whatever"}' | json_field code)"
check "missing password -> 0010" "0010" \
  "$(request POST /v1/api/auth "{\"username\":\"${ADMIN_USER}\"}" | json_field code)"
check "no token -> 0014" "0014" \
  "$(request POST /user/user_controller/add_user '{}' | json_field code)"

echo "== validation =="
check "short password -> 0015" "0015" \
  "$(request POST /user/user_controller/validate_password "{\"username\":\"${ADMIN_USER}\",\"password\":\"123\"}" | json_field code)"
check "valid password -> 0000" "0000" \
  "$(request POST /user/user_controller/validate_password "{\"username\":\"${ADMIN_USER}\",\"password\":\"${ADMIN_PASSWORD}\"}" | json_field code)"
check "bad email -> 0015" "0015" \
  "$(request POST /user/user_controller/modify_user '{"user_id":1,"username":"x","email":"nope","password":"secret123","user_role":0,"status":1}' "$TOKEN" | json_field code)"

echo "== user management =="
create_body="{\"username\":\"${NEW_USER}\",\"password\":\"${NEW_PASSWORD}\",\"role\":0}"
created="$(request POST /user/user_controller/add_user "$create_body" "$TOKEN")"
check "add user code" "0000" "$(json_field code <<<"$created")"
NEW_UID="$(json_field data.user_id <<<"$created")"
check "new user is readable" "${NEW_USER}" \
  "$(request POST /user/user_controller/get_user_info "{\"username\":\"${NEW_USER}\"}" "$TOKEN" | json_field data.username)"

check "duplicate username -> 0017" "0017" \
  "$(request POST /user/user_controller/add_user "$create_body" "$TOKEN" | json_field code)"

new_login="$(request POST /v1/api/auth "{\"username\":\"${NEW_USER}\",\"password\":\"${NEW_PASSWORD}\"}")"
NEW_TOKEN="$(json_field data.access_token <<<"$new_login")"
check "new user can log in" "0000" "$(json_field code <<<"$new_login")"
check "normal user cannot list users -> 0008" "0008" \
  "$(request POST /getall '{}' "$NEW_TOKEN" | json_field code)"

list="$(request POST /getall '{}' "$TOKEN")"
check "admin lists users" "0000" "$(json_field code <<<"$list")"
check "admin list contains the new user" "1" \
  "$(python3 -c 'import json,sys
data = json.load(sys.stdin)
print(1 if any(u["username"] == sys.argv[1] for u in data["data"]) else 0)' "$NEW_USER" <<<"$list")"

check "modify user" "0000" \
  "$(request POST /user/user_controller/modify_user "{\"user_id\":${NEW_UID},\"username\":\"${NEW_USER}\",\"email\":\"${NEW_USER}@example.com\",\"password\":\"${NEW_PASSWORD}\",\"user_role\":0,\"status\":1}" "$TOKEN" | json_field code)"
check "modify email persisted" "${NEW_USER}@example.com" \
  "$(request POST /user/user_controller/get_user_info "{\"user_id\":${NEW_UID}}" "$TOKEN" | json_field data.email)"

check "delete user" "0000" \
  "$(request POST /user/user_controller/delete_user "{\"user_id\":${NEW_UID}}" "$TOKEN" | json_field code)"
check "deleted user is gone" "0013" \
  "$(request POST /user/user_controller/get_user_info "{\"user_id\":${NEW_UID}}" "$TOKEN" | json_field code)"

echo
echo "passed=${pass} failed=${fail}"
[[ "$fail" -eq 0 ]]
