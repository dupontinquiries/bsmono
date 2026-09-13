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
# 3. the REAL engine, when cargo is here: examples/rig_dump.rs links the lib
#    (a --lib build works in the sandbox; only the test crate's codegen dies),
#    runs the engine-coupled checks, and dumps every design archetype through
#    creature_to_vertices_rigged. Merged into the viewer as "<arche> (engine)".
real="$tmp/rig_real.json"
if command -v cargo >/dev/null 2>&1 && [ "${RIG_NO_ENGINE:-0}" != "1" ]; then
  if (cd "$here/../.." && CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-1}" cargo build --example rig_dump -p unendlich-engine --profile sandboxtest >"$tmp/ex.log" 2>&1); then
    exe="$(cd "$here/../.." && cargo metadata --format-version 1 --no-deps 2>/dev/null | python3 -c 'import sys,json; print(json.load(sys.stdin)["target_directory"])')/sandboxtest/examples/rig_dump"
    [ -n "${CARGO_TARGET_DIR:-}" ] && exe="$CARGO_TARGET_DIR/sandboxtest/examples/rig_dump"
    if "$exe" > "$real" 2>"$tmp/ex.run.log"; then
      grep '^ok' "$tmp/ex.run.log" | sed 's/^/engine  /'
    else
      echo "ENGINE CHECKS FAILED (rig_dump exited non-zero):"; tail -20 "$tmp/ex.run.log"; exit 1
    fi
  else
    echo "note: cargo could not build examples/rig_dump here; viewer gets the rustc-only harness only"; tail -5 "$tmp/ex.log"; real=""
  fi
else
  real=""
fi
# 4. bake (merge the engine dump's creatures + sequences when present)
python3 - "$here/viewer.html" "$tmp/rig.json" "$out" "$real" <<'PY'
import sys, json
tpl, js, out, real = sys.argv[1:5]
html = open(tpl).read()
data = json.load(open(js))
if real:
    r = json.load(open(real))
    data['creatures'].update(r['creatures'])
    data['sequences'].extend(r['sequences'])
    data['engine_dump'] = r.get('generated_by', '')
assert html.count('__DATA__') == 1
text = json.dumps(data, separators=(',', ':'))
open(out, 'w').write(html.replace('__DATA__', text))
print(f"wrote {out} ({len(text)//1024} KB of data; {len(data['sequences'])} sequences)")
PY
rm -rf "$tmp"
