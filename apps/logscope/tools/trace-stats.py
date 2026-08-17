#!/usr/bin/env python3
"""Summarize a denoise-bench --trace jsonl: per-step confidence/entropy/margin,
argmax concentration, label rate. The llada2-run1 read: step-0 entropy 8.6 vs
uniform 11.96 with coherent margins ACQUITS gross corruption (scrambled qkv,
rope salad smear toward uniform); confident degenerate continuation by step 8+
is the missing-contribution class (absent shexp / routing magnitude)."""
import json, sys, collections, statistics as st, math
recs = [json.loads(l) for l in open(sys.argv[1])]
by = collections.defaultdict(list)
for r in recs: by[r['step']].append(r)
ids = collections.Counter(r['argmax'] for r in recs)
lab = sum(r['label'] for r in recs)
print(f"{len(recs)} records | top argmax ids {ids.most_common(5)} | label=1 {lab/len(recs):.2f}")
for s_ in sorted(by):
    rs = by[s_]; p=[r['p'] for r in rs]; h=[r['entropy'] for r in rs]; m=[r['margin'] for r in rs]
    print(f"step {s_:>3}: n={len(rs):>4} p_top med {st.median(p):.3f} max {max(p):.3f}  H med {st.median(h):5.2f}  margin med {st.median(m):5.2f}")
print("(uniform H over 157k vocab = 11.96; over 32k = 10.37)")
