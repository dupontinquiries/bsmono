#!/bin/sh
for seed in 1 7 99 12345; do
python3 -c "
import sys,random
random.seed($seed)
sys.stdout.buffer.write(bytes(random.randrange(256) for _ in range(1<<16)))" > /tmp/qdiff/b.bin
for ty in q2_k q3_k q4_k q5_k q6_k iq2_xxs iq2_xs iq3_xxs iq4_nl iq3_s iq2_s iq4_xs iq1_m iq1_s; do
  /tmp/qref "$ty" 1024 < /tmp/qdiff/b.bin > /tmp/qdiff/r.txt 2>/dev/null || continue
  /tmp/qdiff/target/release/qdiff "$ty" 1024 < /tmp/qdiff/b.bin > /tmp/qdiff/o.txt 2>/dev/null || { echo "seed $seed $ty OURS-ERR"; continue; }
  python3 -c "
import sys
r=[float(x) for x in open('/tmp/qdiff/r.txt')];o=[float(x) for x in open('/tmp/qdiff/o.txt')]
sc=max((abs(x) for x in r), default=1.0) or 1.0
bad=sum(1 for a,b in zip(r,o) if abs(a-b) > 1e-5*sc)
print(f'seed $seed {\"$ty\":9s} {\"OK\" if bad==0 else str(bad)+\" MISMATCH\"}')"
done
done
