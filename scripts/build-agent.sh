#!/usr/bin/env bash
#
# Cross-compile nexus-agent for Linux musl targets.
#
# Usage:
#   ./scripts/build-agent.sh              # build all targets
#   ./scripts/build-agent.sh x86_64       # build only x86_64
#   ./scripts/build-agent.sh aarch64      # build only aarch64
#   ./scripts/build-agent.sh armv7        # build only armv7
#
# Requires `cross` (https://github.com/cross-rs/cross) or appropriate
# Rust toolchain targets installed for `cargo build`.
#
# Output: target/agents/nexus-agent-linux-{arch}

set -euo pipefail

TARGETS=(
    "x86_64-unknown-linux-musl:x86_64"
    "aarch64-unknown-linux-musl:aarch64"
    "armv7-unknown-linux-musleabihf:armv7"
)

OUT_DIR="target/agents"
FILTER="${1:-}"

# Determine build tool: prefer `cross` if available
if command -v cross &>/dev/null; then
    BUILD_CMD="cross"
else
    BUILD_CMD="cargo"
    echo "Note: 'cross' not found, falling back to 'cargo build'."
    echo "Install cross for easier cross-compilation: cargo install cross"
fi

mkdir -p "$OUT_DIR"

built=0
for entry in "${TARGETS[@]}"; do
    target="${entry%%:*}"
    arch="${entry##*:}"

    # Apply filter if provided
    if [[ -n "$FILTER" && "$arch" != "$FILTER" ]]; then
        continue
    fi

    echo "==> Building nexus-agent for $target ($arch)..."
    $BUILD_CMD build --release --target "$target" -p nexus-agent

    src="target/$target/release/nexus-agent"
    dst="$OUT_DIR/nexus-agent-linux-$arch"

    if [[ ! -f "$src" ]]; then
        echo "ERROR: Expected binary not found at $src"
        exit 1
    fi

    cp "$src" "$dst"

    # Strip if possible
    strip_cmd="${target}-strip"
    if command -v "$strip_cmd" &>/dev/null; then
        "$strip_cmd" "$dst"
        echo "    Stripped with $strip_cmd"
    elif command -v strip &>/dev/null && [[ "$BUILD_CMD" == "cross" ]]; then
        # cross may have built with compatible strip
        strip "$dst" 2>/dev/null || true
    fi

    size=$(du -h "$dst" | cut -f1)
    echo "    Output: $dst ($size)"
    built=$((built + 1))
done

if [[ $built -eq 0 ]]; then
    echo "No targets matched filter '$FILTER'. Valid: x86_64, aarch64, armv7"
    exit 1
fi

echo ""
echo "Done. Built $built target(s) in $OUT_DIR/"
