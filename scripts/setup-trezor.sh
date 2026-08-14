#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repository_root="$(git -C "$script_dir/.." rev-parse --show-toplevel)"
firmware_dir="$repository_root/trezor-firmware"

printf 'Initializing Trezor firmware and its vendor submodules; the first run is large.\n'
git -C "$repository_root" submodule update --init trezor-firmware
git -C "$firmware_dir" submodule update --init --recursive

if ! git -C "$firmware_dir" remote get-url upstream >/dev/null 2>&1; then
    git -C "$firmware_dir" remote add upstream https://github.com/trezor/trezor-firmware.git
fi

printf 'Trezor firmware initialized at %s\n' "$(git -C "$firmware_dir" rev-parse HEAD)"
printf 'For development, run: git -C trezor-firmware switch anzen\n'
