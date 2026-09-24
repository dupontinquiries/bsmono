# The engine's "model", as a transparent policy: reads ONLY survey digests
# (lane distance, stream distance) -- what mapgen gives an author -- and the
# user's brief (rocks, occasional grass and scrub, bleak; so no jungle trees).
import re, subprocess, sys
import os
EVAL=os.environ.get("MAPGEN_EVAL", "target/debug/examples/mapgen_eval")
pat=re.compile(r"within \d+ of \((-?\d+), (-?\d+)\): .*?(\d+)% walkable.*nearest lane (-?\d+) u, river (-?\d+) u(, INSIDE)?")
def survey(points):
    open("q.txt","w").write("\n".join(f"survey {x} {z} 2.5" for x,z in points)+"\n")
    out=subprocess.run([EVAL,"run","q.txt"],capture_output=True,text=True).stdout
    # v2: drop anything the digest flags as inside a tower clearing (54 of the
    # 66 refusals in v1 were exactly that)
    # v3: the digest's walkable % now counts cliff; keep only open ground
    return [(int(a),int(b),int(l),int(r),int(w)) for a,b,w,l,r,t in pat.findall(out) if not t and int(w) >= 60]
# round 1: coarse, blue half
r1=survey([(x,z) for z in range(-150,-5,20) for x in range(-150,151,25) if (x*x+z*z)**.5<160])
# round 2: fine grid where round 1 says there is play nearby (lane < 40)
hot=[(x,z) for x,z,l,r,w in r1 if l<40]
# r1 was filtered to open ground; also look around the rest of the coarse grid
hot=[(x,z) for z in range(-150,-5,20) for x in range(-150,151,25) if (x*x+z*z)**.5<160]
fine=sorted({(x+dx,z+dz) for x,z in hot for dx in range(-12,13,6) for dz in range(-12,13,6) if z+dz<-3 and ((x+dx)**2+(z+dz)**2)**.5<158})
r2=[]
for i in range(0,len(fine),280):          # the engine stops at 300 steps a run
    r2+=survey(fine[i:i+280])
print(f"# surveys: {len(r1)} coarse + {len(r2)} fine", file=sys.stderr)
picked=[]
def far(x,z,d): return all((x-a)**2+(z-b)**2>=d*d for _,a,b in picked)
def take(kind,cands,spacing,cap):
    n=0
    for x,z,l,r,*_ in cands:
        if n>=cap: break
        if far(x,z,spacing):
            picked.append((kind,x,z)); n+=1
# scrub and grass by the streams
# v3: a clearing, as the digests see one -- fully open, off the lanes
take("Bush",[c for c in r2 if c[4]>=100 and c[2]>=9 and c[3]>=7],7,24)
take("Fern",[c for c in r2 if c[4]>=100 and c[2]>=8 and c[3]>=7],5,30)
# rocks flanking the paths, boulders further out
# v3: stones where the ground is partly open -- the foot of a wall
take("Rock",[c for c in r2 if 60<=c[4]<100 and c[2]>=6 and c[3]>=6],9,35)
take("BoulderCluster",[c for c in r2 if 70<=c[4]<100 and c[2]>=10 and c[3]>=7],16,12)
# occasional grass on the lane margins
take("Fern",[c for c in r2 if c[4]>=90 and 5<=c[2]<=8 and c[3]>=7],14,20)
lines=[f"place {k} {x} {z}" for k,x,z in picked]
open("engine_calls_v3.txt","w").write("\n".join(lines)+"\n")
print(f"# {len(lines)} place calls", file=sys.stderr)
