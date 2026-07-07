#!/usr/bin/env python3
"""edit.py SPECFILE — batch anchored edits with the full safety stack in
one call: unique-anchor enforcement, atomic apply (all-or-nothing per
file), FSM check after, auto-revert on any failure. Replaces the
hand-written python heredoc blocks.

Spec format (blocks separated by lines of `====`):
    FILE path/to/file.rs
    <<<<
    exact old text (must appear EXACTLY once)
    >>>>
    new text
Multiple blocks may target the same file; they apply in order against
the evolving buffer. `>>>>` with nothing after = deletion. A FILE line
with `+++` instead of `<<<<` creates/overwrites... not supported — use
create_file for new files.
Exit 0 = all applied + FSM PASS; non-zero = nothing changed on disk.
"""
import subprocess, sys

spec = open(sys.argv[1], encoding="utf-8").read()
blocks = [b for b in spec.split("\n====\n") if b.strip()]
edits = []  # (file, old, new)
for b in blocks:
    lines = b.split("\n")
    assert lines[0].startswith("FILE "), f"bad block head: {lines[0]!r}"
    f = lines[0][5:].strip()
    body = "\n".join(lines[1:])
    assert "\n<<<<\n" in "\n" + body and "\n>>>>" in body, f"missing markers in {f}"
    _, rest = ("\n" + body).split("\n<<<<\n", 1)
    old, new = rest.split("\n>>>>", 1)
    new = new[1:] if new.startswith("\n") else new
    edits.append((f, old, new))

bufs, orig = {}, {}
for f, old, new in edits:
    if f not in bufs:
        orig[f] = open(f, encoding="utf-8").read()
        bufs[f] = orig[f]
    n = bufs[f].count(old)
    if n != 1:
        print(f"FAIL {f}: anchor x{n} (need exactly 1): {old[:60]!r}")
        sys.exit(2)
    bufs[f] = bufs[f].replace(old, new, 1)

for f, t in bufs.items():
    open(f, "w", encoding="utf-8").write(t)
rust_like = [f for f in bufs if f.endswith((".rs", ".comp", ".frag", ".vert"))]
shells = [f for f in bufs if f.endswith(".sh")]
if rust_like:
    r = subprocess.run(["python3", "tools/wiring_radar.py", "check", *rust_like],
                       capture_output=True, text=True)
else:
    r = subprocess.CompletedProcess([], 0, "", "")
for s in shells:
    sh = subprocess.run(["bash", "-n", s], capture_output=True, text=True)
    if sh.returncode != 0:
        r = subprocess.CompletedProcess([], 3, r.stdout + sh.stderr, "")
print(r.stdout, end="")
if r.returncode != 0:
    for f, t in orig.items():
        open(f, "w", encoding="utf-8").write(t)
    print("REVERTED all files (FSM fail)")
    sys.exit(3)
print(f"OK: {len(edits)} edit(s) across {len(bufs)} file(s)")
