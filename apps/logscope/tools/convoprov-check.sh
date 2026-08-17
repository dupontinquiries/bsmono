#!/bin/sh
# convoprov-check.sh — model-provenance marker logic, without the egui tree.
set -e
D=$(cd "$(dirname "$0")/.." && pwd)/crates/logscope-egui/src
T=$(mktemp -d); cp "$D/convoprov.rs" "$T/t.rs"
rustc --test --edition 2021 -O "$T/t.rs" -o "$T/t"; "$T/t"; rm -rf "$T"
