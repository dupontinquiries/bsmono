#!/bin/sh
# physui-check.sh — run the physical-control motion tests WITHOUT building the egui dep tree.
#
# physui.rs is deliberately egui-free ("no egui, no GPU"), so it compiles standalone in about a
# second. That matters because the full logscope-egui test build needs several GB of debug
# artefacts, and on a constrained machine the alternative is shipping motion changes with the tests
# unexecuted — which is how a feel regression gets in.
#
# The harness is assembled from the REAL sources: physui.rs verbatim, plus genviz::approach copied
# verbatim to satisfy its one crate:: reference. Nothing under test is paraphrased.
set -e
D=$(cd "$(dirname "$0")/.." && pwd)/crates/logscope-egui/src
T=$(mktemp -d)
python3 - "$D" "$T" <<'PY'
import sys
d, t = sys.argv[1], sys.argv[2]
src = open(f"{d}/physui.rs").read()
gv  = open(f"{d}/genviz.rs").read()
i = gv.index('pub fn approach('); j = gv.index('\n}\n', i) + 3
src = src.replace("crate::genviz::approach", "genviz::approach")
lines = src.split('\n')
k = next(n for n, l in enumerate(lines) if not l.startswith('//!'))
open(f"{t}/t.rs", "w").write('\n'.join(lines[:k]) + "\nmod genviz {\n" + gv[i:j] + "}\n" + '\n'.join(lines[k:]))
PY
rustc --test --edition 2021 -O "$T/t.rs" -o "$T/t"
"$T/t"
rm -rf "$T"
