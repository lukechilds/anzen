#!/usr/bin/env sh
set -eu

tools_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
app_dir=$(dirname "$tools_dir")
venv_dir="$app_dir/.benchmark-venv"

if [ ! -x "$venv_dir/bin/python" ]; then
    python3 -m venv "$venv_dir"
fi

"$venv_dir/bin/python" -m pip install \
    --disable-pip-version-check \
    --quiet \
    --requirement "$tools_dir/requirements-benchmark.txt"

exec "$venv_dir/bin/python" "$tools_dir/signing-benchmark.py" "$@"
