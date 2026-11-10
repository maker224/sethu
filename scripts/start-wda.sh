#!/usr/bin/env bash
# Build and run WebDriverAgent for a connected iPhone/iPad.
#
#   ./scripts/start-wda.sh <device-udid>
#
# Optional environment:
#   WDA_DIR              checkout location (default: ~/WebDriverAgent)
#   DEVELOPMENT_TEAM_ID  Apple team id used for signing (strongly recommended)
#
# After it is running, expose port 8100 to Sethu with iproxy or go-ios:
#   iproxy 8100 8100 <device-udid>
#   curl http://127.0.0.1:8100/status
set -euo pipefail

if [[ -z "${1:-}" || "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  sed -n '2,13p' "$0" | sed 's/^# \{0,1\}//'
  exit 1
fi

UDID="$1"
WDA_DIR="${WDA_DIR:-$HOME/WebDriverAgent}"
TEAM="${DEVELOPMENT_TEAM_ID:-}"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "WebDriverAgent needs macOS with Xcode." >&2
  exit 1
fi
if ! command -v xcodebuild >/dev/null 2>&1; then
  echo "xcodebuild not found. Install Xcode and select it with xcode-select." >&2
  exit 1
fi

if [[ ! -d "${WDA_DIR}/WebDriverAgent.xcodeproj" ]]; then
  echo "WebDriverAgent not found at ${WDA_DIR}; cloning appium/WebDriverAgent…"
  git clone --depth 1 https://github.com/appium/WebDriverAgent.git "${WDA_DIR}"
fi

cd "${WDA_DIR}"

ARGS=(
  -project WebDriverAgent.xcodeproj
  -scheme WebDriverAgentRunner
  -destination "id=${UDID}"
  test
)
if [[ -n "${TEAM}" ]]; then
  ARGS+=("DEVELOPMENT_TEAM=${TEAM}" "CODE_SIGN_IDENTITY=Apple Development")
else
  echo "Tip: set DEVELOPMENT_TEAM_ID to your Apple team id to avoid signing prompts." >&2
fi

echo
echo "Starting WebDriverAgent. Keep this terminal open."
echo "In another terminal: iproxy 8100 8100 ${UDID}"
echo "Verify:             curl http://127.0.0.1:8100/status"
echo
echo "iOS 17+ note: XCTest needs a userspace tunnel (go-ios --tunnel or"
echo "pymobiledevice3) in addition to the port forward."
echo

exec xcodebuild "${ARGS[@]}"
