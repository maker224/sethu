#!/usr/bin/env bash
# Turn on macOS Screen Sharing and prepare it for browser (VNC) clients.
#
#   ./scripts/enable-macos-screen-sharing.sh          # asks for confirmation
#   ./scripts/enable-macos-screen-sharing.sh --yes    # unattended
#
# It uses Apple's own `kickstart` tool, which needs sudo. It does NOT set or
# store a VNC password: set that in the UI (step printed at the end).
set -euo pipefail

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  sed -n '2,9p' "$0" | sed 's/^# \{0,1\}//'
  exit 0
fi

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "This script only runs on macOS." >&2
  exit 1
fi

if [[ "${EUID}" -ne 0 ]]; then
  echo "Screen Sharing is a system service; re-running with sudo…"
  exec sudo -- "$0" "$@"
fi

KICKSTART="/System/Library/CoreServices/RemoteManagement/ARDAgent.app/Contents/Resources/kickstart"
if [[ ! -x "${KICKSTART}" ]]; then
  echo "Could not find kickstart at ${KICKSTART}." >&2
  echo "Enable Screen Sharing manually: System Settings → General → Sharing → Screen Sharing." >&2
  exit 1
fi

if [[ "${1:-}" != "--yes" ]]; then
  echo "This will enable macOS Screen Sharing for all users, with remote control"
  echo "privileges, and restart the ARD agent. Continue? [y/N]"
  read -r answer
  if [[ ! "${answer}" =~ ^[Yy]$ ]]; then
    echo "Cancelled."
    exit 1
  fi
fi

echo "Enabling Screen Sharing…"
"${KICKSTART}" -activate -configure -access -on -restart -agent -privs -all

echo
echo "Done. Now do these two things:"
echo "  1. Choose how Sethu may sign in, in Screen Sharing (ⓘ) → Computer Settings…:"
echo "     • Recommended: allow your macOS user under “Allow access for” and sign in"
echo "       with your Mac account in the browser."
echo "     • Or enable “VNC viewers may control screen with password” and set a"
echo "       strong 8-character VNC password."
echo "  2. Allow Screen Sharing through the firewall (System Settings → Network → Firewall)."
echo
echo "Verify from the machine running Sethu with:"
echo "  nc -vz this-mac.local 5900"
echo

if [[ -n "${SUDO_USER:-}" ]]; then
  sudo -u "${SUDO_USER}" open "x-apple.systempreferences:com.apple.Sharing-Settings.extension" || true
fi
