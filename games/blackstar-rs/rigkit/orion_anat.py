import json, struct, math
from collections import defaultdict

def load(path):
    d=open(path,'rb').read(); ln=struct.unpack('<I',d[12:16])[0]
    J=json.loads(d[20:20+ln]); bn=d[20+ln+8:]
    def acc(i):
        a=J['accessors'][i]; v=J['bufferViews'][a['bufferView']]
        off=v.get('byteOffset',0)+a.get('byteOffset',0); n=a['count']
        if a['type']=='VEC3': return [struct.unpack_from('<3f',bn,off+k*12) for k in range(n)]
        fmt={5121:'B',5123:'H',5125:'I'}[a['componentType']]; sz={'B':1,'H':2,'I':4}[fmt]
        return [struct.unpack_from('<'+fmt,bn,off+k*sz)[0] for k in range(n)]
    pr=J['meshes'][0]['primitives'][0]
    return acc(pr['attributes']['POSITION']), acc(pr['indices'])

def islands(V,I,q=1e-4):
    key={}; rep=[]; pos=[]
    for v in V:
        k=(round(v[0]/q),round(v[1]/q),round(v[2]/q))
        if k not in key: key[k]=len(key); pos.append(v)
        rep.append(key[k])
    par=list(range(len(key)))
    def find(a):
        while par[a]!=a: par[a]=par[par[a]]; a=par[a]
        return a
    for k in range(0,len(I),3):
        a,b,c=rep[I[k]],rep[I[k+1]],rep[I[k+2]]
        ra,rb=find(a),find(b)
        if ra!=rb: par[ra]=rb
        rb2,rc=find(b),find(c)
        if rb2!=rc: par[rb2]=rc
    g=defaultdict(list)
    for i in range(len(key)): g[find(i)].append(pos[i])
    # DETERMINISTIC ORDER. Sorting by vertex count alone leaves ties broken by
    # dict order, which differs between runs -- and orion has four tyres of
    # 1855-1856 verts. The cached geometry and the freshly computed bounding
    # boxes then disagreed about which island was which, so a wheel group
    # ended up holding one rear wheel's box and another wheel's mesh: 1855
    # vertices at the front of the car inside a group centred at the rear.
    #
    # A count check cannot catch that, which is why it did not. Breaking ties
    # on the centroid makes the order a property of the geometry.
    def _key(ps):
        n = len(ps)
        c = [sum(p[a] for p in ps) / n for a in range(3)]
        return (-n, round(c[0], 3), round(c[1], 3), round(c[2], 3))
    return sorted(g.values(), key=_key)

def bb(ps):
    xs=[p[0] for p in ps]; ys=[p[1] for p in ps]; zs=[p[2] for p in ps]
    lo=(min(xs),min(ys),min(zs)); hi=(max(xs),max(ys),max(zs))
    return dict(lo=lo,hi=hi,n=len(ps),
                ctr=tuple((lo[i]+hi[i])/2 for i in range(3)),
                size=tuple(hi[i]-lo[i] for i in range(3)))

if __name__=="__main__":
    V,I=load(__import__('rigpaths').inp('orion.glb'))
    gs=islands(V,I)
    B=[bb(g) for g in gs]
    whole=bb(V)
    print(f"orion: {len(V)} verts, {len(gs)} islands")
    print(f"extent {tuple(round(x,2) for x in whole['size'])}  centre {tuple(round(x,2) for x in whole['ctr'])}")
    # which axis is length / width / height?
    ax=sorted(range(3), key=lambda i:-whole['size'][i])
    print(f"longest axis = {'xyz'[ax[0]]}, then {'xyz'[ax[1]]}, then {'xyz'[ax[2]]}")
    # mirror plane: the axis whose centroid distribution is symmetric
    for a in range(3):
        c=whole['ctr'][a]
        left=sum(1 for b in B if b['ctr'][a]<c-whole['size'][a]*0.03)
        right=sum(1 for b in B if b['ctr'][a]>c+whole['size'][a]*0.03)
        mid=len(B)-left-right
        print(f"  axis {'xyz'[a]}: {left} left / {mid} centre / {right} right")

def mirror_pairs(B, axis=2, tol=0.02, scale=1.0):
    """Match islands across the mirror plane by their mirrored bounding box.

    A symmetric transform needs to know which part is the left twin of which
    right one; without it the two sides fold at slightly different angles and
    the whole thing reads as broken rather than mechanical."""
    used=set(); pairs=[]; solo=[]
    key=lambda b: (b['n'], round(b['size'][0],1), round(b['size'][1],1), round(b['size'][2],1))
    buckets=defaultdict(list)
    for i,b in enumerate(B): buckets[key(b)].append(i)
    for i,b in enumerate(B):
        if i in used: continue
        if abs(b['ctr'][axis]) < scale*tol:
            solo.append(i); used.add(i); continue
        best=None; bd=1e30
        for j in buckets[key(b)]:
            if j==i or j in used: continue
            c=B[j]['ctr']
            d=(abs(c[0]-b['ctr'][0])+abs(c[1]-b['ctr'][1])+abs(c[2]+b['ctr'][axis]))
            if d<bd: bd, best = d, j
        if best is not None and bd < scale*tol:
            pairs.append((i,best)); used.add(i); used.add(best)
        else:
            solo.append(i); used.add(i)
    return pairs, solo


def parts(path=None):
    """Per-island geometry -- vertices and island-local faces -- in the SAME
    order as `islands()` (vertex count, ties broken on the centroid).

    This was a 6 MB pickle cached in a sandbox directory, derived entirely
    from orion.glb, so the kit rebuilds it on first use (into out/) instead
    of shipping an opaque file. The order must match `islands()` exactly: the
    catalogue names islands by index, and orion's four tyres (1855-1856
    verts) are only told apart by the tie-break.
    """
    import os, pickle
    from collections import defaultdict
    import rigpaths
    cache = rigpaths.out('orion_parts.pkl')
    if os.path.exists(cache):
        return pickle.load(open(cache, 'rb'))
    V, I = load(path or rigpaths.inp('orion.glb'))
    key = {}; rep = []; pos = []
    q = 1e-4
    for v in V:
        k = (round(v[0] / q), round(v[1] / q), round(v[2] / q))
        if k not in key:
            key[k] = len(key); pos.append(v)
        rep.append(key[k])
    par = list(range(len(key)))

    def find(a):
        while par[a] != a:
            par[a] = par[par[a]]; a = par[a]
        return a
    for k in range(0, len(I), 3):
        a, b, c = rep[I[k]], rep[I[k + 1]], rep[I[k + 2]]
        ra, rb = find(a), find(b)
        if ra != rb: par[ra] = rb
        rb2, rc = find(b), find(c)
        if rb2 != rc: par[rb2] = rc
    tris = defaultdict(list)
    for k in range(0, len(I), 3):
        tris[find(rep[I[k]])].append((rep[I[k]], rep[I[k + 1]], rep[I[k + 2]]))
    verts = defaultdict(set)
    for r, ts in tris.items():
        for t in ts: verts[r].update(t)

    def _key(r):
        ps = [pos[v] for v in verts[r]]; n = len(ps)
        c = [sum(p[a] for p in ps) / n for a in range(3)]
        return (-n, round(c[0], 3), round(c[1], 3), round(c[2], 3))
    out = []
    for r in sorted(tris, key=_key):
        vs = sorted(verts[r]); remap = {v: i for i, v in enumerate(vs)}
        out.append({'v': [pos[v] for v in vs],
                    'f': [(remap[a], remap[b], remap[c]) for a, b, c in tris[r]]})
    pickle.dump(out, open(cache, 'wb'))
    return out
