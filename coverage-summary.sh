#!/bin/bash
# Coverage summary for all crates (totals only)
# Usage: ./coverage-summary.sh

set -e

CRATES="nexus-api nexus-kernel nexus-ui strata nexus-agent nexus-executor nexus-fs nexus-llm nexus-sandbox nexus-term nexus-web"

printf "%-20s %8s %8s\n" "Crate" "Lines" "Cover"
printf "%-20s %8s %8s\n" "--------------------" "--------" "--------"

for crate in $CRATES; do
    result=$(cargo llvm-cov --lib -p "$crate" 2>&1 | grep "^TOTAL" | awk '{print $8, $10}')
    lines=$(echo "$result" | awk '{print $1}')
    cover=$(echo "$result" | awk '{print $2}')
    printf "%-20s %8s %8s\n" "$crate" "$lines" "$cover"
done
