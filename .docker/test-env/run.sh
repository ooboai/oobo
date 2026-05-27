#!/usr/bin/env bash
# Spin up an isolated test environment with SSH access.
# Usage: ./run.sh
#
# Then: ssh dev@localhost -p 2222 (password: dev)
#
# Inside the container, run the full install flow:
#   curl -fsSL https://oobo.ai/install.sh | bash
#
# To stop: docker stop oobo-test && docker rm oobo-test

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

echo "Building test environment..."
docker build -t oobo-test-env "$SCRIPT_DIR"

echo "Starting container..."
docker run -d \
  --name oobo-test \
  -p 2222:22 \
  oobo-test-env

echo ""
echo "Ready! Connect with:"
echo ""
echo "  ssh dev@localhost -p 2222"
echo "  password: dev"
echo ""
echo "Then install oobo:"
echo ""
echo "  curl -fsSL https://oobo.ai/install.sh | bash"
echo ""
echo "To stop:"
echo "  docker stop oobo-test && docker rm oobo-test"
