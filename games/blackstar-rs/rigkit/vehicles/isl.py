"""One island list, one order, ids carried with the geometry.

Surveying by vertex count and rendering by triangle count gave the same
island two different numbers -- a tyre got labelled as a door. Every tool
that names an island reads it from here, so a number means one thing."""
import math
from collections import defaultdict
def islands(P, I, q=1e-4):
    key={};rep=[]
    for v in P:
        k=(round(v[0]/q),round(v[1]/q),round(v[2]/q)); rep.append(key.setdefault(k,len(key)))
    par=list(range(len(key)))
    def f(a):
        while par[a]!=a: par[a]=par[par[a]]; a=par[a]
        return a
    for t in range(0,len(I),3):
        a,b,c=rep[I[t]],rep[I[t+1]],rep[I[t+2]]
        ra,rb=f(a),f(b)
        if ra!=rb: par[ra]=rb
        rb,rc=f(b),f(c)
        if rb!=rc: par[rb]=rc
    tris=defaultdict(list)
    for t in range(0,len(I),3): tris[f(rep[I[t]])].append((I[t],I[t+1],I[t+2]))
    out=[]
    for r,ts in tris.items():
        vs=sorted({i for t in ts for i in t})
        pts=[P[i] for i in vs]
        lo=[min(p[a] for p in pts) for a in range(3)]; hi=[max(p[a] for p in pts) for a in range(3)]
        c=[sum(p[a] for p in pts)/len(pts) for a in range(3)]
        out.append({'tris':ts,'verts':vs,'n':len(vs),'lo':lo,'hi':hi,
                    'size':[hi[a]-lo[a] for a in range(3)],'ctr':c})
    # deterministic: size, then centroid -- ties cannot fall to hash order
    out.sort(key=lambda d:(-d['n'],round(d['ctr'][0],4),round(d['ctr'][1],4),round(d['ctr'][2],4)))
    for i,d in enumerate(out): d['id']=i
    return out
