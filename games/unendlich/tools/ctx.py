#!/usr/bin/env python3
"""ctx.py FILE PAT [PAT2 ...] [-n N] — tight context windows around every
match of each pattern (regex), one block per hit, line-numbered. Replaces
the grep+sed shuffle; N lines of context each side (default 4)."""
import re, sys
args = [a for a in sys.argv[1:]]
n = 4
if "-n" in args:
    i = args.index("-n"); n = int(args[i+1]); del args[i:i+2]
path, pats = args[0], args[1:]
lines = open(path, encoding="utf-8", errors="replace").read().splitlines()
for pat in pats:
    rx = re.compile(pat)
    hits = [i for i, l in enumerate(lines) if rx.search(l)]
    print(f"== {pat!r}: {len(hits)} hit(s)")
    for h in hits[:12]:
        for i in range(max(0, h - n), min(len(lines), h + n + 1)):
            mark = ">" if i == h else " "
            print(f"{mark}{i+1:>6} {lines[i]}")
        print("   ---")
