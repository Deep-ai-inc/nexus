#!/bin/bash
# Coverage by file for all crates
# Usage: ./coverage-by-file.sh [crate-name]
#   No args: show all crates
#   With arg: show only that crate

set -e

CRATES="nexus-api nexus-kernel nexus-ui strata nexus-agent nexus-executor nexus-fs nexus-llm nexus-sandbox nexus-term nexus-web"

if [ -n "$1" ]; then
    CRATES="$1"
fi

for crate in $CRATES; do
    echo "=== $crate ==="
    cargo llvm-cov --lib -p "$crate" 2>&1 | grep -E "^([a-z_/]+\.rs|TOTAL|-{20})" | tail -100
    echo ""
done
