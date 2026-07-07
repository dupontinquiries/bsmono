#!/usr/bin/env python3
"""gen_props.py -- smooth-shaded 3D prop meshes (IQM) ported from logscope
mesh3d.rs: UV spheres, faceted rocks (smoothly displaced spheres with smooth
normals), tapered cylinders (spires/columns), and domes. These replace box
primitives for map decoration so surfaces are ROUND and lit with smooth
vertex normals -- the single biggest step away from a greybox look.

Emits: rock.iqm, boulder.iqm, spire.iqm (tapered), column.iqm (crystal),
dome.iqm, pillar.iqm. Textured; the renderer lights them via vertex normals.
"""
import struct, os, math, random

DATA = os.path.join(os.path.dirname(__file__), "..", "engine_src", "data")

_AABB_ROWS = []
def _record_aabb(path, verts):
    # local axis-aligned half-extents of the mesh (programmatic bounding box)
    import os
    if not verts: return
    xs=[v[0] for v in verts]; ys=[v[1] for v in verts]; zs=[v[2] for v in verts]
    hx=(max(xs)-min(xs))*0.5; hy=(max(ys)-min(ys))*0.5; hz=(max(zs)-min(zs))*0.5
    cx=(max(xs)+min(xs))*0.5; cy=(max(ys)+min(ys))*0.5; cz=(max(zs)+min(zs))*0.5
    name=os.path.splitext(os.path.basename(path))[0]
    _AABB_ROWS.append((name,cx,cy,cz,hx,hy,hz))
def flush_aabb(data_dir):
    import os
    path=os.path.join(data_dir,"asset_aabb.txt")
    with open(path,"w") as f:
        for (name,cx,cy,cz,hx,hy,hz) in _AABB_ROWS:
            f.write("%s %.4f %.4f %.4f %.4f %.4f %.4f\n"%(name,cx,cy,cz,hx,hy,hz))
    print("wrote", path, len(_AABB_ROWS), "asset AABBs")

def write_iqm(path, verts, norms, uvs, tris, tex):
    def u(*a): return struct.pack("<"+"I"*len(a), *a)
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
    for uv in uvs: out+=struct.pack("<2f",*uv)
    for n in norms: out+=struct.pack("<3f",*n)
    for t in tris: out+=u(*t)
    _record_aabb(path, verts)
    assert len(out)==filesize,(len(out),filesize)
    open(path,"wb").write(out); print(f"  {os.path.basename(path)}: {nv} verts {nt} tris")

def normalize(v):
    l=math.sqrt(v[0]**2+v[1]**2+v[2]**2)
    return (v[0]/l, v[1]/l, v[2]/l) if l>1e-6 else (0,1,0)

def uv_sphere(r=1.0, segs=20, rings=14, squashy=1.0):
    verts=[]; norms=[]; uvs=[]; tris=[]
    for i in range(rings+1):
        phi=math.pi*i/rings; sp=math.sin(phi); cp=math.cos(phi)
        for j in range(segs+1):
            th=2*math.pi*j/segs
            n=(sp*math.cos(th), cp, sp*math.sin(th))
            verts.append((n[0]*r, n[1]*r*squashy, n[2]*r))
            norms.append(n)
            uvs.append((j/segs*2.0, i/rings*2.0))
    row=segs+1
    for i in range(rings):
        for j in range(segs):
            a=i*row+j; c=(i+1)*row+j
            tris.append((a,c,a+1)); tris.append((a+1,c,c+1))
    return verts,norms,uvs,tris

def faceted_rock(r=1.0, seed=1, segs=20, rings=14):
    verts,_,uvs,tris = uv_sphere(r, segs, rings)
    s=seed*0.137
    pv=[]
    for p in verts:
        d=normalize(p)
        lump=(0.50*math.sin(d[0]*3.1+s)*math.cos(d[1]*2.3+s*1.7)
              +0.34*math.sin(d[1]*2.7+s*0.6)
              +0.24*math.sin(d[2]*3.5+s*2.1)*math.cos(d[1]*1.9+s))
        rr=r*(1.0+0.13*lump)
        pv.append((d[0]*rr, d[1]*rr*0.82, d[2]*rr))
    # smooth normals over shared topology
    nrm=[[0,0,0] for _ in pv]
    for t in tris:
        a,b,c=pv[t[0]],pv[t[1]],pv[t[2]]
        e1=(b[0]-a[0],b[1]-a[1],b[2]-a[2]); e2=(c[0]-a[0],c[1]-a[1],c[2]-a[2])
        nf=(e1[1]*e2[2]-e1[2]*e2[1], e1[2]*e2[0]-e1[0]*e2[2], e1[0]*e2[1]-e1[1]*e2[0])
        for vi in t:
            nrm[vi][0]+=nf[0]; nrm[vi][1]+=nf[1]; nrm[vi][2]+=nf[2]
    nrm=[normalize(n) for n in nrm]
    return pv,nrm,uvs,tris

def tapered_cyl(r0=1.0, r1=0.4, h=3.0, segs=16, rings=6, twist=0.0, bulge=0.0):
    """A smooth tapered column (spire/crystal). rings for smooth side normals."""
    verts=[]; norms=[]; uvs=[]; tris=[]
    for i in range(rings+1):
        t=i/rings
        r=r0+(r1-r0)*t + bulge*math.sin(t*math.pi)   # optional bulge
        y=h*t
        for j in range(segs+1):
            th=2*math.pi*j/segs + twist*t
            c,sn=math.cos(th),math.sin(th)
            verts.append((r*c, y, r*sn))
            # side normal (approx, accounting for taper)
            slope=(r0-r1)/h
            n=normalize((c, slope, sn))
            norms.append(n); uvs.append((j/segs*2.0, t*2.0))
    row=segs+1
    for i in range(rings):
        for j in range(segs):
            a=i*row+j; c=(i+1)*row+j
            tris.append((a,c,a+1)); tris.append((a+1,c,c+1))
    # cap the top with a fan
    top_center=len(verts); verts.append((0,h,0)); norms.append((0,1,0)); uvs.append((0.5,0.5))
    base=rings*row
    for j in range(segs):
        tris.append((top_center, base+j, base+j+1))
    return verts,norms,uvs,tris

# faceted crystal: low-segment tapered column reads as angular gem
def crystal(r0=1.0, h=4.0, segs=6):
    return tapered_cyl(r0, r0*0.15, h, segs=segs, rings=3, twist=0.3)

def dome(r=1.0, segs=20, rings=8):
    verts=[]; norms=[]; uvs=[]; tris=[]
    for i in range(rings+1):
        phi=(math.pi/2)*i/rings; sp=math.sin(phi); cp=math.cos(phi)
        for j in range(segs+1):
            th=2*math.pi*j/segs
            n=(sp*math.cos(th), cp, sp*math.sin(th))
            verts.append((n[0]*r, n[1]*r, n[2]*r)); norms.append(n); uvs.append((j/segs*2,i/rings))
    row=segs+1
    for i in range(rings):
        for j in range(segs):
            a=i*row+j; c=(i+1)*row+j
            tris.append((a,c,a+1)); tris.append((a+1,c,c+1))
    return verts,norms,uvs,tris

# generate the prop set
v,n,uv,t = faceted_rock(1.0, 3); write_iqm(os.path.join(DATA,"rock.iqm"), v,n,uv,t, "rock.png")
v,n,uv,t = faceted_rock(1.0, 17, 24, 18); write_iqm(os.path.join(DATA,"boulder.iqm"), v,n,uv,t, "asteroid.png")
v,n,uv,t = tapered_cyl(1.0, 0.25, 3.0); write_iqm(os.path.join(DATA,"spire.iqm"), v,n,uv,t, "rock.png")
v,n,uv,t = crystal(1.0, 4.0, 6); write_iqm(os.path.join(DATA,"column.iqm"), v,n,uv,t, "white.png")
v,n,uv,t = tapered_cyl(0.8, 0.8, 4.0, segs=20, rings=6); write_iqm(os.path.join(DATA,"pillar.iqm"), v,n,uv,t, "cybertron.png")
v,n,uv,t = dome(1.0); write_iqm(os.path.join(DATA,"dome.iqm"), v,n,uv,t, "plate_blue.png")
print("props generated")

# ---- character part meshes (smooth, articulated robot) ----
def sphere_mesh(r=1.0, segs=16, rings=12):
    return uv_sphere(r, segs, rings)

def bevel_box(hx=1.0,hy=1.0,hz=1.0,bevel=0.16):
    """A chamfered cube: octagonal cross-sections -> reads as machined metal,
    not a raw cube. Built as a low-res rounded box via corner insets."""
    # 8 corners pulled in by bevel, plus we just approximate with a scaled
    # rounded shape: use a subdivided box with slightly spherified corners.
    verts=[]; norms=[]; uvs=[]; tris=[]
    N=4  # subdivisions per face edge
    # generate a spherified cube (cube mapped toward sphere by `bevel`)
    faces=[
        ((1,0,0),(0,1,0),(0,0,1)),  # +x
        ((-1,0,0),(0,1,0),(0,0,-1)),
        ((0,1,0),(0,0,1),(1,0,0)),  # +y
        ((0,-1,0),(0,0,-1),(1,0,0)),
        ((0,0,1),(1,0,0),(0,1,0)),  # +z
        ((0,0,-1),(-1,0,0),(0,1,0)),
    ]
    for (nrm,uax,vax) in faces:
        base=len(verts)
        for i in range(N+1):
            for j in range(N+1):
                a=(i/N*2-1); b=(j/N*2-1)
                # cube point
                p=[nrm[k]+uax[k]*a+vax[k]*b for k in range(3)]
                # spherify by bevel amount
                l=math.sqrt(p[0]**2+p[1]**2+p[2]**2)
                sp=[p[k]/l for k in range(3)]
                q=[p[k]*(1-bevel)+sp[k]*bevel for k in range(3)]
                verts.append((q[0]*hx, q[1]*hy, q[2]*hz))
                nn=normalize((q[0],q[1],q[2])); norms.append(nn)
                uvs.append((i/N*2.0, j/N*2.0))
        for i in range(N):
            for j in range(N):
                a=base+i*(N+1)+j; c=base+(i+1)*(N+1)+j
                tris.append((a,c,a+1)); tris.append((a+1,c,c+1))
    return verts,norms,uvs,tris

def limb(r0=0.5,r1=0.42,h=2.0,segs=14):
    """A smooth, slightly tapered capped cylinder for arms/legs, CENTERED at y=0
    (spans -h/2..+h/2) so it drops into a part slot like a unit box."""
    v,n,uv,t = tapered_cyl(r0,r1,h,segs=segs,rings=4)
    base=len(v); v.append((0,0,0)); n.append((0,-1,0)); uv.append((0.5,0.5))
    for j in range(segs):
        t.append((base, j+1, j))
    # recenter and normalize to unit half-height so scale3=half-extent works
    v=[(px, (py - h/2)/(h/2), pz) for (px,py,pz) in v]
    return v,n,uv,t

v,n,uv,t = sphere_mesh(1.0,16,12); write_iqm(os.path.join(DATA,"psphere.iqm"), v,n,uv,t, "wall.png")
v,n,uv,t = bevel_box(1,1,1,0.20);  write_iqm(os.path.join(DATA,"pbevel.iqm"), v,n,uv,t, "wall.png")
v,n,uv,t = limb(1.0,0.8,2.0);      write_iqm(os.path.join(DATA,"plimb.iqm"), v,n,uv,t, "wall.png")
print("character part meshes generated")

# ---- arch: a standing half-torus (archway / monument landmark) ----
def arch(big_r=3.0, tube_r=0.5, nmaj=20, nmin=10, span=math.pi):
    """Half (or part) torus standing in the XY plane -> an archway."""
    verts=[]; norms=[]; uvs=[]; tris=[]
    for i in range(nmaj+1):
        a = span * i/nmaj            # 0..pi over the top
        # center of the tube ring, tracing a semicircle standing up
        cx = math.cos(a)*big_r; cy = math.sin(a)*big_r; cz=0.0
        # ring basis: tangent along the arc, plus z
        tang=(-math.sin(a), math.cos(a), 0.0)
        for j in range(nmin+1):
            b=2*math.pi*j/nmin
            # ring point: radial in the plane perp to tangent (use z and in-plane normal)
            rn=(math.cos(a), math.sin(a), 0.0)          # radial (in-plane)
            nx=rn[0]*math.cos(b); ny=rn[1]*math.cos(b); nz=math.sin(b)
            verts.append((cx+nx*tube_r, cy+ny*tube_r, cz+nz*tube_r))
            norms.append(normalize((nx,ny,nz)))
            uvs.append((i/nmaj*3, j/nmin))
    row=nmin+1
    for i in range(nmaj):
        for j in range(nmin):
            a0=i*row+j; c=(i+1)*row+j
            tris.append((a0,c,a0+1)); tris.append((a0+1,c,c+1))
    return verts,norms,uvs,tris

v,n,uv,t = arch(3.0,0.55,22,10); write_iqm(os.path.join(DATA,"arch.iqm"), v,n,uv,t, "cybertron.png")
print("arch landmark generated")

# ---- weapon: an arm-cannon (breech body + tapered barrel + muzzle ring) ----
def weapon_cannon():
    verts=[]; norms=[]; uvs=[]; tris=[]
    def addmesh(vv,nn,uu,tt):
        base=len(verts)
        verts.extend(vv); norms.extend(nn); uvs.extend(uu)
        for t in tt: tris.append((t[0]+base,t[1]+base,t[2]+base))
    # barrel: tapered cylinder along +z (forward). tapered_cyl builds along +y,
    # so rotate y->z.
    bv,bn,bu,bt = tapered_cyl(0.5,0.32,2.2,segs=12,rings=3)
    bv=[(x, z, y) for (x,y,z) in bv]         # y<->z : barrel points forward
    bn=[(x, z, y) for (x,y,z) in bn]
    addmesh(bv,bn,bu,bt)
    # breech body (a chamfered block at the base)
    cv,cn,cu,ct = bevel_box(0.55,0.5,0.6,0.25)
    cv=[(x, y, z-0.5) for (x,y,z) in cv]     # sit behind the barrel
    addmesh(cv,cn,cu,ct)
    # muzzle brake (ring at the tip)
    mv,mn,mu,mt = uv_sphere(0.36,10,6)
    mv=[(x, y, z+2.0) for (x,y,z) in mv]
    addmesh(mv,mn,mu,mt)
    # top sight rail (a thin raised bar along the barrel)
    rv,rn,ru,rt = bevel_box(0.10,0.09,0.9,0.15)
    rv=[(x, y+0.42, z+0.5) for (x,y,z) in rv]
    addmesh(rv,rn,ru,rt)
    # front sight post
    sv,sn,su,st = bevel_box(0.06,0.16,0.06,0.1)
    sv=[(x, y+0.5, z+1.5) for (x,y,z) in sv]
    addmesh(sv,sn,su,st)
    # heat-vent ridges (three short fins near the breech)
    for zz in (0.0, 0.28, 0.56):
        fv,fn,fu,ft = bevel_box(0.42,0.06,0.05,0.1)
        fv=[(x, y+0.30, z+zz) for (x,y,z) in fv]
        addmesh(fv,fn,fu,ft)
    # underslung grip
    gv,gn,gu,gt = bevel_box(0.14,0.34,0.20,0.15)
    gv=[(x, y-0.5, z-0.35) for (x,y,z) in gv]
    addmesh(gv,gn,gu,gt)
    return verts,norms,uvs,tris

v,n,uv,t = weapon_cannon(); write_iqm(os.path.join(DATA,"weapon.iqm"), v,n,uv,t, "plate_blue.png")
print("weapon model generated")

# ---- weapon variants by fire mode (breech + form-specific emitter) ----
def _breech():
    cv,cn,cu,ct = bevel_box(0.55,0.5,0.6,0.25)
    return [(x,y,z-0.5) for (x,y,z) in cv], cn, cu, ct

def _combine(parts):
    verts=[];norms=[];uvs=[];tris=[]
    for (vv,nn,uu,tt) in parts:
        base=len(verts); verts.extend(vv);norms.extend(nn);uvs.extend(uu)
        for t in tt: tris.append((t[0]+base,t[1]+base,t[2]+base))
    return verts,norms,uvs,tris

def _barrel(r0,r1,length,off=(0,0,0),segs=10):
    bv,bn,bu,bt = tapered_cyl(r0,r1,length,segs=segs,rings=2)
    bv=[(x+off[0], z+off[2], y+off[1]) for (x,y,z) in bv]   # point +z
    bn=[(x, z, y) for (x,y,z) in bn]
    return bv,bn,bu,bt

def weapon_burst():  # three thin barrels
    return _combine([_breech(),
        _barrel(0.18,0.14,2.2,(-0.22,0,0)), _barrel(0.18,0.14,2.2,(0.22,0,0)),
        _barrel(0.18,0.14,2.4,(0,0.22,0))])
def weapon_beam():   # wide focusing emitter (cone + ring)
    tip=uv_sphere(0.5,12,7); tip=([(x,y,z+2.1) for(x,y,z) in tip[0]],tip[1],tip[2],tip[3])
    return _combine([_breech(), _barrel(0.55,0.7,2.0), tip])
def weapon_wave():   # broad flat emitter (wide short barrel)
    flat=bevel_box(0.7,0.28,1.1,0.2); flat=([(x,y,z+0.9) for(x,y,z) in flat[0]],flat[1],flat[2],flat[3])
    return _combine([_breech(), flat])
def weapon_lob():    # stubby angled mortar
    return _combine([_breech(), _barrel(0.42,0.5,1.5,(0,0.25,0))])

for nm,fn in [("weapon_burst",weapon_burst),("weapon_beam",weapon_beam),
              ("weapon_wave",weapon_wave),("weapon_lob",weapon_lob)]:
    v,n,uv,t = fn(); write_iqm(os.path.join(DATA,nm+".iqm"), v,n,uv,t, "plate_blue.png")
print("weapon variants generated")

# ---- broken tower: stacked tapered segments, jagged broken top ----
def tower(seed=1):
    parts=[]; y=0.0; r=1.4
    rng=random.Random(seed)
    nseg=rng.randint(3,4)
    for k in range(nseg):
        h=rng.uniform(2.0,3.2); r1=r*rng.uniform(0.72,0.85)
        sv,sn,su,st = tapered_cyl(r, r1, h, segs=12, rings=3)
        sv=[(x, y+yy, z) for (x,yy,z) in sv]
        parts.append((sv,sn,su,st))
        y+=h; r=r1
    # jagged broken crown: a few tilted shards on top
    for j in range(4):
        a=j/4*2*math.pi
        pv,pn,pu,pt = tapered_cyl(0.25,0.05, rng.uniform(0.8,1.6), segs=5, rings=2)
        pv=[(x+math.cos(a)*r*0.6, y+yy, z+math.sin(a)*r*0.6) for (x,yy,z) in pv]
        parts.append((pv,pn,pu,pt))
    # combine
    verts=[];norms=[];uvs=[];tris=[]
    for (vv,nn,uu,tt) in parts:
        base=len(verts); verts.extend(vv);norms.extend(nn);uvs.extend(uu)
        for t in tt: tris.append((t[0]+base,t[1]+base,t[2]+base))
    return verts,norms,uvs,tris

v,n,uv,t = tower(7); write_iqm(os.path.join(DATA,"tower.iqm"), v,n,uv,t, "wall.png")
print("tower landmark generated")

# ---- spaceship: sleek hull + wings + engines + cockpit (menu backdrop) ----
def spaceship():
    def _combine(parts):
        verts=[];norms=[];uvs=[];tris=[]
        for (vv,nn,uu,tt) in parts:
            base=len(verts); verts.extend(vv);norms.extend(nn);uvs.extend(uu)
            for t in tt: tris.append((t[0]+base,t[1]+base,t[2]+base))
        return verts,norms,uvs,tris
    def shift(m,dx,dy,dz):
        return ([(x+dx,y+dy,z+dz) for(x,y,z) in m[0]],m[1],m[2],m[3])
    def scale(m,sx,sy,sz):
        return ([(x*sx,y*sy,z*sz) for(x,y,z) in m[0]],m[1],m[2],m[3])
    parts=[]
    # main hull: a long tapered spindle along z
    hull=uv_sphere(1.0,18,12); hull=scale(hull,1.0,0.7,3.2); parts.append(hull)
    # nose cone
    nose=tapered_cyl(0.7,0.05,2.0,segs=14,rings=3)
    nose=([(x,z,y) for(x,y,z) in nose[0]],[(x,z,y) for(x,y,z) in nose[1]],nose[2],nose[3])
    parts.append(shift(nose,0,0,3.0))
    # cockpit bump
    cock=uv_sphere(0.5,12,8); cock=scale(cock,0.8,0.7,1.0); parts.append(shift(cock,0,0.5,1.4))
    # swept wings (flattened boxes angled back)
    wl=bevel_box(2.2,0.12,1.0,0.15); parts.append(shift(wl,-2.0,-0.1,-0.6))
    wr=bevel_box(2.2,0.12,1.0,0.15); parts.append(shift(wr, 2.0,-0.1,-0.6))
    # twin engine nacelles at the rear
    e1=tapered_cyl(0.45,0.5,1.4,segs=10,rings=2); e1=([(x,z,y) for(x,y,z) in e1[0]],[(x,z,y) for(x,y,z) in e1[1]],e1[2],e1[3])
    parts.append(shift(e1,-0.8,-0.1,-3.2))
    e2=tapered_cyl(0.45,0.5,1.4,segs=10,rings=2); e2=([(x,z,y) for(x,y,z) in e2[0]],[(x,z,y) for(x,y,z) in e2[1]],e2[2],e2[3])
    parts.append(shift(e2, 0.8,-0.1,-3.2))
    return _combine(parts)

v,n,uv,t = spaceship(); write_iqm(os.path.join(DATA,"ship.iqm"), v,n,uv,t, "plate_blue.png")

# engine glow disc (unlit, for the ship's thrusters)
v,n,uv,t = uv_sphere(1.0,10,6); write_iqm(os.path.join(DATA,"glowdisc.iqm"), v,n,uv,t, "white.png")
print("spaceship generated")

def crater_disc(segs=44, rings=11, seed=7):
    import random
    rnd=random.Random(seed)
    verts=[]; uvs=[]; tris=[]
    def height(rr, th):
        if rr < 0.82:
            base = -0.34*(1.0-(rr/0.82)) + 0.10*(rr/0.82)**2   # bowl rising to rim
        else:
            base = 0.16*(1.0-(rr-0.82)/0.18)                    # raised rim falling to 0
        noise = 0.06*math.sin(th*7+rr*9) + 0.045*math.sin(th*13-rr*5) + (rnd.random()-0.5)*0.06
        return base + noise*min(rr*2.0,1.0)
    for i in range(rings+1):
        rr=i/rings
        for j in range(segs+1):
            th=2*math.pi*j/segs
            verts.append((math.cos(th)*rr, height(rr,th) if rr>0.001 else -0.34, math.sin(th)*rr))
            uvs.append((j/segs, i/rings))
    row=segs+1
    for i in range(rings):
        for j in range(segs):
            a=i*row+j; c=(i+1)*row+j
            tris.append((a,c,a+1)); tris.append((a+1,c,c+1))
    norms=[[0,0,0] for _ in verts]
    for t in tris:
        va,vb,vc=verts[t[0]],verts[t[1]],verts[t[2]]
        e1=(vb[0]-va[0],vb[1]-va[1],vb[2]-va[2]); e2=(vc[0]-va[0],vc[1]-va[1],vc[2]-va[2])
        nx=e1[1]*e2[2]-e1[2]*e2[1]; ny=e1[2]*e2[0]-e1[0]*e2[2]; nz=e1[0]*e2[1]-e1[1]*e2[0]
        for idx in t: norms[idx][0]+=nx; norms[idx][1]+=ny; norms[idx][2]+=nz
    nout=[]
    for n in norms:
        l=math.sqrt(n[0]*n[0]+n[1]*n[1]+n[2]*n[2]) or 1.0
        nout.append((n[0]/l,n[1]/l,n[2]/l))
    return verts,nout,uvs,tris

v,n,uv,t = crater_disc(); write_iqm(os.path.join(DATA,"crater.iqm"), v,n,uv,t, "rock.png")
print("crater mesh generated: %d verts %d tris" % (len(v), len(t)))


# ---- defensive turret: squat base + head + forward barrel (rotates as a unit) ----
def def_turret():
    def _combine(parts):
        verts=[];norms=[];uvs=[];tris=[]
        for (vv,nn,uu,tt) in parts:
            base=len(verts); verts.extend(vv);norms.extend(nn);uvs.extend(uu)
            for t in tt: tris.append((t[0]+base,t[1]+base,t[2]+base))
        return verts,norms,uvs,tris
    def shift(m,dx,dy,dz): return ([(x+dx,y+dy,z+dz) for(x,y,z) in m[0]],m[1],m[2],m[3])
    parts=[]
    base=tapered_cyl(1.1,0.9,0.8,segs=12,rings=2); parts.append(base)            # squat base
    head=bevel_box(0.8,0.6,0.9,0.2); parts.append(shift(head,0,1.2,0))           # head
    # forward barrel (+z)
    bar=tapered_cyl(0.28,0.22,1.6,segs=8,rings=2)
    bar=([(x,z,y) for(x,y,z) in bar[0]],[(x,z,y) for(x,y,z) in bar[1]],bar[2],bar[3])
    parts.append(shift(bar,0,1.2,0.7))
    return _combine(parts)

v,n,uv,t=def_turret(); write_iqm(os.path.join(DATA,"turret_def.iqm"), v,n,uv,t, "plate_blue.png")
print("defensive turret model generated")

# ---- proper robot HEAD: angular skull + faceplate + crest + antennae + jaw ----
def robot_head():
    def comb(parts):
        v=[];n=[];u=[];t=[]
        for (vv,nn,uu,tt) in parts:
            b=len(v); v.extend(vv);n.extend(nn);u.extend(uu)
            for tr in tt: t.append((tr[0]+b,tr[1]+b,tr[2]+b))
        return v,n,u,t
    def sh(m,dx,dy,dz): return ([(x+dx,y+dy,z+dz) for(x,y,z) in m[0]],m[1],m[2],m[3])
    P=[]
    # cranium (rounded-ish box, centered so scale3=half works: unit-ish)
    P.append(bevel_box(0.85,0.8,0.82,0.28))
    # angled cheek panels flanking the face (give the skull real structure)
    P.append(sh(bevel_box(0.16,0.5,0.4,0.1), -0.72,-0.16,0.34))
    P.append(sh(bevel_box(0.16,0.5,0.4,0.1),  0.72,-0.16,0.34))
    # faceplate (a slightly protruding lower front block)
    P.append(sh(bevel_box(0.6,0.42,0.2,0.14), 0,-0.28,0.7))
    # recessed visor band (thin, set into the brow -- reads as the eyes)
    P.append(sh(bevel_box(0.52,0.10,0.10,0.03), 0,0.02,0.80))
    # brow ridge over the visor
    P.append(sh(bevel_box(0.7,0.12,0.18,0.1), 0,0.20,0.72))
    # cheek vents (small stacked fins on each side of the faceplate)
    for vy in (-0.18,-0.34):
        P.append(sh(bevel_box(0.12,0.05,0.14,0.02), -0.42,vy,0.66))
        P.append(sh(bevel_box(0.12,0.05,0.14,0.02),  0.42,vy,0.66))
    # crest fin on top (center) + two flanking crown ridges
    P.append(sh(bevel_box(0.08,0.42,0.5,0.06), 0,0.85,0.0))
    P.append(sh(bevel_box(0.05,0.24,0.34,0.04), -0.34,0.80,-0.05))
    P.append(sh(bevel_box(0.05,0.24,0.34,0.04),  0.34,0.80,-0.05))
    # two side antennae
    P.append(sh(tapered_cyl(0.07,0.03,0.5,segs=6,rings=2), -0.7,0.6,0))
    P.append(sh(tapered_cyl(0.07,0.03,0.5,segs=6,rings=2),  0.7,0.6,0))
    # defined chin/jaw (narrower, pushed forward for a determined profile)
    P.append(sh(bevel_box(0.42,0.16,0.42,0.12), 0,-0.62,0.32))
    v,n,u,t = comb(P)
    # normalize to unit half-extent (~1) so the part slot scaling matches
    return v,n,u,t

v,n,uv,t = robot_head(); write_iqm(os.path.join(DATA,"phead.iqm"), v,n,uv,t, "plate_blue.png")

# ---- mechanical HAND: blocky palm + knuckles + thumb ----
def robot_hand():
    def comb(parts):
        v=[];n=[];u=[];t=[]
        for (vv,nn,uu,tt) in parts:
            b=len(v); v.extend(vv);n.extend(nn);u.extend(uu)
            for tr in tt: t.append((tr[0]+b,tr[1]+b,tr[2]+b))
        return v,n,u,t
    def sh(m,dx,dy,dz): return ([(x+dx,y+dy,z+dz) for(x,y,z) in m[0]],m[1],m[2],m[3])
    P=[]
    P.append(bevel_box(0.6,0.7,0.5,0.2))                         # palm
    for i,dx in enumerate((-0.34,-0.11,0.11,0.34)):             # 4 knuckles
        P.append(sh(bevel_box(0.13,0.34,0.16,0.08), dx,-0.7,0.18))
    P.append(sh(bevel_box(0.16,0.30,0.16,0.08), 0.55,-0.1,0.1))  # thumb
    return comb(P)

v,n,uv,t = robot_hand(); write_iqm(os.path.join(DATA,"phand.iqm"), v,n,uv,t, "plate_blue.png")

# ---- robot CHEST: an armored cockpit -- central windshield-like plate, angled
#      pectoral panels, a collar, and side intake vents. Reads as a torso with
#      design instead of a plain bevel box. (all original geometry) ----
def robot_chest():
    def comb(parts):
        v=[];n=[];u=[];t=[]
        for (vv,nn,uu,tt) in parts:
            b=len(v); v.extend(vv);n.extend(nn);u.extend(uu)
            for tr in tt: t.append((tr[0]+b,tr[1]+b,tr[2]+b))
        return v,n,u,t
    def sh(m,dx,dy,dz): return ([(x+dx,y+dy,z+dz) for(x,y,z) in m[0]],m[1],m[2],m[3])
    P=[]
    P.append(bevel_box(1.0,0.95,0.62,0.20))                       # core torso block
    # angled pectoral panels (raised, catching the light on each breast)
    P.append(sh(bevel_box(0.42,0.5,0.18,0.10), -0.42,0.18,0.60))
    P.append(sh(bevel_box(0.42,0.5,0.18,0.10),  0.42,0.18,0.60))
    # central chest plate (the "cockpit" windshield block, protruding)
    P.append(sh(bevel_box(0.34,0.46,0.22,0.08), 0,-0.02,0.66))
    # collar ridge across the top
    P.append(sh(bevel_box(0.9,0.14,0.3,0.08), 0,0.82,0.30))
    # side intake vents
    for vy in (0.30,0.08,-0.14):
        P.append(sh(bevel_box(0.10,0.06,0.30,0.02), -0.92,vy,0.20))
        P.append(sh(bevel_box(0.10,0.06,0.30,0.02),  0.92,vy,0.20))
    # lower abdominal band
    P.append(sh(bevel_box(0.7,0.18,0.5,0.10), 0,-0.78,0.10))
    return comb(P)

v,n,uv,t = robot_chest(); write_iqm(os.path.join(DATA,"pchest.iqm"), v,n,uv,t, "plate_blue.png")
print("robot head + hand + chest meshes generated")

flush_aabb(DATA)
