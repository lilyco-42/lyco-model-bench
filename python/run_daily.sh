#!/bin/sh
# Daily model discovery + token-speed benchmark (Linux / Radxa).
set -eu
DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
# Pick up provider keys from the bot env if present (never printed).
if [ -f "$HOME/qq_bot/env.sh" ]; then
    set -a
    # shellcheck disable=SC1091
    . "$HOME/qq_bot/env.sh"
    set +a
fi
mkdir -p "$DIR/results"
exec python3 "$DIR/bench.py" --provider both --reps 3 --max-tokens 512 --concurrency 6
