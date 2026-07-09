#!/bin/sh
# Paperbound launcher.
#
# Creates a local venv, installs pygame-ce (NOT upstream pygame) + numpy, and
# runs the game. Using pygame-ce sidesteps the SDL_ttf / font-module crash that
# upstream `pygame` hits on Python 3.14.
#
# Usage:  ./run.sh            # normal
#         ./run.sh --shot     # headless screenshots
set -eu

HERE=$(CD=$(dirname "$0"); cd "$CD"; pwd)
cd "$HERE"

# Pick an interpreter. Prefer 3.11/3.12 (widest wheel coverage); fall back to
# whatever python3 is, since pygame-ce also has 3.13/3.14 wheels.
PY=""
for c in python3.12 python3.11 python3.13 python3; do
    if command -v "$c" >/dev/null 2>&1; then PY="$c"; break; fi
done
if [ -z "$PY" ]; then
    echo "No python3 found on PATH." >&2; exit 1
fi
echo "Using interpreter: $PY ($($PY -V 2>&1))"

VENV="$HERE/.venv"
if [ ! -d "$VENV" ]; then
    echo "Creating venv…"
    "$PY" -m venv "$VENV"
fi
# shellcheck disable=SC1091
. "$VENV/bin/activate"

# Make sure the ORIGINAL pygame isn't shadowing pygame-ce (they conflict).
python -c "import pygame, sys; sys.exit(0 if getattr(pygame,'IS_CE',0) else 1)" 2>/dev/null || {
    echo "Installing pygame-ce + numpy (removing any upstream pygame)…"
    pip -q install --upgrade pip
    pip -q uninstall -y pygame 2>/dev/null || true
    pip -q install -r "$HERE/requirements.txt"
}

exec python "$HERE/game.py" "$@"
