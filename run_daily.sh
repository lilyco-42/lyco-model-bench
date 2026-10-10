#!/bin/sh
# 每日模型发现 + token 测速（Rust 版, Linux / Radxa）。
set -eu
DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
BIN="$DIR/target/release/lyco-model-bench"
# 复用 bot 的 provider keys（若有，绝不打印）。
if [ -f "$HOME/qq_bot/env.sh" ]; then
    set -a
    # shellcheck disable=SC1091
    . "$HOME/qq_bot/env.sh"
    set +a
fi
mkdir -p "$DIR/results"
# 首次部署（或清过 target）时先编译 release 二进制。
if [ ! -x "$BIN" ]; then
    ( cd "$DIR" && cargo build --release --locked )
fi
exec "$BIN" daily --provider both --reps 3 --max-tokens 512 --concurrency 6 --out "$DIR/results"
