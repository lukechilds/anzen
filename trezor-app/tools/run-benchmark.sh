#!/bin/sh
set -eu

APP_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
REPO_DIR=$(CDPATH= cd -- "$APP_DIR/.." && pwd)
VENV_DIR="$APP_DIR/.venv"

if [ ! -x "$VENV_DIR/bin/python" ]; then
    python3 -m venv "$VENV_DIR"
    "$VENV_DIR/bin/python" -m pip install --quiet --upgrade pip
    "$VENV_DIR/bin/python" -m pip install --quiet -e "$REPO_DIR/trezor-firmware/python"
fi

exec "$VENV_DIR/bin/python" "$APP_DIR/tools/benchmark.py" "$@"
