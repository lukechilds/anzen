#!/usr/bin/env bash
# Build the Anzen app for Trezor.
#
#   tools/build.sh            both images: the emulator's and the Safe 7's
#   tools/build.sh emulator   the emulator's only
#   tools/build.sh device     the Safe 7's only
#
# Needs the firmware checkout's Nix shell and Python environment (see README.md). The images, and
# what a Trezor needs to accept them, land in target/artifacts/serialized.
set -euo pipefail

here=$(cd "$(dirname "$0")/.." && pwd)
firmware=$here/../trezor-firmware

build() {
    (cd "$firmware" && nix-shell --run "source .venv/bin/activate && cd '$here' && cargo app-tool build -m t3w1 $*")
}

case ${1:-both} in
    emulator) build -e ;;
    device) build ;;
    both) build -e && build ;;
    *) echo "usage: tools/build.sh [emulator|device]" >&2; exit 1 ;;
esac
