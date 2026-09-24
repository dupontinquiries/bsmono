# The engine's "model", as a transparent policy: reads ONLY survey digests
# (lane distance, stream distance) -- what mapgen gives an author -- and the
# user's brief (rocks, occasional grass and scrub, bleak; so no jungle trees).
import re, subprocess, sys
import os
EVAL=os.environ.get("MAPGEN_EVAL", "target/debug/examples/mapgen_eval")
pat=re.compile(r"within \d+ of \((-?\d+), (-?\d+)\).*nearest lane (-?\d+) u, river (-?\d+) u")
def survey(points):
    open("q.txt","w").write("\n".join(f"survey {x} {z} 3" for x,z in points)+"\n")
    out=subprocess.run([EVAL,"run","q.txt"],capture_output=True,text=True).stdout
    return [(int(a),int(b),int(c),int(d)) for a,b,c,d in pat.findall(out)]
# round 1: coarse, blue half
r1=survey([(x,z) for z in range(-150,-5,20) for x in range(-150,151,25) if (x*x+z*z)**.5<160])
# round 2: fine grid where round 1 says there is play nearby (lane < 40)
hot=[(x,z) for x,z,l,r in r1 if l<40]
fine=sorted({(x+dx,z+dz) for x,z in hot for dx in range(-12,13,6) for dz in range(-12,13,6) if z+dz<-3 and ((x+dx)**2+(z+dz)**2)**.5<158})
r2=[]
for i in range(0,len(fine),280):          # the engine stops at 300 steps a run
    r2+=survey(fine[i:i+280])
print(f"# surveys: {len(r1)} coarse + {len(r2)} fine", file=sys.stderr)
picked=[]
def far(x,z,d): return all((x-a)**2+(z-b)**2>=d*d for _,a,b in picked)
def take(kind,cands,spacing,cap):
    n=0
    for x,z,l,r in cands:
        if n>=cap: break
        if far(x,z,spacing):
            picked.append((kind,x,z)); n+=1
# scrub and grass by the streams
take("Bush",[c for c in r2 if 3<=c[3]<=9 and c[2]>5],6,20)
take("Fern",[c for c in r2 if 3<=c[3]<=10 and c[2]>5],4,20)
# rocks flanking the paths, boulders further out
take("Rock",[c for c in r2 if 7<=c[2]<=14 and c[3]>4],10,35)
take("BoulderCluster",[c for c in r2 if 14<c[2]<=24 and c[3]>5],16,15)
# occasional grass on the lane margins
take("Fern",[c for c in r2 if 5<=c[2]<=8 and c[3]>4],14,25)
lines=[f"place {k} {x} {z}" for k,x,z in picked]
open("engine_calls.txt","w").write("\n".join(lines)+"\n")
print(f"# {len(lines)} place calls", file=sys.stderr)
