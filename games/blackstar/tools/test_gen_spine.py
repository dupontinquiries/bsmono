#!/usr/bin/env python3
"""Self-test for the IQM spine pipeline: every section + radial symmetry + all
presets must bake to a structurally valid IQM. Run from repo root."""
import subprocess, sys, os, struct
GEN="tools/gen_spine.py"
def gen(script, out):
    r=subprocess.run([sys.executable,GEN,"--script",script,"--out",out],
                     capture_output=True,text=True)
    if r.returncode!=0: raise SystemExit(f"gen failed for [{script}]:\n{r.stderr}")
def valid_iqm(path):
    d=open(path,'rb').read()
    assert d[:16].startswith(b"INTERQUAKEMODEL"), "bad magic"
    v=struct.unpack("<26I", d[16:16+26*4])
    assert v[8]>0 and v[10]>0, "no verts/tris"   # num_vertexes, num_triangles
    return v[8],v[10]
SECS=["circle","square","tri","star","hex","oct","diamond","blade","plus","gear"]
os.makedirs("/tmp/gstest",exist_ok=True)
for sc in SECS:
    o=f"/tmp/gstest/{sc}.iqm"; gen(f"sec {sc} rad 0.4 seg 8 grow 6", o); nv,nt=valid_iqm(o)
    print(f"  section {sc:8} -> {nv:4} verts {nt:4} tris  OK")
SPECIAL=[("scale flat","sec circle rad 0.4 seg 10 grow 5 scale 1 0.3"),
         ("scale oval","sec square rad 0.4 seg 4 grow 5 scale 0.5 1"),
         ("bend horn","sec circle rad 0.3 seg 8 grow 12 bend 6 taper 0.6"),
         ("arrow sec","sec arrow rad 0.4 seg 7 grow 6")]
for nm,sc in SPECIAL:
    o=f"/tmp/gstest/{nm.split()[0]}.iqm"; gen(sc, o); nv,nt=valid_iqm(o)
    print(f"  {nm:12} -> {nv:4} verts {nt:4} tris  OK")
for n in (3,6,8,12):
    o=f"/tmp/gstest/radial{n}.iqm"; gen(f"sec blade rad 0.3 seg 4 grow 4 pitch 55 radial {n}", o)
    nv,nt=valid_iqm(o); print(f"  radial {n:<2}            -> {nv:4} verts {nt:4} tris  OK")
for pr in ("muscle","wing_cannon","cable","heavy_cannon","crest"):
    o=f"/tmp/gstest/{pr}.iqm"
    r=subprocess.run([sys.executable,GEN,"--preset",pr,"--out",o],capture_output=True,text=True)
    if r.returncode!=0: raise SystemExit(f"preset {pr} failed:\n{r.stderr}")
    valid_iqm(o); print(f"  preset {pr:12} OK")
print("\nGEN_SPINE PIPELINE OK (11 sections, radial + scale + bend, 5 presets)")
