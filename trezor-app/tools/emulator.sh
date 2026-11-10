#!/usr/bin/env bash
# Run the Trezor emulator for the Anzen app: headless, on its own port, with a wallet loaded.
#
#   tools/emulator.sh [port]      default port 31324; stop with ctrl-c
#
# Build the firmware first (see README.md). Every start is a fresh device: the emulated Safe 7's
# secure element keeps nothing between runs, so its flash cannot be kept either. The wallet is
# Trezor's public test phrase (twelve times "all"); set ANZEN_TREZOR_PHRASE for another.
set -euo pipefail

port=${1:-31324}
here=$(cd "$(dirname "$0")" && pwd)
cd "$here/../../trezor-firmware"

log=$(mktemp -t anzen-trezor-emulator)
nix-shell --run "source .venv/bin/activate && exec ./core/emu.py -h -e -P $port -p anzen" >"$log" 2>&1 &
emulator=$!
# Stopping the emulator's own process lets emu.py shut the rest down cleanly.
trap 'kill "$(cat ~/.trezoremu/anzen/trezor.pid 2>/dev/null)" 2>/dev/null; wait $emulator 2>/dev/null' EXIT INT TERM

until grep -q "Emulator ready" "$log"; do
    if ! kill -0 $emulator 2>/dev/null; then
        cat "$log" >&2
        echo "The emulator did not start." >&2
        exit 1
    fi
    sleep 0.5
done

nix-shell --run "source .venv/bin/activate && TREZOR_PATH=udp:127.0.0.1:$port python '$here/seed-emulator.py' ${ANZEN_TREZOR_PHRASE:+\"$ANZEN_TREZOR_PHRASE\"}" 2>/dev/null
echo "Trezor emulator ready on port $port (log: $log)."
wait $emulator
