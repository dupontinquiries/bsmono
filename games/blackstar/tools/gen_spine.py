#!/usr/bin/env python3
"""gen_spine.py -- HEADLESS spine-growth -> IQM generator.

A command-line port of the interactive spine_grower.html: it speaks the same
growth vocabulary (grow / steer / twist / thick / thin / pinch / flare / ridge /
wobble / node / branch / parent / mirror / section) as a small script or a named
preset, lofts the swept rings into a tube mesh, and writes a real IQM v2 static
model in the EXACT byte layout the engine already loads (identical to
gen_props.write_iqm / gen_box.py, both proven in-game).

The output is recentered and normalized so the longest axis spans 2 (half-extent
1) -- i.e. it drops into a robot part slot the same way box.iqm / plimb.iqm do,
so the pose system's per-axis scale maps cleanly.

Fixes carried over from the earlier CLI attempt:
  * multi-char tokens (parent, ridge, pinch...) parse as OPS, never as floats
  * section swaps mid-spine don't crash the bake -- each spine is split into
    runs of equal ring-length and lofted within runs (with end caps), so a
    square->circle transition becomes a clean mechanical joint
  * mirror twins reflect across X and FLIP winding so their normals face out

Usage:
  python3 tools/gen_spine.py --preset muscle       --out data/spine_muscle.iqm
  python3 tools/gen_spine.py --preset wing_cannon   --out data/spine_wingcannon.iqm
  python3 tools/gen_spine.py --script "sec circle rad 0.4 grow 20 twist 0.8 grow 20" \
                             --out data/spine_custom.iqm
  python3 tools/gen_spine.py --list-presets
"""
import struct, os, math, argparse, sys

# ------------------------------------------------------------------ vec3 math
def vadd(a,b): return (a[0]+b[0], a[1]+b[1], a[2]+b[2])
def vsub(a,b): return (a[0]-b[0], a[1]-b[1], a[2]-b[2])
def vscale(a,s): return (a[0]*s, a[1]*s, a[2]*s)
def vdot(a,b): return a[0]*b[0]+a[1]*b[1]+a[2]*b[2]
def vcross(a,b): return (a[1]*b[2]-a[2]*b[1], a[2]*b[0]-a[0]*b[2], a[0]*b[1]-a[1]*b[0])
def vlen(a): return math.sqrt(vdot(a,a))
def vnorm(a):
    l=vlen(a)
    return (a[0]/l, a[1]/l, a[2]/l) if l>1e-9 else (0.0,1.0,0.0)
def vrot(v, axis, ang):
    """Rodrigues rotation of v about (normalized) axis by ang radians."""
    k=vnorm(axis); c=math.cos(ang); s=math.sin(ang)
    return vnorm(vadd(vadd(vscale(v,c), vscale(vcross(k,v),s)),
                      vscale(k, vdot(k,v)*(1.0-c))))

# ------------------------------------------------------------ cross-sections
# 2D (x,y) point lists, unit-ish radius -- ported 1:1 from spine_grower.html.
def sec_circle(seg): return [(math.cos(i/seg*2*math.pi), math.sin(i/seg*2*math.pi)) for i in range(seg)]
def sec_square(seg): return [(1,1),(-1,1),(-1,-1),(1,-1)]
def sec_tri(seg):    return [(0,1.15),(-1,-0.8),(1,-0.8)]
def sec_hex(seg):    return [(math.cos(i/6*2*math.pi), math.sin(i/6*2*math.pi)) for i in range(6)]
def sec_star(seg):
    p=[]
    for i in range(10):
        a=i/10*2*math.pi; r=0.5 if i%2 else 1.1
        p.append((math.cos(a)*r, math.sin(a)*r))
    return p
def sec_oct(seg):    return [(math.cos(i/8*2*math.pi), math.sin(i/8*2*math.pi)) for i in range(8)]
def sec_diamond(seg):return [(0,1.25),(1.0,0),(0,-1.25),(-1.0,0)]
def sec_blade(seg):  return [(1.4,0.0),(0.45,0.16),(-0.45,0.16),(-1.4,0.0),(-0.45,-0.16),(0.45,-0.16)]
def sec_plus(seg):
    return [(0.35,1),(-0.35,1),(-0.35,0.35),(-1,0.35),(-1,-0.35),(-0.35,-0.35),
            (-0.35,-1),(0.35,-1),(0.35,-0.35),(1,-0.35),(1,0.35),(0.35,0.35)]
def sec_arrow(seg):
    return [(1.4,0.0),(0.2,0.5),(0.2,0.2),(-1.0,0.2),(-1.0,-0.2),(0.2,-0.2),(0.2,-0.5)]
def sec_gear(seg):
    teeth=10; p=[]
    for i in range(teeth*2):
        a=i/(teeth*2)*2*math.pi; r=1.0 if i%2==0 else 0.74
        p.append((math.cos(a)*r, math.sin(a)*r))
    return p
SECTIONS = {"circle":sec_circle,"square":sec_square,"tri":sec_tri,"star":sec_star,"hex":sec_hex,
            "oct":sec_oct,"diamond":sec_diamond,"blade":sec_blade,"plus":sec_plus,"gear":sec_gear,"arrow":sec_arrow}
SEC_BY_NUM = {"1":"circle","2":"square","3":"tri","4":"star","5":"hex","6":"oct","7":"diamond","8":"blade","9":"plus","0":"gear"}

# growth constants (tuned so a CLI "grow N" reads like the interactive feel)
SEGLEN     = 0.16   # advance per grow-step
WOB_STEP   = 0.35   # wobble phase advance per grow-step
WOB_AMP    = 0.05
TWIST_DT   = 0.16   # twistAccum += twist * TWIST_DT per grow-step
BRANCH_KICK= 0.6    # default child fork angle (radians) ~= 34 deg

class Spine:
    def __init__(self, parent=None):
        if parent:
            self.pos=parent.pos; self.dir=parent.dir; self.up=parent.up
            self.radius=parent.radius*0.7; self.twist=parent.twist
            self.wobble=parent.wobble; self.section=parent.section
            self.taper=parent.taper; self.ridge=parent.ridge; self.seg=parent.seg
            self.twistAccum=parent.twistAccum
            self.scale_x=parent.scale_x; self.scale_y=parent.scale_y; self.bend=parent.bend
        else:
            self.pos=(0.0,0.6,0.0); self.dir=(0.0,0.0,1.0); self.up=(0.0,1.0,0.0)
            self.radius=0.35; self.twist=0.0; self.wobble=False; self.section="circle"
            self.taper=1.0; self.ridge=0.0; self.seg=8; self.twistAccum=0.0
            self.scale_x=1.0; self.scale_y=1.0; self.bend=0.0
        self.wob=0.0
        self.rings=[]

    def push_ring(self, extra=1.0):
        side=vnorm(vcross(self.dir,self.up))
        up2 =vnorm(vcross(side,self.dir))
        sec=SECTIONS[self.section](self.seg)
        r=self.radius*self.taper*extra
        ca=math.cos(self.twistAccum); sa=math.sin(self.twistAccum)
        pts=[]
        for (x,y) in sec:
            rx=x*ca-y*sa; ry=x*sa+y*ca
            rx*=self.scale_x; ry*=self.scale_y
            spike=(1.0+self.ridge) if (self.ridge>0 and abs(rx)<0.35) else 1.0
            p=vadd(vadd(self.pos, vscale(side, rx*r*spike)), vscale(up2, ry*r))
            pts.append(p)
        self.rings.append(pts)

    def grow_step(self):
        if self.wobble:
            self.wob+=WOB_STEP
            self.dir=vrot(self.dir, self.up, math.sin(self.wob)*WOB_AMP)
        if self.bend: self.dir=vrot(self.dir, self.up, math.radians(self.bend))
        self.twistAccum+=self.twist*TWIST_DT
        self.pos=vadd(self.pos, vscale(self.dir, SEGLEN))
        self.push_ring()

# ------------------------------------------------------------------- engine
class Grower:
    def __init__(self):
        self.spines=[Spine()]
        self.stack=[]
        self.active=self.spines[0]
        self.mirror=False
        self.radial=1             # N-fold radial symmetry around Y (1 = none)
        self.active.push_ring()   # seed first ring

    def run(self, tokens):
        i=0; T=tokens
        def num(default=None):
            nonlocal i
            if i<len(T):
                try:
                    v=float(T[i]); i+=1; return v
                except ValueError:
                    pass
            if default is None:
                raise SystemExit(f"gen_spine: op '{T[i-1]}' expected a number, got '{T[i] if i<len(T) else 'EOF'}'")
            return default
        def word(default=None):
            nonlocal i
            if i<len(T):
                w=T[i]; i+=1; return w
            return default
        S=self.active
        while i<len(T):
            op=T[i]; i+=1; S=self.active
            if op=="seg":        S.seg=int(num())
            elif op in ("sec","section"):
                w=word(); S.section=SEC_BY_NUM.get(w, w)
                if S.section not in SECTIONS: raise SystemExit(f"unknown section '{w}'")
            elif op in ("rad","radius"): S.radius=num()
            elif op=="grow":
                n=int(num())
                for _ in range(n): S.grow_step()
            elif op=="yaw":   a=math.radians(num()); S.dir=vrot(S.dir,S.up,a)
            elif op=="pitch":
                a=math.radians(num()); side=vnorm(vcross(S.dir,S.up))
                S.dir=vrot(S.dir,side,a); S.up=vrot(S.up,side,a)
            elif op=="roll":  a=math.radians(num()); S.up=vrot(S.up,S.dir,a)
            elif op=="twist": S.twist=num()
            elif op=="thick": S.radius=min(1.4, S.radius+num())
            elif op=="thin":  S.radius=max(0.06,S.radius-num())
            elif op=="taper": S.taper=num()
            elif op=="pinch": S.taper=max(0.25,S.taper-num())
            elif op=="flare": S.taper=min(2.2, S.taper+num())
            elif op=="ridge":
                w=word("toggle")
                if   w=="on":  S.ridge=0.6
                elif w=="off": S.ridge=0.0
                else:
                    try: S.ridge=float(w)
                    except ValueError: S.ridge=0.6 if S.ridge==0 else 0.0
            elif op=="wobble":
                w=word("on"); S.wobble = (w!="off")
            elif op=="node":   S.push_ring(1.8); S.push_ring(1.8)
            elif op=="branch":
                kick=num(math.degrees(BRANCH_KICK))
                child=Spine(S); self.spines.append(child); self.stack.append(S)
                self.active=child
                self.active.dir=vrot(self.active.dir,self.active.up,math.radians(kick))
                self.active.push_ring()
            elif op=="parent":
                if self.stack: self.active=self.stack.pop()
            elif op=="mirror":
                w=word("on"); self.mirror=(w!="off")
            elif op=="radial":
                self.radial=max(1,min(24,int(num(6))))   # N copies fanned around the vertical axis
            elif op=="scale":  S.scale_x=num(1.0); S.scale_y=num(1.0)   # non-uniform cross-section (flat plates, oval tubes)
            elif op=="bend":   S.bend=num(0.0)                          # degrees of curve per grow step
            elif op.startswith("#"):
                # skip rest of a comment line -- tokens already whitespace-split,
                # so consume until we've passed the '#'-token (no-op here)
                pass
            else:
                raise SystemExit(f"gen_spine: unknown op '{op}'")

    # ---------------------------------------------------------------- bake
    def bake(self):
        """Return (verts, norms, uvs, tris) recentered + normalized to unit slot."""
        V=[]; TRI=[]
        def loft_run(rings, flip):
            """Loft a run of equal-length rings into the global V/TRI, with caps."""
            if len(rings)<2: return
            n=len(rings[0]); base0=len(V)
            for r in rings:
                for p in r: V.append(p)
            for a in range(len(rings)-1):
                for j in range(n):
                    i0=base0+a*n+j; i1=base0+a*n+(j+1)%n
                    i2=base0+(a+1)*n+j; i3=base0+(a+1)*n+(j+1)%n
                    if flip: TRI.extend([(i0,i1,i2),(i1,i3,i2)])
                    else:    TRI.extend([(i0,i2,i1),(i1,i2,i3)])
            # end caps (fan from ring centroid) so the tube reads solid
            for which,ring_start in ((0,base0),(1,base0+(len(rings)-1)*n)):
                cx=sum(V[ring_start+k][0] for k in range(n))/n
                cy=sum(V[ring_start+k][1] for k in range(n))/n
                cz=sum(V[ring_start+k][2] for k in range(n))/n
                c=len(V); V.append((cx,cy,cz))
                for j in range(n):
                    a=ring_start+j; b=ring_start+(j+1)%n
                    # front cap and back cap wind opposite ways; mirror flips both
                    if (which==0) ^ bool(flip): TRI.append((c,b,a))
                    else:                       TRI.append((c,a,b))

        def _reflect(rings, mirrored):
            if not mirrored: return rings
            return [[(-p[0],p[1],p[2]) for p in r] for r in rings]

        def _roty(rings, ang):
            if not ang: return rings
            c=math.cos(ang); si=math.sin(ang)
            return [[(p[0]*c+p[2]*si, p[1], -p[0]*si+p[2]*c) for p in r] for r in rings]

        def emit_spine(sp, mirrored, ang):
            if len(sp.rings)<2: return
            def xf(rr): return _roty(_reflect(rr, mirrored), ang)
            # split into runs of equal ring length (handles section swaps)
            run=[sp.rings[0]]
            for r in sp.rings[1:]:
                if len(r)==len(run[-1]): run.append(r)
                else: loft_run(xf(run), mirrored); run=[r]
            loft_run(xf(run), mirrored)

        radial = max(1, getattr(self, "radial", 1))
        for k in range(radial):
            ang = (2*math.pi/radial)*k
            for sp in self.spines:
                emit_spine(sp, False, ang)
                if self.mirror: emit_spine(sp, True, ang)

        if not V: raise SystemExit("gen_spine: nothing grew (empty mesh)")

        # normalize: recenter to bbox center, scale so longest axis spans 2
        xs=[p[0] for p in V]; ys=[p[1] for p in V]; zs=[p[2] for p in V]
        cx=(max(xs)+min(xs))*0.5; cy=(max(ys)+min(ys))*0.5; cz=(max(zs)+min(zs))*0.5
        s=max(max(xs)-min(xs), max(ys)-min(ys), max(zs)-min(zs)) or 1.0
        V=[((p[0]-cx)/s*2.0,(p[1]-cy)/s*2.0,(p[2]-cz)/s*2.0) for p in V]

        # smooth vertex normals
        N=[(0.0,0.0,0.0)]*len(V)
        acc=[[0.0,0.0,0.0] for _ in V]
        for (a,b,c) in TRI:
            fn=vcross(vsub(V[b],V[a]), vsub(V[c],V[a]))
            for idx in (a,b,c):
                acc[idx][0]+=fn[0]; acc[idx][1]+=fn[1]; acc[idx][2]+=fn[2]
        N=[vnorm(tuple(a)) for a in acc]
        UV=[(0.5,0.5)]*len(V)
        return V,N,UV,TRI

# --------------------------------------------------------- IQM writer (repo fmt)
def write_iqm(path, verts, norms, uvs, tris, tex):
    """Byte-identical layout to gen_props.write_iqm / gen_box.py (proven in-game)."""
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
    for uv in uvs:  out+=struct.pack("<2f",*uv)
    for n in norms: out+=struct.pack("<3f",*n)
    for t in tris:  out+=u(*t)
    assert len(out)==filesize,(len(out),filesize)
    open(path,"wb").write(out)
    return nv,nt,filesize

def update_aabb(data_dir, name, verts):
    """Merge one row into asset_aabb.txt without clobbering gen_props' rows."""
    path=os.path.join(data_dir,"asset_aabb.txt")
    rows={}
    if os.path.exists(path):
        for line in open(path):
            parts=line.split()
            if len(parts)==7: rows[parts[0]]=line.rstrip("\n")
    xs=[v[0] for v in verts]; ys=[v[1] for v in verts]; zs=[v[2] for v in verts]
    hx=(max(xs)-min(xs))*0.5; hy=(max(ys)-min(ys))*0.5; hz=(max(zs)-min(zs))*0.5
    cx=(max(xs)+min(xs))*0.5; cy=(max(ys)+min(ys))*0.5; cz=(max(zs)+min(zs))*0.5
    rows[name]="%s %.4f %.4f %.4f %.4f %.4f %.4f"%(name,cx,cy,cz,hx,hy,hz)
    with open(path,"w") as f:
        for k in sorted(rows): f.write(rows[k]+"\n")

# ------------------------------------------------------------------ presets
# Each preset reproduces a form from the interactive sessions.
PRESETS = {
  # banded, curving muscle for something in motion: wobble path + periodic
  # ridge/node bulges give the fibrous striations that read as flexing muscle.
  "muscle": (
     "seg 12 sec circle rad 0.42 wobble on ridge on twist 0.4 "
     + ("grow 3 node thick 0.05 yaw 4 grow 3 node thin 0.05 yaw 4 " * 6)
  ),
  # a Star-Trek-ish L cannon-wing: square spar, an L bend at a joint, circular
  # prongs forking off, mirrored into a symmetric pair of wings.
  "wing_cannon": (
     "mirror on sec square seg 4 rad 0.5 grow 7 "        # main spar
     "node yaw 62 grow 4 "                                # the L bend + joint
     "branch 38 sec circle seg 8 rad 0.30 grow 5 parent " # upper fork prong
     "branch -30 sec circle seg 8 rad 0.24 grow 4 parent " # lower fork prong
     "grow 2 node sec tri seg 3 rad 0.34 grow 3"          # muzzle tip
  ),
  # a twisting cable/tendril -- thin, high twist + wobble.
  "cable": "seg 8 sec circle rad 0.16 wobble on twist 1.4 grow 40",
  # a stepped heavy dorsal cannon: square breech -> circular barrel -> muzzle.
  "heavy_cannon": (
     "sec square seg 4 rad 0.55 grow 6 node "
     "sec circle seg 10 rad 0.42 grow 4 node "
     "sec circle seg 10 rad 0.30 grow 6 "
     "flare 0.3 grow 2"
  ),
  # twin swept BLADE FANS: flat blade sections raked back off a short square
  # root, three blades per side stepped in pitch -- an executioner silhouette.
  "razor": (
     "mirror on sec square seg 4 rad 0.34 grow 3 node "
     "branch 24 pitch -18 sec blade seg 6 rad 0.55 grow 7 pinch 0.25 grow 2 parent "
     "branch 40 pitch -4  sec blade seg 6 rad 0.48 grow 6 pinch 0.25 grow 2 parent "
     "branch 56 pitch 12  sec blade seg 6 rad 0.40 grow 5 pinch 0.3 grow 2"
  ),
  # a heavy horned crest: hex trunk, two forked horns, mirrored.
  "crest": (
     "mirror on sec hex seg 6 rad 0.5 grow 4 "
     "branch 30 pitch 20 rad 0.3 grow 6 pinch 0.4 grow 3 parent "
     "branch 55 rad 0.22 grow 5"
  ),
}

def main():
    ap=argparse.ArgumentParser(description="Headless spine-growth -> IQM generator")
    ap.add_argument("--preset", choices=sorted(PRESETS))
    ap.add_argument("--script", help="inline op script (see --list-presets for the vocabulary)")
    ap.add_argument("--out", help="output .iqm path (relative to repo root ok)")
    ap.add_argument("--tex", default="wall.png", help="material texture name (default wall.png)")
    ap.add_argument("--rot", default="", help="post-rotation 'x,y,z' degrees (e.g. '90,0,0' to stand a Z-grown part up along Y)")
    ap.add_argument("--list-presets", action="store_true")
    a=ap.parse_args()

    if a.list_presets:
        print("presets:")
        for k,v in PRESETS.items(): print(f"  {k:12} {v[:70]}{'...' if len(v)>70 else ''}")
        print("\nops: seg N | sec circle|square|tri|star|hex|oct|diamond|blade|plus|gear|arrow | rad R | grow N |")
        print("     yaw/pitch/roll DEG | twist R | thick/thin D | taper R | pinch/flare D | ridge on|off|V |")
        print("     wobble on|off | node | branch [DEG] | parent | mirror on|off | radial N | scale SX SY | bend DEG")
        print("  sections: circle square tri star hex oct diamond blade plus gear  (blade=wide fin,")
        print("            plus=cross, gear=cog, diamond=rotated square)")
        print("  radial N: N copies fanned around the vertical axis (cogs, turbines, crowns, thrusters)")
        print("  scale SX SY: non-uniform cross-section (1 0.3 = flat plate/fin, 0.5 1 = tall oval)")
        print("  bend DEG: curve the growth a little each step (horns, tusks, hooks, arcs)")
        return

    if not a.out: ap.error("--out is required")
    src = a.script if a.script else (PRESETS[a.preset] if a.preset else None)
    if src is None: ap.error("give --preset or --script")

    tokens=[t for t in src.replace("\n"," ").split(" ") if t]
    g=Grower(); g.run(tokens)
    V,N,UV,TRI=g.bake()

    if a.rot:
        rx,ry,rz=[math.radians(float(x)) for x in a.rot.split(",")]
        def rotall(v):
            v=vrot(v,(1,0,0),rx) if rx else v
            v=vrot(v,(0,1,0),ry) if ry else v
            v=vrot(v,(0,0,1),rz) if rz else v
            return v
        # rot as pure rotation (vrot normalizes; positions aren't unit -> do raw)
        def rawrot(v):
            x,y,z=v
            if rx: y,z = y*math.cos(rx)-z*math.sin(rx), y*math.sin(rx)+z*math.cos(rx)
            if ry: x,z = x*math.cos(ry)+z*math.sin(ry), -x*math.sin(ry)+z*math.cos(ry)
            if rz: x,y = x*math.cos(rz)-y*math.sin(rz), x*math.sin(rz)+y*math.cos(rz)
            return (x,y,z)
        V=[rawrot(v) for v in V]; N=[vnorm(rawrot(n)) for n in N]

    # resolve out path relative to repo root (this file lives in tools/)
    repo=os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    out=a.out if os.path.isabs(a.out) else os.path.join(repo, a.out)
    os.makedirs(os.path.dirname(out), exist_ok=True)
    nv,nt,sz=write_iqm(out, V,N,UV,TRI, a.tex)
    name=os.path.splitext(os.path.basename(out))[0]
    data_dir=os.path.dirname(out)
    if os.path.basename(data_dir)=="data":
        update_aabb(data_dir, name, V)
    print(f"wrote {out}: {nv} verts {nt} tris {sz} bytes  ({len(g.spines)} spine(s), mirror={g.mirror})")

if __name__=="__main__":
    main()
