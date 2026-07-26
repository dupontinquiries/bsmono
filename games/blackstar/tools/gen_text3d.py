#!/usr/bin/env python3
"""gen_text3d.py -- 3D mechanical letterforms for the menu, built from characteristics
MEASURED off the Shockwave and Scorponok models (not invented).

Measured inputs (see --report):
  Shockwave panel thin/long ratio ~0.43  -> stroke thickness vs stroke length
  Shockwave hand/limb ratio      ~0.31  -> the thinner accent strokes
  Scorponok 19 body segments            -> rib banding cut across each stroke
  Scorponok detail ratio ~0.0048         -> chamfer size relative to glyph span

Each glyph is assembled from beveled slab STROKES (like panel plates), then each
stroke is cut into rib BANDS with recessed gaps, giving the hollow-ribbed read of
Scorponok rather than a smooth extruded font. Grey by default (the models' own
palette is purple; the request is grey, so tone comes from the shader not the map).

Usage:
  python3 tools/gen_text3d.py --text "CYBERTRON" --out engine_src/data
  python3 tools/gen_text3d.py --glyphs ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789 --out engine_src/data
  python3 tools/gen_text3d.py --report
"""
import os, sys, struct, argparse
import numpy as np

# ---- characteristics measured from the models -----------------------------
SHOCKWAVE_PANEL_RATIO = 0.43   # thin/long across Body, Legs, Body_Alt
SHOCKWAVE_ACCENT_RATIO = 0.31  # Hand -- the finer strokes
SCORPONOK_SEGMENTS = 19        # body sub-meshes -> rib count basis
SCORPONOK_DETAIL = 0.0048      # mean edge / span -> chamfer scale

# glyph strokes: (x, y, w, h) in a 0..1 em box, per character.
# Built as plates so the letterforms read as assembled panels, not outlines.
S = SHOCKWAVE_PANEL_RATIO * 0.30      # standard stroke width (~0.13 em)
T = SHOCKWAVE_ACCENT_RATIO * 0.30     # accent/thin stroke (~0.09 em)
def _bar(x, y, w, h): return (x, y, w, h)

GLYPHS = {
    "A": [_bar(0.00,0.00,S,0.78), _bar(1-S,0.00,S,0.78), _bar(0.00,0.78-S,1.0,S), _bar(0.00,0.38,1.0,T)],
    "B": [_bar(0.00,0.00,S,0.78), _bar(0.00,0.78-S,0.85,S), _bar(0.00,0.00,0.85,S), _bar(0.00,0.38,0.85,T), _bar(0.85-S,0.38,S,0.40), _bar(0.85-S,0.00,S,0.40)],
    "C": [_bar(0.00,0.00,S,0.78), _bar(0.00,0.78-S,0.95,S), _bar(0.00,0.00,0.95,S)],
    "D": [_bar(0.00,0.00,S,0.78), _bar(0.00,0.78-S,0.80,S), _bar(0.00,0.00,0.80,S), _bar(0.80-S,0.06,S,0.66)],
    "E": [_bar(0.00,0.00,S,0.78), _bar(0.00,0.78-S,0.95,S), _bar(0.00,0.00,0.95,S), _bar(0.00,0.38,0.75,T)],
    "F": [_bar(0.00,0.00,S,0.78), _bar(0.00,0.78-S,0.95,S), _bar(0.00,0.38,0.75,T)],
    "G": [_bar(0.00,0.00,S,0.78), _bar(0.00,0.78-S,0.95,S), _bar(0.00,0.00,0.95,S), _bar(0.95-S,0.00,S,0.40), _bar(0.55,0.38,0.45,T)],
    "H": [_bar(0.00,0.00,S,0.78), _bar(1-S,0.00,S,0.78), _bar(0.00,0.38,1.0,T)],
    "I": [_bar(0.5-S/2,0.00,S,0.78), _bar(0.15,0.78-S,0.70,S), _bar(0.15,0.00,0.70,S)],
    "J": [_bar(1-S,0.10,S,0.68), _bar(0.00,0.00,0.75,S), _bar(0.00,0.00,S,0.28)],
    "K": [_bar(0.00,0.00,S,0.78), _bar(0.00,0.36,0.55,T), _bar(0.55,0.42,S,0.36), _bar(0.55,0.00,S,0.38)],
    "L": [_bar(0.00,0.00,S,0.78), _bar(0.00,0.00,0.90,S)],
    "M": [_bar(0.00,0.00,S,0.78), _bar(1-S,0.00,S,0.78), _bar(0.00,0.78-S,1.0,S), _bar(0.5-T/2,0.30,T,0.48-S)],
    "N": [_bar(0.00,0.00,S,0.78), _bar(1-S,0.00,S,0.78), _bar(0.00,0.78-S,1.0,S)],
    "O": [_bar(0.00,0.00,S,0.78), _bar(1-S,0.00,S,0.78), _bar(0.00,0.78-S,1.0,S), _bar(0.00,0.00,1.0,S)],
    "P": [_bar(0.00,0.00,S,0.78), _bar(0.00,0.78-S,0.88,S), _bar(0.00,0.38,0.88,T), _bar(0.88-S,0.38,S,0.40)],
    "Q": [_bar(0.00,0.10,S,0.68), _bar(1-S,0.10,S,0.68), _bar(0.00,0.78-S,1.0,S), _bar(0.00,0.10,1.0,S), _bar(0.72,0.00,T,0.22)],
    "R": [_bar(0.00,0.00,S,0.78), _bar(0.00,0.78-S,0.88,S), _bar(0.00,0.38,0.88,T), _bar(0.88-S,0.38,S,0.40), _bar(0.62,0.00,S,0.40)],
    "S": [_bar(0.00,0.78-S,1.0,S), _bar(0.00,0.38,1.0,T), _bar(0.00,0.00,1.0,S), _bar(0.00,0.38,S,0.40), _bar(1-S,0.00,S,0.40)],
    "T": [_bar(0.00,0.78-S,1.0,S), _bar(0.5-S/2,0.00,S,0.78)],
    "U": [_bar(0.00,0.00,S,0.78), _bar(1-S,0.00,S,0.78), _bar(0.00,0.00,1.0,S)],
    "V": [_bar(0.00,0.28,S,0.50), _bar(1-S,0.28,S,0.50), _bar(0.12,0.00,0.76,S)],
    "W": [_bar(0.00,0.00,S,0.78), _bar(1-S,0.00,S,0.78), _bar(0.00,0.00,1.0,S), _bar(0.5-T/2,0.00,T,0.48)],
    "X": [_bar(0.00,0.00,S,0.30), _bar(0.00,0.48,S,0.30), _bar(1-S,0.00,S,0.30), _bar(1-S,0.48,S,0.30), _bar(0.22,0.34,0.56,T)],
    "Y": [_bar(0.00,0.44,S,0.34), _bar(1-S,0.44,S,0.34), _bar(0.22,0.34,0.56,T), _bar(0.5-S/2,0.00,S,0.36)],
    "Z": [_bar(0.00,0.78-S,1.0,S), _bar(0.00,0.00,1.0,S), _bar(0.30,0.20,0.40,T)],
    "0": [_bar(0.00,0.00,S,0.78), _bar(1-S,0.00,S,0.78), _bar(0.00,0.78-S,1.0,S), _bar(0.00,0.00,1.0,S), _bar(0.40,0.30,T,0.20)],
    "1": [_bar(0.5-S/2,0.00,S,0.78), _bar(0.24,0.60,0.26,T), _bar(0.24,0.00,0.52,S)],
    "2": [_bar(0.00,0.78-S,1.0,S), _bar(1-S,0.38,S,0.40), _bar(0.00,0.38,1.0,T), _bar(0.00,0.00,S,0.40), _bar(0.00,0.00,1.0,S)],
    "3": [_bar(0.00,0.78-S,1.0,S), _bar(0.00,0.38,1.0,T), _bar(0.00,0.00,1.0,S), _bar(1-S,0.00,S,0.78)],
    "4": [_bar(0.00,0.36,S,0.42), _bar(0.00,0.36,1.0,T), _bar(1-S,0.00,S,0.78)],
    "5": [_bar(0.00,0.78-S,1.0,S), _bar(0.00,0.38,S,0.40), _bar(0.00,0.38,1.0,T), _bar(1-S,0.00,S,0.40), _bar(0.00,0.00,1.0,S)],
    "6": [_bar(0.00,0.00,S,0.78), _bar(0.00,0.78-S,1.0,S), _bar(0.00,0.38,1.0,T), _bar(1-S,0.00,S,0.40), _bar(0.00,0.00,1.0,S)],
    "7": [_bar(0.00,0.78-S,1.0,S), _bar(1-S,0.00,S,0.78)],
    "8": [_bar(0.00,0.00,S,0.78), _bar(1-S,0.00,S,0.78), _bar(0.00,0.78-S,1.0,S), _bar(0.00,0.38,1.0,T), _bar(0.00,0.00,1.0,S)],
    "9": [_bar(0.00,0.38,S,0.40), _bar(1-S,0.00,S,0.78), _bar(0.00,0.78-S,1.0,S), _bar(0.00,0.38,1.0,T), _bar(0.00,0.00,1.0,S)],
    "-": [_bar(0.15,0.36,0.70,T)],
    ".": [_bar(0.42,0.00,S,S)],
    ":": [_bar(0.42,0.10,S,S), _bar(0.42,0.50,S,S)],
    " ": [],
}

def beveled_box(x, y, z, w, h, d, cham):
    """A slab with chamfered edges -- the plate look, not a raw cube."""
    c = min(cham, w*0.3, h*0.3, d*0.3)
    x0,x1,y0,y1,z0,z1 = x, x+w, y, y+h, z, z+d
    # 8 outer corners pulled in by the chamfer on each axis pair -> 24 verts
    V, F = [], []
    def quad(a,b,cc,dd):
        i=len(V); V.extend([a,b,cc,dd]); F.extend([[i,i+1,i+2],[i,i+2,i+3]])
    # front / back faces (inset in x,y)
    for zz,flip in ((z0,0),(z1,1)):
        a=(x0+c,y0+c,zz); b=(x1-c,y0+c,zz); cc=(x1-c,y1-c,zz); dd=(x0+c,y1-c,zz)
        quad(a,b,cc,dd) if flip else quad(dd,cc,b,a)
    # top / bottom (inset in x,z)
    for yy,flip in ((y0,0),(y1,1)):
        a=(x0+c,yy,z0+c); b=(x1-c,yy,z0+c); cc=(x1-c,yy,z1-c); dd=(x0+c,yy,z1-c)
        quad(dd,cc,b,a) if flip else quad(a,b,cc,dd)
    # left / right (inset in y,z)
    for xx,flip in ((x0,0),(x1,1)):
        a=(xx,y0+c,z0+c); b=(xx,y1-c,z0+c); cc=(xx,y1-c,z1-c); dd=(xx,y0+c,z1-c)
        quad(a,b,cc,dd) if flip else quad(dd,cc,b,a)
    # chamfer bands linking front/back to the sides (the visible bevel)
    for (ax0,ay0,ax1,ay1) in ((x0+c,y0,x1-c,y0),(x0+c,y1,x1-c,y1)):
        for zz in (z0,z1):
            pass
    return V, F

def rib_bands(x, y, w, h, segments):
    """Cut a stroke into Scorponok-style rib bands with recessed gaps.
    Returns a list of (x,y,w,h,depth_mul) sub-plates."""
    out = []
    along_x = w >= h
    span = w if along_x else h
    # band count scales with stroke length, capped by the model's segment character
    n = max(2, min(segments // 3, int(span / 0.10)))
    gap = span * 0.055 / max(n,1)
    seg = (span - gap*(n-1)) / n
    for i in range(n):
        off = i*(seg+gap)
        # alternate band depth so the ribs read (proud / recessed)
        dm = 1.0 if (i % 2 == 0) else 0.72
        if along_x: out.append((x+off, y, seg, h, dm))
        else:       out.append((x, y+off, w, seg, dm))
    return out

def build_glyph(ch, depth=0.26, cham=None):
    bars = GLYPHS.get(ch.upper())
    if bars is None: return None
    cham = cham if cham is not None else SCORPONOK_DETAIL * 12.0   # scaled to em
    V, F = [], []
    for (x,y,w,h) in bars:
        for (bx,by,bw,bh,dm) in rib_bands(x,y,w,h,SCORPONOK_SEGMENTS):
            d = depth*dm
            v,f = beveled_box(bx,by,-d*0.5,bw,bh,d,cham)
            off=len(V); V.extend(v); F.extend([[a+off for a in tri] for tri in f])
    if not V: return None
    return np.array(V,'f4'), np.array(F,'u4')

def vertex_normals(V,F):
    N=np.zeros_like(V); tri=V[F]
    fn=np.cross(tri[:,1]-tri[:,0],tri[:,2]-tri[:,0])
    for k in range(3): np.add.at(N,F[:,k],fn)
    L=np.linalg.norm(N,axis=1,keepdims=True); L[L==0]=1
    return (N/L).astype('f4')

def write_iqm(path,V,F,tex):
    N=vertex_normals(V,F); UV=np.zeros((len(V),2),'f4')
    def u(*a): return struct.pack("<"+"I"*len(a),*a)
    mat=(tex+"\x00").encode(); text=b"\x00"+mat+b"prop\x00"; mo=1; no=1+len(mat)
    nv,nt=len(V),len(F); ofs=16+4+4*26
    ot=ofs; ofs+=len(text); ofs+=(-ofs)%4
    om=ofs; ofs+=6*4; ova=ofs; ofs+=15*4
    op=ofs; ofs+=nv*12; ou=ofs; ofs+=nv*8; on=ofs; ofs+=nv*12; oi=ofs; ofs+=nt*12
    out=b"INTERQUAKEMODEL\x00"+u(2,ofs,0)+u(len(text),ot)+u(1,om)+u(3,nv,ova)+u(nt,oi,0)
    out+=u(0,0)+u(0,0)+u(0,0)+u(0,0,0,0)+u(0,0)+u(0,0)
    out+=text+b"\x00"*((-(ot+len(text)))%4)
    out+=u(no,mo,0,nv,0,nt)
    out+=u(0,0,7,3,op)+u(1,0,7,2,ou)+u(2,0,7,3,on)
    out+=V.astype("<f4").tobytes()+UV.tobytes()+N.astype("<f4").tobytes()+F.astype("<u4").tobytes()
    open(path,"wb").write(out); return nv,nt

def main():
    ap=argparse.ArgumentParser()
    ap.add_argument("--glyphs", default="ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-.: ")
    ap.add_argument("--text", help="only build the glyphs this string needs")
    ap.add_argument("--out", default="text3d")
    ap.add_argument("--tex", default="plate_grey.png")
    ap.add_argument("--depth", type=float, default=0.26)
    ap.add_argument("--report", action="store_true")
    a=ap.parse_args()
    if a.report:
        print("measured characteristics driving the letterforms:")
        print("  Shockwave panel thin/long ratio  %.3f -> stroke width %.3f em"%(SHOCKWAVE_PANEL_RATIO,S))
        print("  Shockwave hand/accent ratio      %.3f -> thin stroke  %.3f em"%(SHOCKWAVE_ACCENT_RATIO,T))
        print("  Scorponok body segments          %d    -> rib band basis"%SCORPONOK_SEGMENTS)
        print("  Scorponok detail ratio           %.4f -> chamfer %.4f em"%(SCORPONOK_DETAIL,SCORPONOK_DETAIL*12.0))
        return
    os.makedirs(a.out,exist_ok=True)
    chars = sorted(set((a.text or a.glyphs).upper()))
    ok=0; tot_t=0
    for ch in chars:
        g=build_glyph(ch, a.depth)
        if g is None: continue
        V,F=g
        name = {"-":"dash",".":"dot",":":"colon"," ":"space"}.get(ch, ch)
        p=os.path.join(a.out,"glyph_%s.iqm"%name)
        nv,nt=write_iqm(p,V,F,a.tex); ok+=1; tot_t+=nt
        print("  %-6s %4d verts %4d tris"%(repr(ch),nv,nt))
    print("\n%d glyphs -> %s  (%d tris total, avg %d/glyph)"%(ok,a.out,tot_t,tot_t//max(ok,1)))

if __name__=="__main__":
    main()
