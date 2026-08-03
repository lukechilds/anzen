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
if [[ ${VAULT_SKIP_DOCKER_BUILD:-0} != 1 ]]; then
    docker compose build tests
fi
docker compose up --detach --wait bitcoind
docker compose run --rm tests
