#!/bin/sh
# diagfeed-check.sh — run the diagnostics-feed tests without building the egui tree.
#
# diagfeed.rs is pure text classification, so it compiles standalone in about a second where
# `cargo test -p logscope-egui` needs the whole GUI tree — a build that timed out at 400s in c4-801
# and hid every test in that crate for five bundles. Same pattern as physui-check.sh and
# kvtree-check.sh: if the fast path does not exist, the tests do not get run.
set -e
D=$(cd "$(dirname "$0")/.." && pwd)/crates/logscope-egui/src
T=$(mktemp -d)
cp "$D/diagfeed.rs" "$T/t.rs"
rustc --test --edition 2021 -O "$T/t.rs" -o "$T/t"
"$T/t"
rm -rf "$T"
