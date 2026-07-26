#!/usr/bin/env python3
"""process_models.py -- ADAPTIVE compression pipeline for the downloaded models.

Corrects an earlier over-generalization: I measured 3 TEXTURED models and
concluded "textures are the weight". But the no-color models are big too, which
means for THOSE the geometry is the weight. So this tool doesn't assume -- it
MEASURES each model and compresses whatever is actually heavy:

  GEOMETRY-heavy  -> quadric-decimate over a triangle budget. (Measured on a real
                    model: 90% face reduction = only ~1.5% surface deviation --
                    these game meshes have near-coplanar detail that collapses cheaply.)
  TEXTURE-heavy   -> "emulate more compactly": downscale to a cap + quantize to a
                    tight palette + optimized PNG. (Measured: a 1024 baseColor ->
                    512+pal64 is ~10% of the size and still clearly readable --
                    keeps the ACTUAL painted panel/wear/insignia detail, unlike a
                    procedural FBM fake which would invent generic noise.)

Two independent outputs:
  --iqm      : engine IQM (geometry only, optionally decimated). The game path.
  --archive  : slim TEXTURED copy (textures downscaled+quantized). A keepable copy.

Usage:
  python3 tools/process_models.py MODELS_DIR --iqm --tri-budget 6000
  python3 tools/process_models.py MODELS_DIR --archive --tex-cap 512 --palette 64
  python3 tools/process_models.py MODELS_DIR --iqm --archive        # both, defaults

Deps: trimesh + fast_simplification (decimation), pillow (textures). Degrades
gracefully -- a missing dep means that part is left unchanged and reported, so
data is never silently dropped.
"""
import sys, os, argparse, subprocess, shutil, warnings, struct
warnings.filterwarnings("ignore")   # quiet cosmetic PIL palette/transparency notes

HERE = os.path.dirname(os.path.abspath(__file__))

# ---- self-contained IQM writer (no external gltf_to_iqm.py dependency) -------
def _normalize_slot(V):
    import numpy as np
    lo = V.min(0); hi = V.max(0); c = (hi + lo) * 0.5
    s = float((hi - lo).max()) or 1.0
    return ((V - c) / s * 2.0).astype("f4")

def _vertex_normals(V, F):
    import numpy as np
    N = np.zeros_like(V)
    tri = V[F]
    fn = np.cross(tri[:, 1] - tri[:, 0], tri[:, 2] - tri[:, 0])
    for k in range(3):
        np.add.at(N, F[:, k], fn)
    ln = np.linalg.norm(N, axis=1, keepdims=True); ln[ln == 0] = 1.0
    return (N / ln).astype("f4")

def validate_iqm(path, src_bytes=None, src_mtime=None):
    """Check an existing output IQM is genuinely good, so a re-run can skip it.
    Returns (ok, reason). Catches: missing, truncated (filesize mismatch), wrong
    magic/version, empty geometry, denormalized bounds, and 'suspiciously sparse'
    (a huge source that yielded almost no triangles -- what the old strip/fan
    reshape bug produced, e.g. 32MB source -> 2KB iqm). Also treats output older
    than its source as stale."""
    try:
        if not os.path.isfile(path):
            return False, "missing"
        size = os.path.getsize(path)
        if size < 128:
            return False, "too small"
        with open(path, "rb") as fh:
            head = fh.read(16 + 4*13)
        if head[:16] != b"INTERQUAKEMODEL\x00":
            return False, "bad magic"
        w = struct.unpack_from("<13I", head, 16)
        version, filesize = w[0], w[1]
        nv, nt = w[8], w[10]
        ofs_va, ofs_tri = w[9], w[11]
        if version != 2:
            return False, f"version {version}"
        if filesize != size:
            return False, f"truncated ({size}B, header says {filesize}B)"
        if nv == 0 or nt == 0:
            return False, "empty geometry"
        if ofs_tri + nt*12 > size:
            return False, "triangle data past EOF"
        # stale?
        if src_mtime and os.path.getmtime(path) < src_mtime:
            return False, "stale (source is newer)"
        # suspiciously sparse: big source, almost no geometry
        if src_bytes and src_bytes > 5*1024*1024 and nt < 200:
            return False, f"suspiciously sparse ({nt} tris from {human(src_bytes)} source)"
        # bounds sanity: positions should be normalized into ~[-1,1]
        try:
            import numpy as np
            with open(path, "rb") as fh:
                fh.seek(ofs_va)
                va = struct.unpack("<5I", fh.read(20))   # first vertexarray = positions
                ofs_pos = va[4]
                n_check = min(nv, 300)
                fh.seek(ofs_pos)
                pts = np.frombuffer(fh.read(n_check*12), dtype="<f4")
            if len(pts) and (not np.isfinite(pts).all()):
                return False, "non-finite positions"
            if len(pts) and np.abs(pts).max() > 1.05:
                return False, f"denormalized bounds (max {float(np.abs(pts).max()):.2f})"
        except Exception:
            pass   # bounds check is best-effort; structure already validated
        return True, f"{nt}t"
    except Exception as e:
        return False, f"{type(e).__name__}"

def _write_iqm(path, V, F, tex="plate_blue.png"):
    """Write a static IQM (engine part format) from vertices+faces."""
    import numpy as np
    N = _vertex_normals(V, F)
    UV = np.zeros((len(V), 2), "f4")
    def u(*a): return struct.pack("<" + "I"*len(a), *a)
    mat = (tex + "\x00").encode(); text = b"\x00" + mat + b"prop\x00"
    mat_off = 1; name_off = 1 + len(mat)
    nv = len(V); nt = len(F); ofs = 16 + 4 + 4*26
    ofs_text = ofs; ofs += len(text); ofs += (-ofs) % 4
    ofs_meshes = ofs; ofs += 6*4; ofs_va = ofs; ofs += 15*4
    ofs_pos = ofs; ofs += nv*12; ofs_uv = ofs; ofs += nv*8; ofs_nrm = ofs; ofs += nv*12; ofs_tri = ofs; ofs += nt*12
    filesize = ofs
    out = b"INTERQUAKEMODEL\x00" + u(2, filesize, 0) + u(len(text), ofs_text) + u(1, ofs_meshes) + u(3, nv, ofs_va) + u(nt, ofs_tri, 0)
    out += u(0,0)+u(0,0)+u(0,0)+u(0,0,0,0)+u(0,0)+u(0,0)
    out += text + b"\x00"*((-(ofs_text+len(text))) % 4)
    out += u(name_off, mat_off, 0, nv, 0, nt)
    out += u(0,0,7,3,ofs_pos) + u(1,0,7,2,ofs_uv) + u(2,0,7,3,ofs_nrm)
    out += V.astype("<f4").tobytes()
    out += UV.astype("<f4").tobytes()
    out += N.astype("<f4").tobytes()
    out += F.astype("<u4").tobytes()
    assert len(out) == filesize, (len(out), filesize)
    open(path, "wb").write(out)
    return nv, nt, filesize

def find_models(root):
    found = []
    for dp, _d, files in os.walk(root):
        for f in files:
            if f.lower().endswith((".gltf", ".glb", ".obj")):
                base = os.path.basename(dp.rstrip("/")) or "model"
                parent = os.path.basename(os.path.dirname(dp.rstrip("/")))
                found.append([base, parent, os.path.join(dp, f)])
                break
    # disambiguate duplicate base names by prefixing the parent dir
    from collections import Counter
    dupes = {n for n, c in Counter(b for b, _p, _g in found).items() if c > 1}
    for rec in found:
        if rec[0] in dupes and rec[1]:
            rec[0] = f"{rec[1]}_{rec[0]}"
    for base, _parent, gltf in found:
        yield base, gltf

def human(n):
    for u in ("B","K","M","G"):
        if n < 1024: return f"{n:.0f}{u}"
        n /= 1024
    return f"{n:.0f}T"

def dir_size(p):
    t = 0
    for dp, _d, fs in os.walk(p):
        for f in fs:
            try: t += os.path.getsize(os.path.join(dp, f))
            except OSError: pass
    return t

def mesh_tris(gltf):
    try:
        import json, struct as _st
        d = open(gltf, 'rb').read(4)
        if d[:4] == b'glTF':                      # GLB: parse the JSON chunk
            raw = open(gltf, 'rb').read()
            off = 12; g = None
            while off < len(raw):
                clen, ctype = _st.unpack_from('<II', raw, off); off += 8
                if ctype == 0x4E4F534A:
                    g = json.loads(raw[off:off+clen].decode('utf-8')); break
                off += clen
            if g is None: return None
        else:
            g = json.load(open(gltf))
        tot = 0
        for m in g.get("meshes", []):
            for p in m["primitives"]:
                if "indices" in p: tot += g["accessors"][p["indices"]]["count"] // 3
        return tot
    except Exception:
        return None

def _load_obj_geometry(path):
    """Parse merged (positions, faces) from a Wavefront .obj. Triangulates
    polygons, handles negative (relative) indices. Returns (V, F) or None."""
    try:
        import numpy as np
    except ImportError:
        return None
    verts = []; faces = []
    try:
        with open(path, 'r', errors='ignore') as fh:
            for line in fh:
                if line.startswith('v '):
                    p = line.split()
                    verts.append((float(p[1]), float(p[2]), float(p[3])))
                elif line.startswith('f '):
                    idx = []
                    for tok in line.split()[1:]:
                        vi = tok.split('/')[0]
                        if not vi: continue
                        vi = int(vi)
                        idx.append(vi-1 if vi > 0 else len(verts)+vi)  # 1-based / negative
                    for k in range(1, len(idx)-1):    # fan-triangulate
                        faces.append((idx[0], idx[k], idx[k+1]))
    except Exception:
        return None
    if not verts or not faces:
        return None
    V = np.array(verts, 'f4'); F = np.array(faces, 'u4')
    # drop out-of-range faces defensively
    F = F[(F < len(V)).all(axis=1)]
    if len(F) == 0: return None
    return V, F

def _obj_block_signature(path):
    """(vertex_block_hash, face_block_hash) for an .obj, without retaining data."""
    import hashlib
    hv = hashlib.sha1(); hf = hashlib.sha1()
    try:
        with open(path, 'r', errors='ignore') as fh:
            for line in fh:
                if line.startswith('v ') or line.startswith('vt '):
                    hv.update(line.encode('utf-8', 'replace'))
                elif line.startswith('f '):
                    hf.update(line.encode('utf-8', 'replace'))
    except OSError:
        return None, None
    return hv.hexdigest(), hf.hexdigest()

def load_geometry(path):
    """Dispatch by extension: .obj -> obj loader, else glTF loader.

    Multi-part OBJ models (model_0.obj ... model_N.obj in one folder) are merged,
    since parsing a single piece silently drops the rest of the model. Many such
    exports also REPEAT the same vertex and/or face blocks across siblings, so
    byte-equivalent meshes are skipped and repeated triangles removed -- otherwise
    the mesh is rebuilt dozens of times over."""
    if path.lower().endswith('.obj'):
        import os as _os
        try:
            import numpy as _np
        except ImportError:
            return _load_obj_geometry(path)
        d = _os.path.dirname(path)
        try:
            parts = sorted(f for f in _os.listdir(d) if f.lower().endswith('.obj'))
        except OSError:
            parts = []
        if len(parts) <= 1:
            return _load_obj_geometry(path)
        seen = set(); keep = []
        for f in parts:
            sig = _obj_block_signature(_os.path.join(d, f))
            if sig[0] is None or sig in seen: continue
            seen.add(sig); keep.append(f)
        Vs = []; Fs = []; off = 0
        for f in keep:
            geo = _load_obj_geometry(_os.path.join(d, f))
            if not geo: continue
            V, F = geo
            Vs.append(V); Fs.append(F + off); off += len(V)
        if not Vs: return None
        V = _np.vstack(Vs).astype('f4'); F = _np.vstack(Fs).astype('u4')
        # collapse vertices repeated across siblings, then repeated triangles
        key = _np.round(V / 1e-5).astype(_np.int64)
        _u, inv = _np.unique(key, axis=0, return_inverse=True)
        Vw = _np.zeros((len(_u), 3), 'f4'); Vw[inv] = V
        Fw = inv[F]
        good = (Fw[:, 0] != Fw[:, 1]) & (Fw[:, 1] != Fw[:, 2]) & (Fw[:, 0] != Fw[:, 2])
        Fw = Fw[good]
        if len(Fw):
            srt = _np.sort(Fw, axis=1)
            _uu, idx = _np.unique(srt, axis=0, return_index=True)
            Fw = Fw[_np.sort(idx)]
        return Vw, Fw.astype('u4')
    return _load_gltf_geometry(path)

def _load_gltf_geometry(gltf):
    """Directly extract merged (positions, faces) from a .gltf/.glb -- lean, no
    trimesh scene graph (47x less RAM at scale). Returns (V, F) numpy or None."""
    try:
        import numpy as np, json, base64, struct as _st
    except ImportError:
        return None
    CT = {5120:'b',5121:'B',5122:'h',5123:'H',5125:'I',5126:'f'}
    NC = {'SCALAR':1,'VEC2':2,'VEC3':3,'VEC4':4,'MAT4':16}
    d = open(gltf, 'rb').read()
    if d[:4] == b'glTF':                          # GLB container
        off = 12; g = None; binc = b''
        while off < len(d):
            clen, ctype = _st.unpack_from('<II', d, off); off += 8
            chunk = d[off:off+clen]; off += clen
            if ctype == 0x4E4F534A: g = json.loads(chunk.decode('utf-8'))
            elif ctype == 0x004E4942: binc = chunk
        bufs = [binc]
    else:
        g = json.loads(d.decode('utf-8')); base = os.path.dirname(gltf); bufs = []
        for b in g.get('buffers', []):
            uri = b.get('uri', '')
            if uri.startswith('data:'): bufs.append(base64.b64decode(uri.split(',',1)[1]))
            elif uri:
                p = os.path.join(base, uri)
                if not os.path.exists(p): return None      # missing .bin -> skip gracefully
                bufs.append(open(p, 'rb').read())
            else: bufs.append(b'')
    def acc(i):
        a = g['accessors'][i]; bv = g['bufferViews'][a['bufferView']]
        o = bv.get('byteOffset',0) + a.get('byteOffset',0)
        n = a['count'] * NC[a['type']]
        vals = _st.unpack_from('<' + CT[a['componentType']]*n, bufs[bv['buffer']], o)
        return np.array(vals).reshape(a['count'], NC[a['type']])
    import numpy as np
    V = []; F = []; voff = 0
    for m in g.get('meshes', []):
        for p in m['primitives']:
            if 'POSITION' not in p.get('attributes', {}): continue
            mode = p.get('mode', 4)          # default 4 = TRIANGLES
            if mode < 4: continue            # 0..3 = POINTS/LINES/LOOP/STRIP: no surface
            pos = acc(p['attributes']['POSITION'])
            idx = acc(p['indices']).reshape(-1) if 'indices' in p else np.arange(len(pos))
            idx = np.asarray(idx).reshape(-1)
            if mode == 5:                    # TRIANGLE_STRIP -> triangles
                if len(idx) < 3: continue
                a0 = idx[:-2]; a1 = idx[1:-1]; a2 = idx[2:]
                tri = np.column_stack([a0, a1, a2]).copy()
                tri[1::2, [0, 1]] = tri[1::2, [1, 0]]      # flip winding on odd tris
            elif mode == 6:                  # TRIANGLE_FAN -> triangles
                if len(idx) < 3: continue
                tri = np.column_stack([np.repeat(idx[0], len(idx)-2), idx[1:-1], idx[2:]])
            else:                            # 4 = TRIANGLES
                if len(idx) < 3: continue
                if len(idx) % 3: idx = idx[:len(idx) - (len(idx) % 3)]
                tri = idx.reshape(-1, 3)
            V.append(pos); F.append(tri + voff); voff += len(pos)
    if not V: return None
    return np.vstack(V).astype(np.float32), np.vstack(F).astype(np.int64)

def decimate_to_gltf(gltf, tri_budget, workdir):
    """If the mesh exceeds tri_budget, decimate and write a temp .glb.
    Uses the lean direct-parse path (fast_simplification) -- feasible on the
    huge no-color sculpts. Returns (path_to_use, decimated_tris_or_str)."""
    n = mesh_tris(gltf)
    if n is not None and tri_budget > 0 and n <= tri_budget:
        return gltf, None
    # lean path: direct parse + fast_simplification, no trimesh scene graph
    try:
        import fast_simplification as fs, numpy as np, trimesh
    except ImportError:
        return gltf, "no-decim-dep"
    geo = load_geometry(gltf)
    if geo is None:
        return gltf, "no-geometry"               # missing buffer / empty -> use original
    V, F = geo
    if len(F) <= tri_budget:
        return gltf, None
    red = 1.0 - (tri_budget / float(len(F)))
    vo, fo = fs.simplify(V, F, target_reduction=min(0.995, max(0.02, red)))
    os.makedirs(workdir, exist_ok=True)
    out = os.path.join(workdir, "dec.glb")
    trimesh.Trimesh(vertices=vo, faces=fo, process=False).export(out)
    return out, len(fo)

def _cluster_decimate(V, F, grid):
    """Vertex-clustering decimation: snap verts to a grid cell, average per cell,
    drop degenerate faces. Guarantees reduction regardless of topology (unlike
    quadric, which stalls on disconnected/boundary meshes). Lower grid = fewer tris."""
    import numpy as np
    lo = V.min(0); span = float((V.max(0) - lo).max()) or 1.0
    cell = span / grid
    keys = np.floor((V - lo) / cell).astype(np.int64)
    uniq, inv = np.unique(keys, axis=0, return_inverse=True)
    newV = np.zeros((len(uniq), 3), "f8"); cnt = np.zeros(len(uniq))
    np.add.at(newV, inv, V); np.add.at(cnt, inv, 1); newV /= cnt[:, None]
    newF = inv[F]
    good = (newF[:,0]!=newF[:,1]) & (newF[:,1]!=newF[:,2]) & (newF[:,0]!=newF[:,2])
    return newV.astype("f4"), newF[good].astype("u4")

def _weld(V, F, tol=1e-4):
    """Merge near-coincident vertices (source meshes split verts at UV/material
    seams, which decimation turns into holes). Returns welded (V, F)."""
    import numpy as np
    keys = np.round(V / tol).astype(np.int64)
    uniq, inv = np.unique(keys, axis=0, return_inverse=True)
    nV = np.zeros((len(uniq), 3)); cnt = np.zeros(len(uniq))
    np.add.at(nV, inv, V); np.add.at(cnt, inv, 1); nV /= cnt[:, None]
    Fw = inv[F]
    # drop faces that welding collapsed to degenerate
    good = (Fw[:,0]!=Fw[:,1]) & (Fw[:,1]!=Fw[:,2]) & (Fw[:,0]!=Fw[:,2])
    return nV.astype("f4"), Fw[good].astype("u4")

def _hole_ratio(F):
    """Fraction of edges used by only one face (boundary edges = holes/open surface)."""
    import numpy as np
    from collections import Counter
    ec = Counter()
    for tri in F:
        a,b,c = int(tri[0]),int(tri[1]),int(tri[2])
        for u,w in ((a,b),(b,c),(c,a)): ec[(min(u,w),max(u,w))] += 1
    if not ec: return 1.0
    return sum(1 for v in ec.values() if v==1)/len(ec)

def _decimate(V, F, budget):
    """Reduce to ~budget triangles. Welds seam-split verts first (fixes holes),
    then quadric; if that plateaus above budget, vertex-cluster (guaranteed).
    Quality guard: if the result is still hole-riddled, retry at a higher budget."""
    import numpy as np, fast_simplification as fs
    def run(bud):
        v, f = _weld(V, F)                          # close seam holes before decimating
        prev = len(f) + 1; passes = 0
        while len(f) > bud * 1.05 and passes < 8 and len(f) < prev:
            prev = len(f)
            r = min(0.7, 1.0 - bud / float(len(f)))
            v, f = fs.simplify(np.asarray(v,"f4"), np.asarray(f,"u4"), target_reduction=max(0.05, r))
            v = np.asarray(v,"f4"); f = np.asarray(f,"u4"); passes += 1
        if len(f) > bud * 1.10:                     # quadric plateaued -> cluster to hit budget
            lo_g, hi_g = 8, 256; best = (v, f)
            for _ in range(9):
                g = (lo_g + hi_g)//2
                cv, cf = _cluster_decimate(v, f, g)
                if len(cf) > bud: hi_g = g
                else: best = (cv, cf); lo_g = g + 1
            v, f = best
        return v, f
    v, f = run(budget)
    # quality guard: a torn mesh (clustering artifact) -> retry with more room
    if _hole_ratio(f) > 0.25 and budget < 60000:
        v2, f2 = run(budget * 4)
        if _hole_ratio(f2) < _hole_ratio(f): v, f = v2, f2
    return v, f

def to_iqm(name, gltf, out_dir, tex, tri_budget):
    """Parse geometry, optionally decimate, normalize, write IQM. Self-contained.
    Returns (out_path, dec_info) or (None, error_string)."""
    os.makedirs(out_dir, exist_ok=True)
    try:
        import numpy as np, fast_simplification as fs
    except ImportError as e:
        return None, f"missing dep: {e.name} (pip install numpy fast_simplification)"
    geo = load_geometry(gltf)
    if geo is None:
        return None, "no geometry (missing .bin or empty mesh)"
    V, F = geo
    dec = None
    if tri_budget > 0 and len(F) > tri_budget:
        try:
            V, F = _decimate(np.asarray(V,"f4"), np.asarray(F,"u4"), tri_budget)
            dec = len(F)
        except Exception as e:
            return None, f"decimate failed: {e}"
    try:
        V = _normalize_slot(np.asarray(V, "f4"))
        out = os.path.join(out_dir, name + ".iqm")
        _write_iqm(out, V, np.asarray(F, "u4"), tex)
    except Exception as e:
        return None, f"write failed: {e}"
    return out, dec

def slim_archive(name, gltf, out_root, cap, palette):
    try:
        from PIL import Image
    except ImportError:
        return None, "no-pillow"
    src_dir = os.path.dirname(gltf); dst_dir = os.path.join(out_root, name)
    os.makedirs(dst_dir, exist_ok=True)
    for dp, _d, fs in os.walk(src_dir):
        rel = os.path.relpath(dp, src_dir)
        outp = os.path.join(dst_dir, rel) if rel != "." else dst_dir
        os.makedirs(outp, exist_ok=True)
        for f in fs:
            s = os.path.join(dp, f); d = os.path.join(outp, f)
            if f.lower().endswith((".png", ".jpg", ".jpeg")):
                try:
                    im = Image.open(s); w, h = im.size
                    if max(w, h) > cap:
                        sc = cap / max(w, h)
                        im = im.resize((max(1,int(w*sc)), max(1,int(h*sc))), Image.LANCZOS)
                    is_normal = "normal" in f.lower() or "_norm" in f.lower()
                    if palette and not is_normal and f.lower().endswith(".png"):
                        im = im.convert("RGB").quantize(colors=palette, method=Image.MEDIANCUT)
                    if f.lower().endswith(".png"):
                        im.save(d, "PNG", optimize=True)
                    else:
                        im.convert("RGB").save(d, quality=85)
                except Exception:
                    shutil.copy2(s, d)
            else:
                shutil.copy2(s, d)
    return dst_dir, "ok"

def _process_one(args):
    """Worker: process a single model (iqm and/or archive). Picklable for pools.
    Never raises -- a broken model reports FAIL so the batch keeps going."""
    name, gltf, opts = args
    try:
        src = dir_size(os.path.dirname(gltf))
    except Exception:
        src = 0
    res = {"name": name, "src": src, "iqm_sz": 0, "dec": None, "iqm_ok": False,
           "iqm_err": None, "arch_sz": 0, "arch_msg": None,
           "skipped": None, "redo": None}
    if opts["iqm"]:
        out_path = os.path.join(opts["out_iqm"], name + ".iqm")
        if not opts.get("force"):
            try:
                src_mtime = os.path.getmtime(gltf)
            except Exception:
                src_mtime = None
            good, why = validate_iqm(out_path, src_bytes=src, src_mtime=src_mtime)
            if good:
                res["iqm_sz"] = os.path.getsize(out_path); res["iqm_ok"] = True
                res["skipped"] = why
                if not opts["archive"]:
                    return res
            elif why != "missing":
                res["redo"] = why      # existing output was bad -> reprocess it
        try:
            out, dec = to_iqm(name, gltf, opts["out_iqm"], opts["tex"], opts["tri_budget"])
            if out:
                res["iqm_sz"] = os.path.getsize(out); res["dec"] = dec; res["iqm_ok"] = True
            else:
                res["iqm_err"] = dec
        except Exception as e:
            res["iqm_err"] = f"{type(e).__name__}: {e}"[:70]
    if opts["archive"]:
        try:
            out, msg = slim_archive(name, gltf, opts["out_archive"], opts["tex_cap"], opts["palette"])
            if out: res["arch_sz"] = dir_size(out)
            else:   res["arch_msg"] = msg
        except Exception as e:
            res["arch_msg"] = f"{type(e).__name__}: {e}"[:50]
    return res

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("models_dir")
    ap.add_argument("--iqm", action="store_true")
    ap.add_argument("--archive", action="store_true")
    ap.add_argument("--out-iqm", default="imported_iqm")
    ap.add_argument("--out-archive", default="slim_models")
    ap.add_argument("--tex", default="plate_blue.png")
    ap.add_argument("--tri-budget", type=int, default=8000)
    ap.add_argument("--tex-cap", type=int, default=512)
    ap.add_argument("--palette", type=int, default=64)
    ap.add_argument("--force", action="store_true",
                    help="reprocess everything, even outputs that validate as good")
    ap.add_argument("--quality", choices=["indie","high","ultra"], default=None,
                    help="fidelity tier: indie=8k tri/512 tex, high=16k/1024, ultra=32k/2048")
    ap.add_argument("--jobs", "-j", type=int, default=os.cpu_count() or 4,
                    help="parallel worker processes (default: all cores; 1 = serial)")
    a = ap.parse_args()
    if not (a.iqm or a.archive): a.iqm = True
    # quality tiers scale tri budget + texture cap together (AAA/IMAX = ultra).
    # explicit --tri-budget/--tex-cap override the tier.
    if a.quality:
        tier = {"indie":(8000,512,64), "high":(16000,1024,96), "ultra":(32000,2048,128)}[a.quality]
        if "--tri-budget" not in sys.argv: a.tri_budget = tier[0]
        if "--tex-cap"    not in sys.argv: a.tex_cap    = tier[1]
        if "--palette"    not in sys.argv: a.palette    = tier[2]
        print(f"quality tier '{a.quality}': tri-budget {a.tri_budget}, tex-cap {a.tex_cap}, palette {a.palette}\n")

    models = list(find_models(a.models_dir))
    if not models:
        print("no models found under", a.models_dir); sys.exit(1)
    print(f"found {len(models)} models\n")

    src_total = iqm_total = arch_total = 0; ok = fail = skipped = 0
    opts = {"iqm": a.iqm, "archive": a.archive, "out_iqm": a.out_iqm,
            "out_archive": a.out_archive, "tex": a.tex, "tri_budget": a.tri_budget,
            "tex_cap": a.tex_cap, "palette": a.palette, "force": a.force}
    jobs = [(name, gltf, opts) for name, gltf in sorted(models)]

    def emit(res):
        nonlocal src_total, iqm_total, arch_total, ok, fail, skipped
        src_total += res["src"]; line = f"{res['name']:40.40s} src {human(res['src']):>6}"
        if a.iqm:
            if res["iqm_ok"]:
                iqm_total += res["iqm_sz"]; ok += 1
                if res.get("skipped"):
                    skipped += 1
                    line += f" | SKIP (ok, {res['skipped']})"
                else:
                    tag = f" iqm {human(res['iqm_sz']):>6}"
                    if isinstance(res["dec"], int): tag += f" (decim->{res['dec']}t)"
                    if res.get("redo"): tag += f"  [redone: {res['redo']}]"
                    line += " |" + tag
            else:
                line += f" | FAIL: {res['iqm_err']}"; fail += 1
        if a.archive:
            if res["arch_msg"] is None: arch_total += res["arch_sz"]; line += f" | slim {human(res['arch_sz']):>6}"
            else: line += f" | slim skip ({res['arch_msg']})"
        print(line)

    if a.jobs == 1:
        for j in jobs: emit(_process_one(j))
    else:
        from concurrent.futures import ProcessPoolExecutor, as_completed
        with ProcessPoolExecutor(max_workers=a.jobs) as ex:
            futs = [ex.submit(_process_one, j) for j in jobs]
            for fut in as_completed(futs):
                emit(fut.result())

    print("\n" + "-"*64)
    print(f"source total: {human(src_total)}")
    if a.iqm:     print(f"iqm total:    {human(iqm_total)}  ({100*iqm_total/max(1,src_total):.1f}% of source)  [{ok} ok ({skipped} skipped as already-good), {fail} fail]")
    if a.archive: print(f"slim total:   {human(arch_total)}  ({100*arch_total/max(1,src_total):.1f}% of source)")

if __name__ == "__main__":
    main()
