# Derive an ORDERED activity stream from Hector's transcript: scan the
# narration in sequence, emitting an Activity marker when we see a reboot,
# a file mention (.py/.rs/.c), a run/script, or an editor open. This is a
# noisy but REAL-ordering stream — validates the segmenter on actual
# workflow cadence, not planted data.
import re
text = open("/tmp/asahi_all.txt").read().replace("\n"," ")
text = re.sub(r'\s+',' ',text).lower()
toks = text.split()
events = []  # (kind, target)
i=0
# tracer/dcp work happens in an asahi-like repo; reboots reset context.
files_seen = set()
while i < len(toks):
    w = toks[i]
    nxt = toks[i+1] if i+1<len(toks) else ""
    if w in ("reboot","reboots","reflash"):
        events.append(("Reboot",""))
    elif re.search(r'\.(py|rs|c|h)\b', w):
        events.append(("EditorOpen", w.strip('.,')))
    elif w in ("python","ipython") and nxt:
        events.append(("Run", f"python {nxt.strip('.,')}"))
    elif w in ("tracer","dcptracer"):
        events.append(("EditorOpen","tracer.py"))
    elif w in ("run","running") and nxt in ("script","it","trace","shell"):
        events.append(("Run", nxt.strip('.,')))
    elif w in ("make","cargo","build"):
        events.append(("Build", w))
    i+=1
# collapse immediate duplicates
collapsed=[]
for e in events:
    if not collapsed or collapsed[-1]!=e: collapsed.append(e)
print(f"derived {len(collapsed)} activity events from the transcript")
# write as a simple TSV the Rust harness reads: kind\ttarget
with open("/tmp/hector_stream.tsv","w") as f:
    for k,t in collapsed:
        f.write(f"{k}\t{t}\n")
# show the kind distribution + a sample
from collections import Counter
c=Counter(k for k,_ in collapsed)
print("kinds:", dict(c))
print("sample:", collapsed[:20])
