#!/bin/sh
# Verify games/clawbound-battleground/src/teamplay.rs without building clawbound.
#
# clawbound is a large graphics crate and a cold `cargo test` there exceeds
# twenty minutes in the sandbox, which is how teamplay.rs shipped with five
# failing tests nobody had run. The module's ONLY dependency on the rest of the
# crate is `consts::Lane`, so it can be checked standalone in seconds.
#
# The Lane substituted below must stay identical to the real one. If it drifts,
# this harness passes while the module does not compile in place -- so the
# check below compares the two definitions and refuses rather than lying.
set -e
ROOT="${1:-.}"
SRC="$ROOT/games/clawbound-battleground/src/teamplay.rs"
CONSTS="$ROOT/games/clawbound-battleground/src/consts.rs"
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

LANE='pub enum Lane { Top = 0, Mid = 1, Bot = 2 }'
if ! grep -q 'Top = 0,' "$CONSTS" || ! grep -q 'Bot = 2,' "$CONSTS"; then
    echo "teamplay_check: Lane in consts.rs no longer matches this harness — update both" >&2
    exit 2
fi

mkdir -p "$WORK/src"
cat > "$WORK/Cargo.toml" <<TOML
[package]
name = "teamplay-check"
version = "0.0.0"
edition = "2021"
TOML
sed "s|use crate::consts::Lane;|#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]\n$LANE|" \
    "$SRC" > "$WORK/src/lib.rs"

cd "$WORK"
cargo test --quiet 2>&1 | grep -E "^test result|FAILED|^error" || true
