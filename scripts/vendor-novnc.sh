#!/usr/bin/env bash
# Vendor noVNC so the viewer works without cdn.jsdelivr.net.
#
#   ./scripts/vendor-novnc.sh [version]
#
# Files land in web/vendor/novnc and are served by Sethu from SETHU_VENDOR_DIR
# (default ./web/vendor). The vendor directory is intentionally gitignored.
set -euo pipefail

VERSION="${1:-1.7.0}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEST="${ROOT}/web/vendor/novnc"
TMP="$(mktemp -d)"
trap 'rm -rf "${TMP}"' EXIT

echo "Downloading @novnc/novnc@${VERSION}…"
curl -fsSL "https://registry.npmjs.org/@novnc/novnc/-/novnc-${VERSION}.tgz" \
  | tar -xz -C "${TMP}"

if [[ ! -f "${TMP}/package/core/rfb.js" ]]; then
  echo "Unexpected tarball layout; aborting." >&2
  exit 1
fi

rm -rf "${DEST}"
mkdir -p "${DEST}"
cp -R "${TMP}/package/core" "${DEST}/core"
cp -R "${TMP}/package/vendor" "${DEST}/vendor"
cp "${TMP}/package/LICENSE.txt" "${DEST}/LICENSE.txt" 2>/dev/null || true

echo "Vendored noVNC ${VERSION} into ${DEST}"
echo "Restart Sethu (the viewer prefers /vendor/novnc/core/rfb.js)."
