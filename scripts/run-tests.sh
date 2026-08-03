#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

cleanup() {
    docker compose down --volumes --remove-orphans
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

docker compose down --volumes --remove-orphans
docker compose build tests
docker compose up --detach --wait bitcoind
docker compose run --rm tests
