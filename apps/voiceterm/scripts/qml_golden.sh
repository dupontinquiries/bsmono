#!/usr/bin/env bash
# v0.1.730: golden-output diff harness for the HTML→QML compiler.
#
# Compiles a fixed corpus of HTML fixtures through generate_safe_qml and
# compares the emitted QML against committed golden snapshots, BYTE-FOR-BYTE.
# This is the safety net required before any refactor of the qml_gen output
# path (e.g. the parked Vec<String>→&mut String accumulation change): run it
# before, refactor, run it after — any diff is a behavior change.
#
# Usage:
#   scripts/qml_golden.sh           # diff against goldens (fail on any diff)
#   scripts/qml_golden.sh --update  # (re)write the golden snapshots
#
# Stages qml_gen + ad_filter + table_ops (the full crate-dep closure) into a
# throwaway crate, so it runs in the sandbox with no GUI / no qmetaobject.

set -euo pipefail
cd "$(dirname "$0")/.."
ROOT="$(pwd)"
FIXT="$ROOT/tests/golden/fixtures"
GOLD="$ROOT/tests/golden/expected"
STAGE="/tmp/qml_golden_stage"
UPDATE="${1:-}"

mkdir -p "$GOLD"
rm -rf "$STAGE"
mkdir -p "$STAGE/src"

# --- stage the crate-dep closure of qml_gen ---
cat > "$STAGE/Cargo.toml" <<'TOML'
[package]
name = "qml_golden"
version = "0.1.0"
edition = "2021"
[dependencies]
scraper = "0.20"
[[bin]]
name = "qg"
path = "src/main.rs"
TOML

cp "$ROOT/src/qml_gen.rs"  "$STAGE/src/qml_gen.rs"
cp "$ROOT/src/ad_filter.rs" "$STAGE/src/ad_filter.rs"
cp "$ROOT/src/table_ops.rs" "$STAGE/src/table_ops.rs"
cp "$ROOT/src/anim_detect.rs" "$STAGE/src/anim_detect.rs"
cp "$ROOT/src/layout_detect.rs" "$STAGE/src/layout_detect.rs"

# lib root exposing the modules
cat > "$STAGE/src/lib.rs" <<'RS'
pub mod ad_filter;
pub mod table_ops;
pub mod anim_detect;
pub mod layout_detect;
pub mod qml_gen;
RS

# runner: compile each fixture, print "===FILE===\n<qml>" to stdout
cat > "$STAGE/src/main.rs" <<'RS'
use qml_golden::qml_gen::{self, RenderStyle, select_body};
use qml_golden::ad_filter::FilterMode;
use scraper::Html;
use std::io::{Read, Write};

fn main() {
    // deterministic style: fixed theme/size/mono so output is stable.
    let sty = RenderStyle::from_theme(true, 15, false, FilterMode::Standard);
    let mut paths: Vec<String> = std::env::args().skip(1).collect();
    paths.sort();
    let mut out = std::io::stdout().lock();
    for p in paths {
        let mut html = String::new();
        std::fs::File::open(&p).unwrap().read_to_string(&mut html).unwrap();
        let doc = Html::parse_document(&html);
        let qml = select_body(&doc)
            .map(|b| qml_gen::generate_safe_qml(&b, 0, 12, "https://test.local/", &sty))
            .unwrap_or_default();
        let name = std::path::Path::new(&p).file_name().unwrap().to_string_lossy();
        write!(out, "===<<<{}>>>===\n{}\n", name, qml).unwrap();
    }
}
RS

echo "[qml_golden] building staged crate…"
( cd "$STAGE" && cargo build --release -q 2>&1 | grep -vE "^\s*Compiling|^\s*Finished" || true )

BIN="$STAGE/target/release/qg"
[ -x "$BIN" ] || { echo "[qml_golden] BUILD FAILED"; exit 2; }

# run over every fixture, split the combined output into per-fixture files
TMP_OUT="$STAGE/combined.txt"
"$BIN" "$FIXT"/*.html > "$TMP_OUT"

# split on the ===<<<name>>>=== markers using awk
mkdir -p "$STAGE/got"
awk -v dir="$STAGE/got" '
  /^===<<<.*>>>===$/ {
    name=$0; sub(/^===<<</,"",name); sub(/>>>===$/,"",name);
    file=dir"/"name".qml"; next
  }
  { if (file != "") print >> file }
' "$TMP_OUT"

status=0
if [ "$UPDATE" = "--update" ]; then
  for f in "$STAGE/got"/*.qml; do
    cp "$f" "$GOLD/$(basename "$f")"
  done
  echo "[qml_golden] goldens updated: $(ls "$GOLD" | wc -l) file(s)"
else
  for f in "$STAGE/got"/*.qml; do
    base="$(basename "$f")"
    if [ ! -f "$GOLD/$base" ]; then
      echo "[qml_golden] MISSING golden: $base (run --update to create)"; status=1; continue
    fi
    if ! diff -q "$GOLD/$base" "$f" >/dev/null; then
      echo "[qml_golden] DIFF in $base:"
      diff "$GOLD/$base" "$f" | head -30 || true
      status=1
    fi
  done
  if [ "$status" = "0" ]; then
    echo "[qml_golden] OK — all $(ls "$GOLD"/*.qml 2>/dev/null | wc -l) fixtures match golden output"
  fi
fi
exit $status
