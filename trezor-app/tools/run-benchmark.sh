#!/bin/sh
set -eu

APP_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
REPO_DIR=$(CDPATH= cd -- "$APP_DIR/.." && pwd)

if [ -n "${PYTHON:-}" ]; then
    PYTHON_BIN=$PYTHON
elif command -v python3.12 >/dev/null 2>&1; then
    PYTHON_BIN=$(command -v python3.12)
else
    PYTHON_BIN=$(command -v python3)
fi

PYTHON_TAG=$($PYTHON_BIN -c 'import sys; print(f"{sys.version_info.major}.{sys.version_info.minor}")')
VENV_DIR="$APP_DIR/.venv-$PYTHON_TAG"

if [ ! -x "$VENV_DIR/bin/python" ]; then
    "$PYTHON_BIN" -m venv "$VENV_DIR"
    "$VENV_DIR/bin/python" -m pip install --quiet --upgrade pip
    "$VENV_DIR/bin/python" -m pip install --quiet -e "$REPO_DIR/trezor-firmware/python"
fi

exec "$VENV_DIR/bin/python" "$APP_DIR/tools/benchmark.py" "$@"
