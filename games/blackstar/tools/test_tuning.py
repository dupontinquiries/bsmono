#!/usr/bin/env python3
"""test_tuning.py -- the tuning/kind/pair tables must stay coherent.

Gates (each guards a real failure mode):
  * every installed model has a tuning entry with a valid kind (a model without a
    kind animates with the wrong profile silently);
  * pairs are symmetric and cross-kind vehicle<->robot (an asymmetric pair makes T
    a one-way trip; a same-kind pair is a classification bug);
  * every pair target exists;
  * the baked header agrees with the json (EX_N_IMPORTED == entries used, kind enum
    range, pair indices in range) -- the header is what the game actually reads.
"""
import json, os, re, sys

root = "."
if "--root" in sys.argv:
    root = sys.argv[sys.argv.index("--root") + 1]
t = json.load(open(os.path.join(root, "model_tuning.json")))
data = os.path.join(root, "engine_src", "data")
names = sorted(f[len("imported_"):-4] for f in os.listdir(data)
               if f.startswith("imported_") and f.endswith(".iqm"))
KINDS = {"prop", "robot", "vehicle", "weapon", "env", "ship"}
bad = 0
for n in names:
    e = t.get(n)
    if not e or e.get("kind") not in KINDS:
        print(f"MISSING/BAD kind: {n}"); bad += 1
pairs = {n: e["pair"] for n, e in t.items() if "pair" in e}
for a, b in pairs.items():
    if b not in t: print(f"PAIR TARGET MISSING: {a} -> {b}"); bad += 1; continue
    if pairs.get(b) != a: print(f"PAIR ASYMMETRIC: {a} -> {b} -> {pairs.get(b)}"); bad += 1
    ka, kb = t[a].get("kind"), t[b].get("kind")
    if {ka, kb} != {"vehicle", "robot"}:
        print(f"PAIR KINDS WRONG: {a}({ka}) <-> {b}({kb})"); bad += 1
hdr = open(os.path.join(root, "engine_src", "imported_models.h")).read()
n_hdr = int(re.search(r"#define EX_N_IMPORTED (\d+)", hdr).group(1))
if n_hdr != len(names):
    print(f"HEADER COUNT {n_hdr} != data {len(names)}"); bad += 1
kind_arr = re.search(r"g_imp_kind\[EX_N_IMPORTED\] = \{ ([^}]*) \}", hdr).group(1)
kvals = [int(x) for x in kind_arr.split(",")]
pair_arr = re.search(r"g_imp_pair\[EX_N_IMPORTED\] = \{ ([^}]*) \}", hdr).group(1)
pvals = [int(x) for x in pair_arr.split(",")]
if len(kvals) != n_hdr or len(pvals) != n_hdr:
    print("HEADER ARRAY LENGTHS WRONG"); bad += 1
if any(k < 0 or k > 5 for k in kvals): print("KIND OUT OF RANGE"); bad += 1
if any(p < -1 or p >= n_hdr for p in pvals): print("PAIR INDEX OUT OF RANGE"); bad += 1
sym = sum(1 for i, p in enumerate(pvals) if p >= 0 and pvals[p] != i)
if sym: print(f"{sym} HEADER PAIRS ASYMMETRIC"); bad += 1
npairs = sum(1 for p in pvals if p >= 0) // 2
print(f"tuning check: {len(names)} models, {npairs} header pairs, "
      f"kinds " + ",".join(f"{k}:{kvals.count(k)}" for k in range(6)))
if bad:
    print(f"FAILED: {bad} problem(s)."); sys.exit(1)
print("OK: tuning, kinds, pairs, and the baked header all agree.")
