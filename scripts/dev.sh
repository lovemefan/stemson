#!/usr/bin/env bash
# Start the service with the local development configuration (port 8080).
set -euo pipefail

cd "$(dirname "$0")/.."
export STEMSON_CONFIG="${STEMSON_CONFIG:-config/config.dev.yaml}"
export RUST_LOG="${RUST_LOG:-debug}"

cargo run -- "$@"
