#!/bin/bash
# Generate a test keypair and build the Docker sshd image.
set -euo pipefail
cd "$(dirname "$0")"

if [ ! -f test_key ]; then
    echo "Generating test SSH keypair..."
    ssh-keygen -t ed25519 -f test_key -N "" -C "nexus-test"
fi

echo "Building sshd container..."
docker build -t nexus-test-sshd -f Dockerfile.sshd .

echo ""
echo "Ready. Run:  docker compose up -d"
echo "Then:        cargo test --test osc_integration"
