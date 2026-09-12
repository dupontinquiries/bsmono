#!/usr/bin/env bash
# harness/rig/run.sh — build the rig harness with plain rustc (no cargo, no
# engine compile), run it, and bake its JSON into viewer.html.
#
#   harness/rig/run.sh [out.html]      default: /mnt/user-data/outputs/rig_viewer.html
#                                      (falls back to /tmp/rig_viewer.html)
#
# Also runs the pure-module tests the harness composes (rustc --test on a
# shim that includes the same real files), so a red core never reaches the
# viewer.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
out="${1:-}"
if [ -z "$out" ]; then
  if [ -d /mnt/user-data/outputs ]; then out=/mnt/user-data/outputs/rig_viewer.html; else out=/tmp/rig_viewer.html; fi
fi
tmp="$(mktemp -d)"
# 1. tests: the same #[path] set, compiled as a test crate
cat > "$tmp/lib.rs" <<RS
#![allow(dead_code)]
#[path = "$here/sim/mod.rs"] pub mod sim;
#[path = "$here/util/mod.rs"] pub mod util;
#[path = "$here/../../crates/unendlich-engine/src/item_model.rs"] pub mod item_model;
#[path = "$here/../../crates/unendlich-engine/src/anim_rig.rs"] pub mod anim_rig;
RS
rustc --test --edition 2021 "$tmp/lib.rs" -o "$tmp/t" 2>"$tmp/t.log" || { echo "TEST SHIM FAILED TO COMPILE"; tail -30 "$tmp/t.log"; exit 1; }
"$tmp/t" >"$tmp/t.out" 2>&1 || { echo "TESTS FAILED"; tail -40 "$tmp/t.out"; exit 1; }
grep 'test result:' "$tmp/t.out"
# 2. the harness
rustc --edition 2021 -O "$here/main.rs" -o "$tmp/rig" 2>"$tmp/rig.log" || { echo "HARNESS FAILED TO COMPILE"; tail -30 "$tmp/rig.log"; exit 1; }
"$tmp/rig" > "$tmp/rig.json"
# 3. bake
python3 - "$here/viewer.html" "$tmp/rig.json" "$out" <<'PY'
import sys
tpl, js, out = sys.argv[1:4]
html = open(tpl).read()
data = open(js).read()
assert html.count('__DATA__') == 1
open(out, 'w').write(html.replace('__DATA__', data))
print(f"wrote {out} ({len(data)//1024} KB of data)")
PY
rm -rf "$tmp"
