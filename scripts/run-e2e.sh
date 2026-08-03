#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

usage() {
    cat <<'EOF'
Usage: ./scripts/run-e2e.sh [--list | all | FLOW...]

Run one or more user flows serially. Every flow gets a fresh Bitcoin Core
regtest chain and fresh vault state. With no arguments, all flows run.

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

ALL_FLOWS=()
while IFS= read -r flow; do
    ALL_FLOWS[${#ALL_FLOWS[@]}]=$flow
done < <(scripts/e2e-demo.sh --list)

if [[ $# -eq 0 || ${1:-} == all ]]; then
    if [[ $# -gt 1 ]]; then
        printf '`all` cannot be combined with individual flow names.\n' >&2
        exit 2
    fi
    SELECTED_FLOWS=("${ALL_FLOWS[@]}")
else
    SELECTED_FLOWS=("$@")
fi

for selected in "${SELECTED_FLOWS[@]}"; do
    valid=false
    for known in "${ALL_FLOWS[@]}"; do
        if [[ $selected == "$known" ]]; then
            valid=true
            break
        fi
    done
    if [[ $valid != true ]]; then
        printf 'Unknown flow: %s\n\n' "$selected" >&2
        usage >&2
        printf '\nAvailable flows:\n' >&2
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

total=${#SELECTED_FLOWS[@]}
index=0
for flow in "${SELECTED_FLOWS[@]}"; do
    index=$((index + 1))
    cleanup
    printf '\n############################################################\n'
    printf 'Flow %s/%s: %s (fresh regtest chain)\n' "$index" "$total" "$flow"
    printf '############################################################\n'
    COMPOSE_PROGRESS=quiet docker compose up --detach --wait bitcoind >/dev/null
    COMPOSE_PROGRESS=quiet docker compose run --rm --no-deps demo "$flow"
done

printf '\nAll %s selected flow(s) passed.\n' "$total"
