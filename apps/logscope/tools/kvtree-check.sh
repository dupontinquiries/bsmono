#!/bin/sh
# kvtree-check.sh — run the KV prefix-tree decision tests WITHOUT building the egui dep tree.
#
# kvtree.rs is pure: token slices in, an anchor choice out. It compiles standalone in about a second,
# where `cargo test -p logscope-egui` needs the whole GUI tree and several GB of debug artefacts.
# Same reasoning as tools/physui-check.sh — the alternative to a fast runner is changes shipped with
# the tests unexecuted.
set -e
D=$(cd "$(dirname "$0")/.." && pwd)/crates/logscope-egui/src
T=$(mktemp -d)
cp "$D/kvtree.rs" "$T/t.rs"
rustc --test --edition 2021 -O "$T/t.rs" -o "$T/t"
"$T/t"
rm -rf "$T"
