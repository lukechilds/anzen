#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

usage() {
    cat <<'EOF'
Usage: ./scripts/run-e2e.sh [--list | all | TEST...]

Run one or more end-to-end tests serially. Every test gets a fresh Bitcoin Core
regtest chain and fresh vault state. With no arguments, all tests run.

Examples:
  ./scripts/run-e2e.sh --list
  ./scripts/run-e2e.sh monthly-spend
  ./scripts/run-e2e.sh lost-phone lost-hww
  ./scripts/run-e2e.sh all
EOF
}

if [[ ${1:-} == --help || ${1:-} == -h ]]; then
    usage
    exit 0
fi

if [[ ${1:-} == --list ]]; then
    scripts/e2e-demo.sh --list
    exit 0
fi

ALL_TESTS=()
while IFS= read -r test_name; do
    ALL_TESTS[${#ALL_TESTS[@]}]=$test_name
done < <(scripts/e2e-demo.sh --list)

if [[ $# -eq 0 || ${1:-} == all ]]; then
    if [[ $# -gt 1 ]]; then
        printf '`all` cannot be combined with individual test names.\n' >&2
        exit 2
    fi
    SELECTED_TESTS=("${ALL_TESTS[@]}")
else
    SELECTED_TESTS=("$@")
fi

for selected in "${SELECTED_TESTS[@]}"; do
    valid=false
    for known in "${ALL_TESTS[@]}"; do
        if [[ $selected == "$known" ]]; then
            valid=true
            break
        fi
    done
    if [[ $valid != true ]]; then
        printf 'Unknown test: %s\n\n' "$selected" >&2
        usage >&2
        printf '\nAvailable tests:\n' >&2
        scripts/e2e-demo.sh --list | sed 's/^/  /' >&2
        exit 2
    fi
done

cleanup() {
    COMPOSE_PROGRESS=quiet docker compose down --volumes --remove-orphans >/dev/null 2>&1 || true
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

cleanup
if [[ ${VAULT_SKIP_DOCKER_BUILD:-0} != 1 ]]; then
    printf 'Building the demo image once...\n'
    COMPOSE_PROGRESS=quiet docker compose build demo
fi

total=${#SELECTED_TESTS[@]}
index=0
for test_name in "${SELECTED_TESTS[@]}"; do
    index=$((index + 1))
    cleanup
    printf '\n🔧 Test %s/%s: %s (fresh regtest chain)\n' \
        "$index" "$total" "$test_name"
    COMPOSE_PROGRESS=quiet docker compose up --detach --wait bitcoind >/dev/null
    COMPOSE_PROGRESS=quiet docker compose run --rm --no-deps demo "$test_name"
done

printf '\n✨ All %s selected tests passed.\n' "$total"
