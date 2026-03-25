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
# Build methods (in preference order):
#   1. Docker (messense/rust-musl-cross) — works on macOS without native toolchain
#   2. `cross` — if installed
#   3. `cargo` — requires native musl toolchain
#
# The release profile uses LTO + codegen-units=1 + strip for minimal size.
# UPX is applied if available (brew install upx) for ~75% further reduction.
#
# Output: target/agents/nexus-agent-linux-{arch}
# Also installs to: ~/.nexus/agents/nexus-agent-{target}

set -euo pipefail

TARGETS=(
    "x86_64-unknown-linux-musl:x86_64:x86_64-musl"
    "aarch64-unknown-linux-musl:aarch64:aarch64-musl"
    "armv7-unknown-linux-musleabihf:armv7:armv7-musleabihf"
)

OUT_DIR="target/agents"
FILTER="${1:-}"

# Determine build method
BUILD_METHOD=""
if command -v docker &>/dev/null; then
    BUILD_METHOD="docker"
elif command -v cross &>/dev/null; then
    BUILD_METHOD="cross"
else
    BUILD_METHOD="cargo"
    echo "Note: neither docker nor cross found, falling back to 'cargo build'."
fi

mkdir -p "$OUT_DIR"

built=0
for entry in "${TARGETS[@]}"; do
    IFS=: read -r target arch docker_image <<< "$entry"

    # Apply filter if provided
    if [[ -n "$FILTER" && "$arch" != "$FILTER" ]]; then
        continue
    fi

    echo "==> Building nexus-agent for $target ($arch) via $BUILD_METHOD..."
    if [[ "$BUILD_METHOD" == "docker" ]]; then
        docker run --rm -v "$(pwd)":/src -w /src \
            "messense/rust-musl-cross:${docker_image}" \
            cargo build --release --target "$target" -p nexus-agent
    else
        $BUILD_METHOD build --release --target "$target" -p nexus-agent
    fi

    src="target/$target/release/nexus-agent"
    dst="$OUT_DIR/nexus-agent-linux-$arch"

    if [[ ! -f "$src" ]]; then
        echo "ERROR: Expected binary not found at $src"
        exit 1
    fi

    cp "$src" "$dst"
    orig_size=$(du -h "$dst" | cut -f1)

    # Strip if possible
    strip_cmd="${target}-strip"
    if command -v "$strip_cmd" &>/dev/null; then
        "$strip_cmd" "$dst"
        echo "    Stripped with $strip_cmd"
    elif command -v strip &>/dev/null && [[ "$BUILD_METHOD" == "cross" ]]; then
        strip "$dst" 2>/dev/null || true
    fi

    stripped_size=$(du -h "$dst" | cut -f1)

    # UPX compress for smallest possible binary
    if command -v upx &>/dev/null; then
        upx --best --lzma "$dst" -o "${dst}.upx" --force -q 2>/dev/null
        mv "${dst}.upx" "$dst"
        echo "    UPX compressed"
    else
        echo "    Warning: 'upx' not found, skipping compression. Install: brew install upx"
    fi

    final_size=$(du -h "$dst" | cut -f1)
    echo "    Output: $dst ($orig_size → $stripped_size → $final_size)"

    # Install to ~/.nexus/agents/ for deploy
    install_dir="$HOME/.nexus/agents"
    install_name="nexus-agent-${target}"
    mkdir -p "$install_dir"
    cp "$dst" "$install_dir/$install_name"
    echo "    Installed: $install_dir/$install_name"
    built=$((built + 1))
done

if [[ $built -eq 0 ]]; then
    echo "No targets matched filter '$FILTER'. Valid: x86_64, aarch64, armv7"
    exit 1
fi

echo ""
echo "Done. Built $built target(s) in $OUT_DIR/"
