#!/usr/bin/env python3
"""texture_export.py -- export an imported model as REAL-TEXTURED per-part IQMs.

The single-mesh import wears a flat 'plate_blue' placeholder because merging a
model's several materials collapses their UVs. This tool instead keeps each
material as its own mesh part -- preserving that part's real UVs -- and writes an
IQM per part whose material field points at the part's real baseColor texture. It
also copies the textures into the engine texture dir under the names the engine's
IQM loader expects (diffuse = <name>.png, normal = norm_<name>.png, spec =
spec_<name>.png -- the loader auto-derives the norm_/spec_ lookups).

Result: a model becomes N textured parts the engine can render with real skins.
This is also the structure animation needs (separate posable parts), so it's the
shared foundation for both.

Usage:
  python3 tools/texture_export.py MODEL_DIR --name jazz --out-iqm engine_src/data \
      --out-tex engine_src/data/textures
"""
import sys, os, json, struct, argparse, base64, shutil, re

def _san(s):
    return re.sub(r'[^a-zA-Z0-9_]', '_', s).strip('_').lower()

def sibling_objs(obj_path):
    """All .obj files that make up this model. Downloads frequently split a model
    into dozens of sibling pieces (model_0.obj ... model_68.obj) in one folder;
    parsing only one silently drops the rest of the model."""
    d = os.path.dirname(obj_path)
    try:
        objs = sorted(f for f in os.listdir(d) if f.lower().endswith(".obj"))
    except OSError:
        return [obj_path]
    return [os.path.join(d, f) for f in objs] or [obj_path]

def _obj_signature(path):
    """(vertex_block_hash, face_block_hash, n_v). Sibling files sharing BOTH are
    byte-equivalent meshes and only one needs parsing; sharing only the vertex
    hash means one vertex list with differing faces."""
    import hashlib
    hv = hashlib.sha1(); hf = hashlib.sha1(); nv = 0
    try:
        with open(path, "r", errors="ignore") as fh:
            for line in fh:
                if line.startswith("v ") or line.startswith("vt "):
                    hv.update(line.encode("utf-8", "replace")); nv += 1
                elif line.startswith("f "):
                    hf.update(line.encode("utf-8", "replace"))
    except OSError:
        return None, None, 0
    return hv.hexdigest(), hf.hexdigest(), nv

def _dedupe_faces(F):
    """Drop repeated triangles (same 3 indices in any order)."""
    import numpy as np
    if len(F) == 0: return F
    key = np.sort(F, axis=1)
    _u, idx = np.unique(key, axis=0, return_index=True)
    return F[np.sort(idx)]

def load_obj_by_material(obj_path):
    """Parse a model grouped by usemtl, with UVs, across ALL sibling .obj pieces.
    Handles siblings that re-declare the same vertex list AND the same faces."""
    import numpy as np
    parts = sibling_objs(obj_path)
    if len(parts) == 1:
        return _load_one_obj(obj_path)

    base = os.path.dirname(obj_path)
    by_vblock = {}; seen_pairs = set(); skipped = 0
    for p in parts:
        vh, fh, _n = _obj_signature(p)
        if vh is None: continue
        if (vh, fh) in seen_pairs:      # identical mesh already taken
            skipped += 1; continue
        seen_pairs.add((vh, fh))
        by_vblock.setdefault(vh, []).append(p)

    merged = {}; all_tex = {}
    for _vh, group in by_vblock.items():
        Va, VTa = _load_obj_vertices(group[0])      # one copy per vertex block
        nv, nvt = len(Va), len(VTa)
        for p in group:
            faces, mtl = _load_obj_faces(p, nv, nvt)
            if mtl:
                mp = os.path.join(base, mtl)
                if os.path.exists(mp): all_tex.update(_parse_mtl(mp, base))
            for mat, tris in faces.items():
                vmap = {}; lv = []; luv = []; lf = []
                for ptri, ttri in tris:
                    face = []
                    for pv_i, tv_i in zip(ptri, ttri):
                        key = (pv_i, tv_i)
                        if key not in vmap:
                            if not (0 <= pv_i < nv): continue
                            vmap[key] = len(lv); lv.append(Va[pv_i])
                            luv.append(VTa[tv_i] if 0 <= tv_i < nvt else (0.0, 0.0))
                        face.append(vmap[key])
                    if len(face) == 3: lf.append(face)
                if not lf: continue
                Vp = np.array(lv, "f4"); Up = np.array(luv, "f4"); Fp = np.array(lf, "u4")
                if mat in merged:
                    V0, U0, F0 = merged[mat]
                    merged[mat] = (np.vstack([V0, Vp]), np.vstack([U0, Up]),
                                   np.vstack([F0, Fp + len(V0)]))
                else:
                    merged[mat] = (Vp, Up, Fp)

    # final numeric pass: removes triangles repeated across partially-overlapping files
    out = {}
    for i, (mat, (V, U, F)) in enumerate(merged.items()):
        out[(mat, i)] = (V, U, _dedupe_faces(F))
    if skipped:
        print(f"    (merged {len(parts)} .obj pieces; {skipped} duplicate mesh file(s) skipped, "
              f"{len(by_vblock)} distinct vertex block(s))")
    return out, all_tex, base

def _load_obj_vertices(path):
    """Just the v / vt arrays."""
    import numpy as np
    V = []; VT = []
    with open(path, "r", errors="ignore") as fh:
        for line in fh:
            if line.startswith("v "):
                p = line.split(); V.append((float(p[1]), float(p[2]), float(p[3])))
            elif line.startswith("vt "):
                p = line.split(); VT.append((float(p[1]), float(p[2]) if len(p) > 2 else 0.0))
    return (np.array(V, "f4") if V else np.zeros((0, 3), "f4"),
            np.array(VT, "f4") if VT else np.zeros((1, 2), "f4"))

def _load_obj_faces(path, nv, nvt):
    """{material: [(v_idx, t_idx) x3]} without re-reading the vertex block."""
    faces = {}; cur = "default"; mtl = None
    with open(path, "r", errors="ignore") as fh:
        for line in fh:
            if line.startswith("usemtl"):
                cur = line.split(maxsplit=1)[1].strip() if len(line.split()) > 1 else "default"
            elif line.startswith("mtllib"):
                mtl = line.split(maxsplit=1)[1].strip()
            elif line.startswith("f "):
                vs = []; ts = []
                for tok in line.split()[1:]:
                    a = tok.split("/")
                    vi = int(a[0]); vs.append(vi - 1 if vi > 0 else nv + vi)
                    if len(a) > 1 and a[1]:
                        ti = int(a[1]); ts.append(ti - 1 if ti > 0 else nvt + ti)
                    else:
                        ts.append(-1)
                for k in range(1, len(vs) - 1):
                    faces.setdefault(cur, []).append(((vs[0], vs[k], vs[k+1]),
                                                     (ts[0], ts[k], ts[k+1])))
    return faces, mtl

def _load_one_obj(obj_path):
    """Parse a .obj grouped by usemtl material, with UVs. Returns
    (groups{(mat,i):(V,UV,F)}, mtl_textures{mat:basecolor_path}, base_dir)."""
    import numpy as np
    base = os.path.dirname(obj_path)
    V=[]; VT=[]; groups={}; cur='default'; matlist=[]
    mtl_tex={}
    def gkey(m):
        if m not in [k[0] for k in groups]:
            matlist.append(m)
        return m
    raw = {}   # mat -> (pos_idx list, uv_idx list) as faces
    with open(obj_path,'r',errors='ignore') as fh:
        for line in fh:
            if line.startswith('v '):
                p=line.split(); V.append((float(p[1]),float(p[2]),float(p[3])))
            elif line.startswith('vt '):
                p=line.split(); VT.append((float(p[1]), float(p[2]) if len(p)>2 else 0.0))
            elif line.startswith('usemtl'):
                cur=line.split(maxsplit=1)[1].strip() if len(line.split())>1 else 'default'
                raw.setdefault(cur, [])
            elif line.startswith('mtllib'):
                mtl=line.split(maxsplit=1)[1].strip()
                mp=os.path.join(base,mtl)
                if os.path.exists(mp): mtl_tex.update(_parse_mtl(mp, base))
            elif line.startswith('f '):
                toks=line.split()[1:]; vs=[]; ts=[]
                for tok in toks:
                    a=tok.split('/')
                    vi=int(a[0]); vs.append(vi-1 if vi>0 else len(V)+vi)
                    if len(a)>1 and a[1]:
                        ti=int(a[1]); ts.append(ti-1 if ti>0 else len(VT)+ti)
                    else: ts.append(-1)
                raw.setdefault(cur, [])
                for k in range(1,len(vs)-1):
                    raw[cur].append(((vs[0],vs[k],vs[k+1]),(ts[0],ts[k],ts[k+1])))
    Va=np.array(V,'f4'); VTa=np.array(VT,'f4') if VT else np.zeros((1,2),'f4')
    for mi,(mat,tris) in enumerate(raw.items()):
        if not tris: continue
        # build a local vertex list keyed by (pos,uv) so UVs are per-corner
        vmap={}; lv=[]; luv=[]; lf=[]
        for ptri,ttri in tris:
            face=[]
            for pv_i,tv_i in zip(ptri,ttri):
                key=(pv_i,tv_i)
                if key not in vmap:
                    vmap[key]=len(lv); lv.append(Va[pv_i])
                    luv.append(VTa[tv_i] if tv_i>=0 and tv_i<len(VTa) else (0.0,0.0))
                face.append(vmap[key])
            lf.append(face)
        groups[(mat,mi)]=(np.array(lv,'f4'), np.array(luv,'f4'), np.array(lf,'u4'))
    return groups, mtl_tex, base

def _parse_mtl(mtl_path, base):
    """mat name -> baseColor texture path (map_Kd)."""
    tex={}; cur=None
    with open(mtl_path,'r',errors='ignore') as fh:
        for line in fh:
            if line.startswith('newmtl'): cur=line.split(maxsplit=1)[1].strip()
            elif line.lower().startswith('map_kd') and cur:
                fn=line.split(maxsplit=1)[1].strip()
                p=os.path.join(base, fn.replace('\\','/'))
                if os.path.exists(p): tex[cur]=p
    return tex

def load_by_material(gltf_path):
    """{material_name: (V Nx3, UV Nx2, F Mx3)} plus the glTF + dir for textures.
    Handles both text .gltf and binary .glb containers."""
    import numpy as np
    base = os.path.dirname(gltf_path)
    raw = open(gltf_path, "rb").read()
    glb_bin = b""
    if raw[:4] == b"glTF":                       # binary container
        off = 12; g = None
        while off + 8 <= len(raw):
            clen, ctype = struct.unpack_from("<II", raw, off); off += 8
            chunk = raw[off:off+clen]; off += clen + ((-clen) % 4)
            if ctype == 0x4E4F534A:              # 'JSON'
                g = json.loads(chunk.decode("utf-8", "replace"))
            elif ctype == 0x004E4942:            # 'BIN\0'
                glb_bin = chunk
        if g is None: return None, {}, base
    else:
        g = json.loads(raw.decode("utf-8", "replace"))
    bufs = []
    for b in g.get("buffers", []):
        uri = b.get("uri","")
        if uri.startswith("data:"): bufs.append(base64.b64decode(uri.split(",",1)[1]))
        elif uri:
            p = os.path.join(base, uri)
            if not os.path.exists(p): return None, g, base
            bufs.append(open(p,"rb").read())
        else: bufs.append(glb_bin)               # GLB: buffer 0 lives in the BIN chunk
    CT={5120:'b',5121:'B',5122:'h',5123:'H',5125:'I',5126:'f'}; NC={'SCALAR':1,'VEC2':2,'VEC3':3,'VEC4':4}
    def acc(i):
        a=g['accessors'][i]; bv=g['bufferViews'][a['bufferView']]
        o=bv.get('byteOffset',0)+a.get('byteOffset',0); n=a['count']*NC[a['type']]
        return np.array(struct.unpack_from('<'+CT[a['componentType']]*n, bufs[bv['buffer']], o)).reshape(a['count'],NC[a['type']])
    groups={}
    for m in g.get('meshes',[]):
        for p in m['primitives']:
            if 'POSITION' not in p.get('attributes',{}): continue
            mode = p.get('mode', 4)              # default 4 = TRIANGLES
            if mode < 4: continue                # points/lines carry no surface
            mi = p.get('material')
            mat = g['materials'][mi]['name'] if mi is not None else 'unnamed'
            pos = acc(p['attributes']['POSITION'])
            uv  = acc(p['attributes']['TEXCOORD_0']) if 'TEXCOORD_0' in p['attributes'] else np.zeros((len(pos),2))
            idx = np.asarray(acc(p['indices']) if 'indices' in p else np.arange(len(pos))).reshape(-1)
            if mode == 5:                        # TRIANGLE_STRIP
                if len(idx) < 3: continue
                tri = np.column_stack([idx[:-2], idx[1:-1], idx[2:]]).copy()
                tri[1::2, [0,1]] = tri[1::2, [1,0]]
            elif mode == 6:                      # TRIANGLE_FAN
                if len(idx) < 3: continue
                tri = np.column_stack([np.repeat(idx[0], len(idx)-2), idx[1:-1], idx[2:]])
            else:
                if len(idx) < 3: continue
                if len(idx) % 3: idx = idx[:len(idx)-(len(idx)%3)]
                tri = idx.reshape(-1,3)
            key = (mat, mi)
            if key not in groups: groups[key] = (pos, uv, tri)
            else:
                V,U,F = groups[key]; off=len(V)
                groups[key] = (np.vstack([V,pos]), np.vstack([U,uv]), np.vstack([F, tri+off]))
    return groups, g, base

def base_color_texture(g, base, mat_index):
    """Return the source path of this material's baseColor image, or None."""
    if mat_index is None: return None
    mat = g['materials'][mat_index]
    pbr = mat.get('pbrMetallicRoughness', {})
    ti = pbr.get('baseColorTexture', {}).get('index')
    if ti is None:
        ti = mat.get('emissiveTexture', {}).get('index')  # fallback
    if ti is None: return None
    img_i = g['textures'][ti].get('source')
    if img_i is None: return None
    uri = g['images'][img_i].get('uri')
    if not uri: return None
    p = os.path.join(base, uri)
    return p if os.path.exists(p) else None

def normal_texture(g, base, mat_index):
    if mat_index is None: return None
    mat = g['materials'][mat_index]
    ti = mat.get('normalTexture', {}).get('index')
    if ti is None: return None
    img_i = g['textures'][ti].get('source')
    if img_i is None: return None
    uri = g['images'][img_i].get('uri')
    if not uri: return None
    p = os.path.join(base, uri)
    return p if os.path.exists(p) else None

def emissive_texture(g, base, mat_index):
    if mat_index is None: return None
    mat = g['materials'][mat_index]
    ti = mat.get('emissiveTexture', {}).get('index')
    if ti is None: return None
    img_i = g['textures'][ti].get('source')
    if img_i is None: return None
    uri = g['images'][img_i].get('uri')
    if not uri: return None
    p = os.path.join(base, uri)
    return p if os.path.exists(p) else None

def metallic_rough_texture(g, base, mat_index):
    if mat_index is None: return None
    mat = g['materials'][mat_index]
    pbr = mat.get('pbrMetallicRoughness', {})
    ti = pbr.get('metallicRoughnessTexture', {}).get('index')
    if ti is None: return None
    img_i = g['textures'][ti].get('source')
    if img_i is None: return None
    uri = g['images'][img_i].get('uri')
    if not uri: return None
    p = os.path.join(base, uri)
    return p if os.path.exists(p) else None

def vertex_normals(V, F):
    import numpy as np
    N=np.zeros_like(V); tri=V[F]; fn=np.cross(tri[:,1]-tri[:,0],tri[:,2]-tri[:,0])
    for k in range(3): np.add.at(N,F[:,k],fn)
    ln=np.linalg.norm(N,axis=1,keepdims=True); ln[ln==0]=1
    return (N/ln).astype("f4")

def normalize_together(groups):
    """Normalize ALL parts by the SAME transform (whole-model bounds) so parts
    keep their relative positions -- essential for reassembly."""
    import numpy as np
    allV = np.vstack([V for (V,U,F) in groups.values()])
    lo=allV.min(0); hi=allV.max(0); c=(lo+hi)*0.5; s=float((hi-lo).max()) or 1.0
    return {k:(((V-c)/s*2.0).astype("f4"),U,F) for k,(V,U,F) in groups.items()}, s

def weld_uv(V, UV, F, tol=1e-4):
    """Weld only TRUE duplicates (same position AND same uv). Preserves genuine UV
    seams -- merging across them averages mismatched UVs and corrupts the texture
    (purple/wrong regions). Closes numerical-duplicate cracks; leaves real seam
    boundaries (which are the era's open-panel design, not a bug we can weld away)."""
    import numpy as np
    key = np.concatenate([np.round(V/tol).astype(np.int64),
                          np.round(UV/tol).astype(np.int64)], axis=1)
    uniq, inv = np.unique(key, axis=0, return_inverse=True)
    nV=np.zeros((len(uniq),3)); nU=np.zeros((len(uniq),2)); cnt=np.zeros(len(uniq))
    np.add.at(nV,inv,V); np.add.at(nU,inv,UV); np.add.at(cnt,inv,1)
    nV/=cnt[:,None]; nU/=cnt[:,None]
    Fw=inv[F]
    good=(Fw[:,0]!=Fw[:,1])&(Fw[:,1]!=Fw[:,2])&(Fw[:,0]!=Fw[:,2])
    return nV.astype("f4"), nU.astype("f4"), Fw[good].astype("u4")

def write_iqm(path, V, UV, F, tex):
    import numpy as np
    N=vertex_normals(V,F)
    def u(*a): return struct.pack("<"+"I"*len(a),*a)
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

def convert_texture(src, dst, cap=1024, fmt="auto", quality=92):
    """Resize a source texture into the engine texture dir.

    fmt='auto' stores opaque COLOUR maps as JPEG (measured ~6.4x smaller than PNG
    at q92, PSNR 35-38dB) and keeps NORMAL/SPEC maps as PNG -- lossy compression
    on a normal map produces visible shading artifacts. Anything with real alpha
    stays PNG. Returns the filename actually written (extension may change)."""
    from PIL import Image
    im = Image.open(src)
    im = im.convert("RGBA") if im.mode in ("RGBA", "LA", "P") else im.convert("RGB")
    if max(im.size) > cap:
        r = cap / max(im.size)
        im = im.resize((max(1, int(im.size[0]*r)), max(1, int(im.size[1]*r))))

    base = os.path.basename(dst)
    is_map = base.startswith(("norm_", "spec_"))     # must stay lossless
    has_alpha = (im.mode == "RGBA" and im.getextrema()[3][0] < 255)

    use_jpeg = (fmt == "jpg") or (fmt == "auto" and not is_map and not has_alpha)
    if fmt == "png": use_jpeg = False
    if use_jpeg:
        dst = os.path.splitext(dst)[0] + ".jpg"
        im.convert("RGB").save(dst, "JPEG", quality=quality, optimize=True, progressive=True)
    else:
        dst = os.path.splitext(dst)[0] + ".png"
        im.save(dst, "PNG", optimize=True)
    return os.path.basename(dst)

def find_models(root):
    """Yield (name, gltf_path) for every model under root, disambiguating
    duplicate folder names by parent dir (same as process_models.py)."""
    found = []
    for dp, _d, files in os.walk(root):
        for f in files:
            if f.lower().endswith((".gltf", ".glb", ".obj")):
                base = os.path.basename(dp.rstrip("/")) or "model"
                parent = os.path.basename(os.path.dirname(dp.rstrip("/")))
                found.append([base, parent, os.path.join(dp, f)])
                break
    from collections import Counter
    dupes = {n for n, c in Counter(b for b, _p, _g in found).items() if c > 1}
    for rec in found:
        if rec[0] in dupes and rec[1]:
            rec[0] = f"{rec[1]}_{rec[0]}"
    for base, _parent, gltf in found:
        yield base, gltf

def scan_anim(mat_name, V):
    """Classify a part's animation type from NAME (reliable) confirmed by SHAPE.
    Returns (tag, axis). tire=spins on axle; fan/rotor=spins; door/armor=hinge/
    shift; body=none. Tires LOCK in robot mode unless the character is flagged
    wheeled (Sideswipe-style) -- the engine decides lock vs engaged; the tag just
    identifies the part."""
    import numpy as np
    n = mat_name.lower()
    ext=V.max(0)-V.min(0); s=np.sort(ext); c=V.mean(0); d=V-c
    ax=np.argsort(ext)
    r=np.sqrt(d[:,ax[1]]**2+d[:,ax[2]]**2); roundness=r.std()/max(1e-6,r.mean())
    flat=s[0]/s[2]; round_ok = roundness < 0.32
    if any(k in n for k in ("wheel","tire","tyre")):   return ("tire", int(ax[0]) if round_ok else int(np.argmin(ext)))
    if any(k in n for k in ("fan","rotor","turbine","prop")): return ("fan", int(np.argmin(ext)))
    if any(k in n for k in ("door","hatch","flap")):   return ("door", int(np.argmax(ext)))
    if any(k in n for k in ("shin","guard","armor","armour","pauldron","shoulder")): return ("armor", int(np.argmax(ext)))
    if any(k in n for k in ("logo","badge","decal","emblem","insignia","sticker")):
        return ("body", -1)          # flat decals are disc-shaped but must NOT spin
    # shape-only fallback (low confidence): require a real disc AND meaningful size,
    # otherwise small flat bits get mistaken for wheels.
    span = float(np.linalg.norm(ext))
    if round_ok and flat < 0.35 and s[0] < 0.4*s[1] and span > 0.12:
        return ("tire?", int(ax[0]))
    return ("body", -1)

# engine atag codes (must match ex_tmodel_t in game.c): 0 body,1 tire,2 fan,3 armor,4 door
ATAG_CODE = {"body": 0, "tire": 1, "tire?": 1, "fan": 2, "armor": 3, "door": 4}

def _validate_part(path):
    """Structural check of one part IQM: magic, version, filesize (truncation),
    non-empty geometry. Returns (ok, reason)."""
    try:
        if not os.path.isfile(path): return False, "missing part"
        size = os.path.getsize(path)
        if size < 128: return False, "part too small"
        with open(path,"rb") as fh: head = fh.read(16 + 4*13)
        if head[:16] != b"INTERQUAKEMODEL\x00": return False, "bad magic"
        w = struct.unpack_from("<13I", head, 16)
        if w[0] != 2: return False, f"version {w[0]}"
        if w[1] != size: return False, f"truncated part ({size}B vs {w[1]}B)"
        if w[8] == 0 or w[10] == 0: return False, "empty part"
        return True, f"{w[10]}t"
    except Exception as e:
        return False, type(e).__name__

def validate_export(out_iqm, name, src_mtime=None):
    """A model counts as already-done only if its manifest exists and EVERY part
    it lists validates. Returns (ok, reason, n_parts)."""
    man = os.path.join(out_iqm, "imported_%s_parts.txt" % name)
    if not os.path.isfile(man): return False, "no manifest", 0
    try:
        rows = [r.split() for r in open(man).read().split("\n") if r.strip()]
    except Exception as e:
        return False, f"manifest unreadable ({type(e).__name__})", 0
    if not rows: return False, "empty manifest", 0
    if src_mtime and os.path.getmtime(man) < src_mtime:
        return False, "stale (source is newer)", len(rows)
    total = 0
    for r in rows:
        ok, why = _validate_part(os.path.join(out_iqm, r[0]))
        if not ok: return False, f"{r[0]}: {why}", len(rows)
        total += 1
    return True, f"{total} parts", total

def export_one(name, gltf, out_iqm, out_tex, cap, fmt='auto', quality=92):
    """Export one model -> textured per-part IQMs + textures. Returns part count
    or error string. Handles glTF and OBJ."""
    import numpy as np
    is_obj = gltf.lower().endswith('.obj')
    if is_obj:
        groups, mtl_tex, base = load_obj_by_material(gltf)
        g = None
    else:
        groups, g, base = load_by_material(gltf)
    if not groups:
        return None, "no geometry"
    groups, _s = normalize_together(groups)
    parts = []
    for pi,((mat, mi),(V,UV,F)) in enumerate(groups.items()):
        texsrc = mtl_tex.get(mat) if is_obj else base_color_texture(g, base, mi)
        if texsrc:
            texname = "%s_%s.png" % (name, _san(mat)[:24])
            try:
                # colour map may come back as .jpg; companions are named off its stem
                texname = convert_texture(texsrc, os.path.join(out_tex, texname), cap, fmt, quality)
                stem = os.path.splitext(texname)[0]
                if not is_obj:
                    nrm = normal_texture(g, base, mi)
                    if nrm: convert_texture(nrm, os.path.join(out_tex, "norm_"+stem+".png"), cap, "png")
                    emi = emissive_texture(g, base, mi)
                    if emi: convert_texture(emi, os.path.join(out_tex, "emit_"+stem+".png"), cap, fmt, quality)
                    mr = metallic_rough_texture(g, base, mi)
                    if mr: convert_texture(mr, os.path.join(out_tex, "spec_"+stem+".png"), cap, "png")
            except Exception as e:
                texname = "plate_blue.png"
        else:
            texname = "plate_blue.png"
        iqm = os.path.join(out_iqm, "imported_%s_part%d.iqm" % (name, pi))
        Vw, UVw, Fw = weld_uv(np.asarray(V,"f4"), np.asarray(UV,"f4"), np.asarray(F,"u4"))
        tag, axis = scan_anim(mat, np.asarray(V,"f4"))
        # A part that SPINS must rotate about its own centre. Parts are normalised
        # as a group, so each carries its offset inside its own vertices -- rotating
        # one would swing it around the model centre (tires orbiting, not turning).
        # Centre spinning parts and hand the offset to the engine instead.
        off = (0.0, 0.0, 0.0)
        if ATAG_CODE.get(tag, 0) in (1, 2):          # tire / fan
            c = (Vw.min(0) + Vw.max(0)) * 0.5
            Vw = (Vw - c).astype("f4")
            off = (float(c[0]), float(c[1]), float(c[2]))
        write_iqm(iqm, Vw, UVw, Fw, texname)
        parts.append((os.path.basename(iqm), texname, tag, axis, ATAG_CODE.get(tag, 0), off))
    man = os.path.join(out_iqm, "imported_%s_parts.txt" % name)
    open(man,"w").write("\n".join("%s %s %s %d %d %.5f %.5f %.5f"
                                  % (p[0], p[1], p[2], p[3], p[4], p[5][0], p[5][1], p[5][2])
                                  for p in parts))
    return len(parts), None

def _export_safe(name, gltf, out_iqm, out_tex, cap, force=False, fmt='auto', quality=92):
    """Never raises. Skips models whose existing export VALIDATES; re-exports the
    ones whose parts are missing/truncated/corrupt. Returns (n, err, skip_reason)."""
    redo = None
    if not force:
        try: src_mtime = os.path.getmtime(gltf)
        except Exception: src_mtime = None
        good, why, n = validate_export(out_iqm, name, src_mtime)
        if good:
            return n, None, why
        if why not in ("no manifest",):
            redo = why
    try:
        n, err = export_one(name, gltf, out_iqm, out_tex, cap, fmt, quality)
        if err is None and redo: err = None
        return (n, err, None) if not redo else (n, err, "REDO:" + redo)
    except Exception as e:
        return None, f"{type(e).__name__}: {e}"[:70], None

def main():
    ap = argparse.ArgumentParser(description="Export imported model(s) as real-textured per-part IQMs. "
                                             "Point at a single model dir OR a parent folder to batch ALL models under it.")
    ap.add_argument("path", help="a model dir (scene.gltf inside) OR a folder of many models")
    ap.add_argument("--name", default=None, help="override name (single-model only; else auto from folder)")
    ap.add_argument("--out-iqm", default="textured_parts")
    ap.add_argument("--out-tex", default="textured_parts/textures")
    ap.add_argument("--cap", type=int, default=1024)
    ap.add_argument("--tex-format", choices=["auto","png","jpg"], default="auto",
                    help="auto: colour maps -> JPEG (~6x smaller), normal/spec stay PNG")
    ap.add_argument("--tex-quality", type=int, default=92, help="JPEG quality (default 92)")
    ap.add_argument("--force", action="store_true",
                    help="re-export everything, even models whose parts validate as good")
    ap.add_argument("--jobs", "-j", type=int, default=os.cpu_count() or 4,
                    help="parallel worker processes (default: all cores; 1 = serial)")
    a = ap.parse_args()
    os.makedirs(a.out_iqm, exist_ok=True); os.makedirs(a.out_tex, exist_ok=True)

    # single model dir? (scene.gltf directly inside)
    direct = None
    for f in os.listdir(a.path):
        if f.lower().endswith((".gltf",".glb",".obj")): direct = os.path.join(a.path,f); break

    if direct and a.name:
        models = [(a.name, direct)]
    else:
        models = list(find_models(a.path))
    if not models:
        print("no models (.gltf/.glb) found under", a.path); sys.exit(1)
    print("found %d model%s\n" % (len(models), "" if len(models)==1 else "s"))

    ok = fail = total_parts = skipped = 0
    def emit(name, res):
        nonlocal ok, fail, total_parts, skipped
        n, err, note = res
        if n is None:
            print("  %-46s FAIL: %s" % (name, err)); fail += 1
        elif note and not str(note).startswith("REDO:"):
            print("  %-46s SKIP (ok, %s)" % (name, note)); ok += 1; skipped += 1; total_parts += n
        else:
            tail = "  [redone: %s]" % str(note)[5:] if note else ""
            print("  %-46s %d textured parts%s" % (name, n, tail)); ok += 1; total_parts += n

    jobs = sorted(models)
    if a.jobs == 1:
        for name, gltf in jobs: emit(name, _export_safe(name, gltf, a.out_iqm, a.out_tex, a.cap, a.force, a.tex_format, a.tex_quality))
    else:
        from concurrent.futures import ProcessPoolExecutor, as_completed
        with ProcessPoolExecutor(max_workers=a.jobs) as ex:
            futs = {ex.submit(_export_safe, name, gltf, a.out_iqm, a.out_tex, a.cap, a.force, a.tex_format, a.tex_quality): name
                    for name, gltf in jobs}
            for fut in as_completed(futs):
                emit(futs[fut], fut.result())
    print("\n%d ok (%d skipped as already-good), %d fail, %d total parts. IQMs+manifests in %s, textures in %s"
          % (ok, skipped, fail, total_parts, a.out_iqm, a.out_tex))

if __name__ == "__main__":
    main()
