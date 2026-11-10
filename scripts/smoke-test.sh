#!/usr/bin/env bash
# Start a throwaway Sethu instance and exercise the main HTTP flows.
# No devices, database or network access required.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PORT="${PORT:-18080}"
BASE="http://127.0.0.1:${PORT}"
DATA="$(mktemp -d)"
LOG="$(mktemp)"
COOKIE="$(mktemp)"
BIN="${BIN:-${ROOT}/target/release/sethu}"

if [[ ! -x "${BIN}" ]]; then
  echo "Building release binary…"
  (cd "${ROOT}" && cargo build --release)
fi

cleanup() {
  kill "${PID:-0}" 2>/dev/null || true
  wait "${PID:-0}" 2>/dev/null || true
  rm -rf "${DATA}" "${LOG}" "${COOKIE}"
}
trap cleanup EXIT

echo "Starting Sethu on ${BASE}…"
SETHU_BIND="127.0.0.1:${PORT}" \
SETHU_DATA_DIR="${DATA}" \
SETHU_ALLOW_REGISTRATION=true \
SETHU_PUBLIC_URL="${BASE}" \
  "${BIN}" >"${LOG}" 2>&1 &
PID=$!

for _ in $(seq 1 60); do
  if curl -fsS "${BASE}/api/health" >/dev/null 2>&1; then break; fi
  sleep 0.1
done

fail() {
  echo "FAIL: $1" >&2
  echo "--- server log ---" >&2
  tail -40 "${LOG}" >&2 || true
  exit 1
}

curl -fsS "${BASE}/api/health" | grep -q '"status":"ok"' || fail "health endpoint"

echo "Checking security headers…"
curl -fsSI "${BASE}/" | grep -qi 'content-security-policy' || fail "missing CSP"
curl -fsSI "${BASE}/" | grep -qi 'x-frame-options: DENY' || fail "missing frame denial"

echo "Registering an account…"
curl -fsS -c "${COOKIE}" \
  -H 'content-type: application/json' \
  -H "origin: ${BASE}" \
  -d '{"email":"smoke@example.com","password":"smoke-test-password"}' \
  "${BASE}/api/auth/register" | grep -q 'smoke@example.com' || fail "registration"

CSRF="$(curl -fsS -b "${COOKIE}" "${BASE}/api/session" \
  | sed -n 's/.*"csrf":"\([^"]*\)".*/\1/p')"
[[ -n "${CSRF}" ]] || fail "missing CSRF token"

echo "Checking CSRF enforcement…"
STATUS="$(curl -s -o /dev/null -w '%{http_code}' -b "${COOKIE}" \
  -H "origin: ${BASE}" -H 'content-type: application/json' \
  -d '{"name":"Mac","kind":"macos","target":"mac.local"}' "${BASE}/api/hosts")"
[[ "${STATUS}" == "403" ]] || fail "CSRF missing token should be 403 (got ${STATUS})"

echo "Adding a device…"
curl -fsS -b "${COOKIE}" \
  -H "origin: ${BASE}" \
  -H "x-sethu-csrf: ${CSRF}" \
  -H 'content-type: application/json' \
  -d '{"name":"Studio Mac","kind":"macos","target":"mac.local"}' \
  "${BASE}/api/hosts" | grep -q 'mac.local:5900' || fail "normalised host target"

curl -fsS -b "${COOKIE}" "${BASE}/api/hosts" | grep -q 'Studio Mac' || fail "host list"

echo "Checking cross-origin rejection…"
STATUS="$(curl -s -o /dev/null -w '%{http_code}' -b "${COOKIE}" \
  -H 'origin: https://evil.example' "${BASE}/api/hosts")"
[[ "${STATUS}" == "403" ]] || fail "cross-origin request should be 403 (got ${STATUS})"

echo "Checking the viewer shell…"
curl -fsS "${BASE}/viewer.html" | grep -q 'Sethu' || fail "viewer page"

echo
echo "Smoke test passed."
