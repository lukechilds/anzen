#!/usr/bin/env bash
# Run the Flex build in Speculos inside Ledger's dev container, headless, with a fixed test seed.
# APDUs on port 9999, the automation API on 5001.
set -euo pipefail
cd "$(dirname "$0")/.."
pkill -x speculos 2>/dev/null || pkill -f 'bin/speculos' 2>/dev/null || true
sleep 1
exec speculos --model "${1:-flex}" --display headless --api-port 5001 --apdu-port 9999 \
    --seed "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about" \
    "target/${1:-flex}/release/anzen-ledger"
