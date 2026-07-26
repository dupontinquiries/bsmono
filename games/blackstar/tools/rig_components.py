#!/usr/bin/env python3
"""rig_components.py -- split an imported glTF model into ENGINE PART COMPONENTS.

The converter (process_models.py --iqm) merges a model into ONE rigid mesh -- fine
for static props/vehicles, but it can't be POSED or ANIMATED like the engine's
built-in bots, which are assembled from separate part slots (head, chest, arms,
legs...). This tool is the first step toward that: it keeps the model's parts
SEPARATE, using the structure already in the glTF.

HOW: many of these models tag each mesh with a material whose name encodes the
body part (RB_Jazz_Head_TEXSET, ..._Chest_..., ..._ArmUP_..., ..._Legs_...). We
group geometry by material, map the part-name to an engine slot, and emit one IQM
per slot. Each part keeps its own UVs, so per-part TEXTURE FITTING is possible
later (the material tells us which source texture belongs to it).

HONEST LIMITS (reported per model, not hidden):
  * Only works when material names are semantic. Opaque names (Cust_Mat, INST)
    -> we can't map by name; those fall back to a single 'body' component.
  * Left/right limbs often SHARE one material (both upper arms = 'ArmUP') -> they
    come out as one merged 'both arms' mesh, not separate L/R slots. Splitting
    those needs a spatial pass (cluster by x-sign), which this flags as TODO.
  * The parts sit at the MODEL's proportions, not the engine skeleton's joint
    positions. Posing them with the engine's animation needs a retarget/align
    pass (map each part's pivot to the engine slot origin) -- the next stage.

Usage:
  python3 tools/rig_components.py MODEL_DIR --out parts_out
  # MODEL_DIR holds scene.gltf (+ scene.bin). Writes <slot>.iqm per mapped part.
"""
import sys, os, json, struct, argparse, base64

# source part-name fragment -> engine slot name. Order matters (first match wins).
PART_MAP = [
    ("head",   "phead"),
    ("chest",  "pchest"),
    ("armup",  "parm_upper"),   # both upper arms share this (L/R split = TODO)
    ("armlo",  "parm_lower"),
    ("arm",    "parm"),
    ("hand",   "phand"),
    ("legs",   "plegs"),
    ("leg",    "plegs"),
    ("foot",   "pfoot"),
    ("wheel",  "pwheel"),
    ("logo",   "plogo"),
]

def load_geometry_by_material(gltf_path):
    """Return {material_name: (positions Nx3, faces Mx3)} merged per material."""
    import numpy as np
    g = json.load(open(gltf_path)); base = os.path.dirname(gltf_path)
    bufs = []
    for b in g.get("buffers", []):
        uri = b.get("uri", "")
        if uri.startswith("data:"): bufs.append(base64.b64decode(uri.split(",",1)[1]))
        elif uri:
            p = os.path.join(base, uri)
            if not os.path.exists(p): return None, None
            bufs.append(open(p, "rb").read())
        else: bufs.append(b"")
    CT = {5120:'b',5121:'B',5122:'h',5123:'H',5125:'I',5126:'f'}
    NC = {'SCALAR':1,'VEC2':2,'VEC3':3,'VEC4':4}
    def acc(i):
        a = g['accessors'][i]; bv = g['bufferViews'][a['bufferView']]
        o = bv.get('byteOffset',0)+a.get('byteOffset',0); n = a['count']*NC[a['type']]
        return np.array(struct.unpack_from('<'+CT[a['componentType']]*n, bufs[bv['buffer']], o)).reshape(a['count'], NC[a['type']])
    groups = {}
    for m in g.get('meshes', []):
        for p in m['primitives']:
            mat = g['materials'][p['material']]['name'] if 'material' in p else 'unnamed'
            pos = acc(p['attributes']['POSITION'])
            idx = acc(p['indices']).reshape(-1) if 'indices' in p else np.arange(len(pos))
            V, F = groups.get(mat, (None, None))
            if V is None: groups[mat] = (pos, idx.reshape(-1,3))
            else:
                off = len(V); groups[mat] = (np.vstack([V, pos]), np.vstack([F, idx.reshape(-1,3)+off]))
    return groups, g.get('materials', [])

def map_slot(material_name):
    n = material_name.lower()
    for frag, slot in PART_MAP:
        if frag in n: return slot
    return None

def normalize_slot(V):
    import numpy as np
    lo = V.min(0); hi = V.max(0); c = (hi+lo)*0.5; s = float((hi-lo).max()) or 1.0
    return ((V-c)/s*2.0).astype("f4")

def vertex_normals(V, F):
    import numpy as np
    N = np.zeros_like(V); tri = V[F]
    fn = np.cross(tri[:,1]-tri[:,0], tri[:,2]-tri[:,0])
    for k in range(3): np.add.at(N, F[:,k], fn)
    ln = np.linalg.norm(N,axis=1,keepdims=True); ln[ln==0]=1
    return (N/ln).astype("f4")

def write_iqm(path, V, F, tex="plate_blue.png"):
    import numpy as np
    N = vertex_normals(V, F); UV = np.zeros((len(V),2),"f4")
    def u(*a): return struct.pack("<"+"I"*len(a), *a)
    mat=(tex+"\x00").encode(); text=b"\x00"+mat+b"prop\x00"; mat_off=1; name_off=1+len(mat)
    nv=len(V); nt=len(F); ofs=16+4+4*26
    ofs_text=ofs; ofs+=len(text); ofs+=(-ofs)%4
    ofs_meshes=ofs; ofs+=6*4; ofs_va=ofs; ofs+=15*4
    ofs_pos=ofs; ofs+=nv*12; ofs_uv=ofs; ofs+=nv*8; ofs_nrm=ofs; ofs+=nv*12; ofs_tri=ofs; ofs+=nt*12
    out=b"INTERQUAKEMODEL\x00"+u(2,ofs,0)+u(len(text),ofs_text)+u(1,ofs_meshes)+u(3,nv,ofs_va)+u(nt,ofs_tri,0)
    out+=u(0,0)+u(0,0)+u(0,0)+u(0,0,0,0)+u(0,0)+u(0,0)
    out+=text+b"\x00"*((-(ofs_text+len(text)))%4)
    out+=u(name_off,mat_off,0,nv,0,nt)
    out+=u(0,0,7,3,ofs_pos)+u(1,0,7,2,ofs_uv)+u(2,0,7,3,ofs_nrm)
    out+=V.astype("<f4").tobytes()+UV.astype("<f4").tobytes()+N.astype("<f4").tobytes()+F.astype("<u4").tobytes()
    open(path,"wb").write(out); return nv,nt

def main():
    import numpy as np
    ap = argparse.ArgumentParser()
    ap.add_argument("model_dir")
    ap.add_argument("--out", default="parts_out")
    ap.add_argument("--tex", default="plate_blue.png")
    a = ap.parse_args()
    gltf = None
    for f in os.listdir(a.model_dir):
        if f.lower().endswith((".gltf",".glb")): gltf = os.path.join(a.model_dir, f); break
    if not gltf: print("no scene.gltf in", a.model_dir); sys.exit(1)

    groups, _mats = load_geometry_by_material(gltf)
    if groups is None: print("missing .bin -- can't split"); sys.exit(1)
    os.makedirs(a.out, exist_ok=True)

    mapped = {}; unmapped = []
    for mat, (V, F) in groups.items():
        slot = map_slot(mat)
        if slot: mapped.setdefault(slot, []).append((V, F, mat))
        else: unmapped.append(mat)

    # paired limb slots -> split L/R by x-sign so they fill the engine's separate slots
    PAIRED = {"parm", "parm_upper", "parm_lower", "phand", "plegs", "pfoot"}

    print("model:", os.path.basename(a.model_dir.rstrip("/")))
    name = os.path.basename(a.model_dir.rstrip("/"))
    def emit(slot, V, F):
        Vn = normalize_slot(np.asarray(V,"f4"))
        out = os.path.join(a.out, f"{name}__{slot}.iqm")
        nv, nt = write_iqm(out, Vn, np.asarray(F,"u4"), a.tex)
        return nv, nt, out
    for slot, chunks in sorted(mapped.items()):
        V = np.vstack([c[0] for c in chunks]); Fs = []; vo = 0
        for cv, cf, _m in chunks:
            Fs.append(cf + vo); vo += len(cv)
        F = np.vstack(Fs)
        srcs = ",".join(c[2].split("_TEXSET")[0].split("_")[-1] for c in chunks)
        if slot in PAIRED:
            fc = V[F].mean(1)                       # face centroids
            for side, mask in (("L", fc[:,0] < 0), ("R", fc[:,0] >= 0)):
                if not mask.any(): continue
                # compact the side's faces + verts
                fsub = F[mask]; used = np.unique(fsub)
                remap = {int(o): i for i, o in enumerate(used)}
                Vsub = V[used]
                Fsub = np.array([[remap[int(x)] for x in tri] for tri in fsub])
                nv, nt, out = emit(f"{slot}_{side}", Vsub, Fsub)
                print(f"  {slot+'_'+side:14s} <- {srcs:16s} {nv:6d}v {nt:6d}t  -> {os.path.basename(out)}")
        else:
            nv, nt, out = emit(slot, V, F)
            print(f"  {slot:14s} <- {srcs:16s} {nv:6d}v {nt:6d}t  -> {os.path.basename(out)}")
    if unmapped:
        print("  UNMAPPED (opaque material names, need spatial split):")
        for m in unmapped: print("     ", m)
    print("  NOTE: parts sit at the model's proportions; aligning each part's pivot")
    print("        to the engine joint slots for posing is the next stage (retarget).")

if __name__ == "__main__":
    main()
