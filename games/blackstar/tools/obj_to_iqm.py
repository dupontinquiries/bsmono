#!/usr/bin/env python3
"""obj_to_iqm.py -- convert a Wavefront .obj (e.g. a Blender export) into an IQM
v2 static model in the SAME byte layout the engine loads (identical to
gen_props.write_iqm / gen_box.py), so hand-sculpted parts drop into a robot part
slot exactly like the spine-grown ones.

By default the mesh is recentered and normalized so its longest axis half-extent
== 1 (the part-slot convention: box.iqm / plimb.iqm / spine_*.iqm all follow it).
Pass --raw to keep the OBJ's original coordinates (for world props, not parts).

  # Blender: File > Export > Wavefront (.obj), +Y up, -Z forward, triangulate.
  python3 tools/obj_to_iqm.py mypart.obj --out engine_src/data/spine_custom.iqm --tex wall.png
  python3 tools/obj_to_iqm.py prop.obj  --out engine_src/data/prop.iqm --raw --tex rock.png

Handles: v / vt / vn, faces as v, v/vt, v//vn, v/vt/vn; polygons are fan-
triangulated; missing normals are computed smooth; missing UVs default to (.5,.5).
"""
import struct, os, math, argparse

def vsub(a,b): return (a[0]-b[0],a[1]-b[1],a[2]-b[2])
def vcross(a,b): return (a[1]*b[2]-a[2]*b[1],a[2]*b[0]-a[0]*b[2],a[0]*b[1]-a[1]*b[0])
def vnorm(a):
    l=math.sqrt(a[0]*a[0]+a[1]*a[1]+a[2]*a[2])
    return (a[0]/l,a[1]/l,a[2]/l) if l>1e-9 else (0.0,1.0,0.0)

def parse_obj(path):
    P=[]; UV=[]; N=[]; faces=[]
    for line in open(path, errors="replace"):
        if not line or line[0]=='#': continue
        t=line.split()
        if not t: continue
        if   t[0]=='v':  P.append(tuple(float(x) for x in t[1:4]))
        elif t[0]=='vt': UV.append(tuple(float(x) for x in t[1:3]))
        elif t[0]=='vn': N.append(tuple(float(x) for x in t[1:4]))
        elif t[0]=='f':
            face=[]
            for w in t[1:]:
                a=(w.split('/')+['','',''])[:3]
                vi=int(a[0]); ti=int(a[1]) if a[1] else 0; ni=int(a[2]) if a[2] else 0
                face.append((vi,ti,ni))
            for k in range(1,len(face)-1):     # fan triangulate
                faces.append((face[0],face[k],face[k+1]))
    return P,UV,N,faces

def build(P,UV,N,faces):
    verts=[]; norms=[]; uvs=[]; tris=[]; cache={}
    def res(i,n): return (i-1) if i>0 else (n+i)  # OBJ 1-based, negatives allowed
    for tri in faces:
        idx=[]
        for (vi,ti,ni) in tri:
            key=(vi,ti,ni)
            if key not in cache:
                p=P[res(vi,len(P))]
                uv=UV[res(ti,len(UV))] if (ti and UV) else (0.5,0.5)
                nm=N[res(ni,len(N))] if (ni and N) else None
                cache[key]=len(verts)
                verts.append(p); uvs.append((uv[0],uv[1])); norms.append(nm)
            idx.append(cache[key])
        tris.append(tuple(idx))
    # fill any missing normals by smooth accumulation
    if any(n is None for n in norms):
        acc=[[0.0,0.0,0.0] for _ in verts]
        for (a,b,c) in tris:
            fn=vcross(vsub(verts[b],verts[a]), vsub(verts[c],verts[a]))
            for i in (a,b,c):
                acc[i][0]+=fn[0]; acc[i][1]+=fn[1]; acc[i][2]+=fn[2]
        norms=[vnorm(tuple(acc[i])) if norms[i] is None else norms[i] for i in range(len(verts))]
    return verts,norms,uvs,tris

def normalize_slot(verts):
    xs=[v[0] for v in verts]; ys=[v[1] for v in verts]; zs=[v[2] for v in verts]
    cx=(max(xs)+min(xs))*0.5; cy=(max(ys)+min(ys))*0.5; cz=(max(zs)+min(zs))*0.5
    s=max(max(xs)-min(xs),max(ys)-min(ys),max(zs)-min(zs)) or 1.0
    return [((v[0]-cx)/s*2.0,(v[1]-cy)/s*2.0,(v[2]-cz)/s*2.0) for v in verts]

def write_iqm(path, verts, norms, uvs, tris, tex):
    def u(*a): return struct.pack("<"+"I"*len(a),*a)
    mat=(tex+"\x00").encode(); text=b"\x00"+mat+b"prop\x00"
    mat_off=1; name_off=1+len(mat)
    nv=len(verts); nt=len(tris); ofs=16+4+4*26
    ofs_text=ofs; ofs+=len(text); ofs+=(-ofs)%4
    ofs_meshes=ofs; ofs+=6*4; ofs_va=ofs; ofs+=15*4
    ofs_pos=ofs; ofs+=nv*12; ofs_uv=ofs; ofs+=nv*8; ofs_nrm=ofs; ofs+=nv*12; ofs_tri=ofs; ofs+=nt*12
    filesize=ofs
    out=b"INTERQUAKEMODEL\x00"+u(2,filesize,0)+u(len(text),ofs_text)+u(1,ofs_meshes)+u(3,nv,ofs_va)+u(nt,ofs_tri,0)
    out+=u(0,0)+u(0,0)+u(0,0)+u(0,0,0,0)+u(0,0)+u(0,0)
    out+=text+b"\x00"*((-(ofs_text+len(text)))%4)
    out+=u(name_off,mat_off,0,nv,0,nt)
    out+=u(0,0,7,3,ofs_pos)+u(1,0,7,2,ofs_uv)+u(2,0,7,3,ofs_nrm)
    for p in verts: out+=struct.pack("<3f",*p)
    for uv in uvs:  out+=struct.pack("<2f",*uv)
    for n in norms: out+=struct.pack("<3f",*n)
    for t in tris:  out+=u(*t)
    assert len(out)==filesize
    open(path,"wb").write(out); return nv,nt,filesize

def main():
    ap=argparse.ArgumentParser(description="Wavefront OBJ -> IQM (engine part format)")
    ap.add_argument("obj")
    ap.add_argument("--out", required=True)
    ap.add_argument("--tex", default="wall.png")
    ap.add_argument("--raw", action="store_true", help="keep OBJ coords (skip unit-slot normalize)")
    a=ap.parse_args()
    P,UV,N,faces=parse_obj(a.obj)
    if not faces: raise SystemExit("obj_to_iqm: no faces found in "+a.obj)
    verts,norms,uvs,tris=build(P,UV,N,faces)
    if not a.raw: verts=normalize_slot(verts)
    nv,nt,sz=write_iqm(a.out, verts,norms,uvs,tris, a.tex)
    print(f"wrote {a.out}: {nv} verts {nt} tris {sz} bytes  (from {len(P)} obj verts, {len(faces)} tris)")

if __name__=="__main__":
    main()
