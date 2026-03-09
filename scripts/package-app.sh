#!/bin/bash
# Build and package Nexus as a macOS .app bundle.
#
# Usage:
#   ./scripts/package-app.sh [--release]
#
# Output: target/Nexus.app/

set -euo pipefail

PROFILE="debug"
if [[ "${1:-}" == "--release" ]]; then
    PROFILE="release"
    cargo build --release -p nexus-ui
else
    cargo build -p nexus-ui
fi

BINARY="target/${PROFILE}/nexus"
APP_DIR="target/Nexus.app"
CONTENTS="${APP_DIR}/Contents"

rm -rf "${APP_DIR}"
mkdir -p "${CONTENTS}/MacOS"
mkdir -p "${CONTENTS}/Resources"

# Copy binary
cp "${BINARY}" "${CONTENTS}/MacOS/nexus"

# Copy bundle resources
cp nexus-ui/assets/Info.plist "${CONTENTS}/Info.plist"
cp nexus-ui/assets/Nexus.sdef "${CONTENTS}/Resources/Nexus.sdef"

echo "Packaged: ${APP_DIR}"
echo ""
echo "Run with:  open ${APP_DIR}"
echo "Or:        ${CONTENTS}/MacOS/nexus"
