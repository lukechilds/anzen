#!/usr/bin/env bash
# Enforce the crate boundaries from the resolved dependency graph rather than from source text.
set -euo pipefail

cd "$(dirname "$0")/.."

EMBEDDED_TARGET=thumbv7em-none-eabihf
failed=0

# Normal (non-dev, non-build) dependencies of a package, one crate name per line.
dependencies() {
    cargo tree --locked -p "$1" -e normal --prefix none --format '{p}' "${@:2}" \
        | awk '{print $1}' | sort -u
}

fail() {
    printf 'boundary violation: %s\n' "$1" >&2
    failed=1
}

# The protocol and signer must build without std for a bare-metal hardware-wallet target.
echo "== no_std build for $EMBEDDED_TARGET =="
cargo build --locked -p anzen-protocol -p anzen-signer --target "$EMBEDDED_TARGET"

# The signer's complete dependency tree is small enough to audit, so allowlist it exactly.
echo "== anzen-signer dependency allowlist =="
allowed='anzen-protocol anzen-signer bech32 proc-macro2 quote serde serde_core serde_derive syn unicode-ident'
for crate in $(dependencies anzen-signer --target "$EMBEDDED_TARGET"); do
    [[ " $allowed " == *" $crate "* ]] || fail "anzen-signer depends on $crate"
done

# Host crates that must never reach wallet, chain or network code.
deny() {
    local package=$1
    shift
    echo "== $package denylist =="
    local tree
    tree=$(dependencies "$package")
    for pattern in "$@"; do
        while read -r crate; do
            [[ -n $crate ]] && fail "$package depends on $crate"
        done < <(grep -E "^($pattern)\$" <<<"$tree" || true)
    done
}

network='bdk_.*|electrum-client|esplora-client|bitcoincore-rpc|jsonrpc|reqwest|minreq|ureq|tokio|hyper|axum|rusqlite|tungstenite'
deny anzen-protocol 'std|bitcoin|miniscript|anzen-.*-app|anzen-(common|storage|wallet|chain)' "$network"
deny anzen-signer-app 'anzen-wallet|anzen-chain' "$network"
deny anzen-common 'anzen-wallet|anzen-chain|anzen-signer|anzen-signer-app' "$network"
deny anzen-wallet 'anzen-signer|anzen-signer-app'
deny anzen-chain 'anzen-wallet|anzen-signer|anzen-signer-app'
# The playground is a public-data host: it must not be able to act as a signer.
deny anzen-playground 'anzen-signer|anzen-signer-app'

if [[ $failed -ne 0 ]]; then
    exit 1
fi
echo "All crate boundaries hold."
