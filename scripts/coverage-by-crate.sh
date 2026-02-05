#!/bin/bash
# Coverage report by crate
# Usage: ./scripts/coverage-by-crate.sh
#
# Requires: cargo-llvm-cov (install with: cargo install cargo-llvm-cov)

echo "=== Coverage by Crate ==="
echo ""
echo "Running workspace tests with coverage instrumentation..."
echo ""

# Run coverage once and capture output
OUTPUT=$(cargo llvm-cov test --workspace --lib --summary-only 2>&1)

# Check if it succeeded
if ! echo "$OUTPUT" | grep -q "^TOTAL"; then
    echo "Coverage run failed. Output:"
    echo "$OUTPUT" | tail -20
    exit 1
fi

printf "%-18s %10s %10s %10s\n" "Crate" "Lines" "Covered" "Percent"
printf "%-18s %10s %10s %10s\n" "------------------" "----------" "----------" "----------"

# Parse per-file coverage and aggregate by crate
echo "$OUTPUT" | awk '
/^[a-z].*\/.*\.rs/ {
    # Extract crate name from path (first directory component)
    split($1, parts, "/")
    crate = parts[1]

    # The columns are: filename regions missed% funcs missed% lines missed cover%
    # Lines data is at positions NF-5 (total), NF-4 (missed), NF-2 (cover%)
    lines = $(NF-5)
    missed = $(NF-4)

    # Remove commas from numbers
    gsub(",", "", lines)
    gsub(",", "", missed)

    if (lines ~ /^[0-9]+$/ && missed ~ /^[0-9]+$/) {
        total_lines[crate] += lines
        missed_lines[crate] += missed
    }
}
END {
    for (crate in total_lines) {
        if (total_lines[crate] > 0) {
            covered = total_lines[crate] - missed_lines[crate]
            pct = (covered / total_lines[crate]) * 100
            printf "%-18s %10d %10d %9.2f%%\n", crate, total_lines[crate], covered, pct
        }
    }
}'

echo ""
echo "=== Workspace Total ==="
echo "$OUTPUT" | grep "^TOTAL" | awk '{
    lines = $(NF-5)
    missed = $(NF-4)
    covered = lines - missed
    if (lines > 0) {
        pct = (covered / lines) * 100
        printf "Lines: %d, Covered: %d, Coverage: %.2f%%\n", lines, covered, pct
    }
}'
