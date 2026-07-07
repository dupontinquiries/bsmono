#!/usr/bin/env python3
"""api.py NAME [NAME2..] — compact signature lookup: every `pub fn/struct/
enum NAME` line (+1 continuation) across src/. One line per hit."""
import os, re, sys
SRC = "crates/unendlich-engine/src"
names = sys.argv[1:]
rx = re.compile(r"pub (?:fn|struct|enum|trait|const) (?:%s)\b" % "|".join(map(re.escape, names)))
for dp, _, fns in os.walk(SRC):
    for fn in fns:
        if not fn.endswith(".rs"): continue
        p = os.path.join(dp, fn)
        ls = open(p, encoding="utf-8", errors="replace").read().splitlines()
        for i, l in enumerate(ls):
            if rx.search(l):
                cont = (" " + ls[i+1].strip()) if l.rstrip().endswith("(") and i+1 < len(ls) else ""
                print(f"{p}:{i+1}: {l.strip()}{cont}")
