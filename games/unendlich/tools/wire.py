#!/usr/bin/env python3
"""wire.py field NAME RUST_TYPE DEFAULT [DOC] — the most-repeated edit in
the project: add a display field to WorldRenderer (decl + ctor default),
anchor-gated, FSM-checked. Prints the bin-side sync line to paste.
"""
import subprocess, sys
assert sys.argv[1] == "field"
name, rty, dflt = sys.argv[2], sys.argv[3], sys.argv[4]
doc = sys.argv[5] if len(sys.argv) > 5 else "wired display field"
P = "crates/unendlich-engine/src/render/world_renderer.rs"
t = open(P, encoding="utf-8").read()
da = "    pub menu_screen_kind: u8,"
ca = "            menu_screen_kind: 0,"
for a in (da, ca):
    if t.count(a) != 1:
        print(f"FAIL anchor x{t.count(a)}: {a!r}"); sys.exit(2)
t = t.replace(da, da + f"\n    /// {doc}\n    pub {name}: {rty},")
t = t.replace(ca, ca + f"\n            {name}: {dflt},")
open(P, "w", encoding="utf-8").write(t)
r = subprocess.run(["python3", "tools/wiring_radar.py", "check", P],
                   capture_output=True, text=True)
print(r.stdout, end="")
sys.exit(0 if r.returncode == 0 else 3)
