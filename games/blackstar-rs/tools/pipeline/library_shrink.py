#!/usr/bin/env python3
"""library_shrink.py -- shrink the extracted model library toward a size target
while keeping every model RE-PROCESSABLE.

The library is dominated by two kinds of waste:
  1. GEOMETRY -- sculpting tools (ZBrush dynamesh etc.) emit 10-100M triangle
     meshes. The pipeline's top tier only uses 32k. Decimating the SOURCE to a
     generous working budget (default 250k tris) is invisible to every downstream
     export and typically removes 90-99% of the bytes.
  2. TEXTURES -- 4K/8K maps. Capping to a working resolution keeps them fine for
     re-export at any tier the game actually ships.

Both levers are lossy-but-sufficient: the slimmed library still feeds
process_models / texture_export at full quality for the tiers you ship.

Geometry is rewritten as a single self-describing .glb per model that PRESERVES
material groups, UVs and texture references, so texture_export still produces
correct per-part textured meshes afterwards. Folder names (your classification
metadata) are never touched.

DRY RUN BY DEFAULT. Nothing is deleted or rewritten without --apply.

Usage:
  python3 tools/library_shrink.py LIBRARY_ROOT                    # plan only
  python3 tools/library_shrink.py LIBRARY_ROOT --target 15G       # plan to a target
  python3 tools/library_shrink.py LIBRARY_ROOT --target 15G --apply
  python3 tools/library_shrink.py LIBRARY_ROOT --tri-budget 100000 --cap 1024 --apply
"""
import sys, os, json, struct, argparse, shutil

MODEL_EXT = (".gltf", ".glb", ".obj")
IMG_EXT   = (".png", ".jpg", ".jpeg", ".tga", ".bmp", ".webp", ".tif", ".tiff")
GEO_EXT   = (".gltf", ".glb", ".obj", ".bin", ".fbx", ".stl", ".dae", ".ply")
# never touch the pipeline's own output
SKIP_DIRS = {"parts", "iqm", "imported_iqm", "slim_models", "textured_parts", "build"}

def human(n):
    for u in ("B","K","M","G","T"):
        if n < 1024 or u == "T":
            return f"{n:.0f}{u}" if u == "B" else f"{n:.1f}{u}"
        n /= 1024.0

def parse_size(s):
    s = str(s).strip().upper()
    mult = 1
    for suf, m in (("T",1<<40), ("G",1<<30), ("M",1<<20), ("K",1<<10)):
        if s.endswith(suf): mult = m; s = s[:-1]; break
    return int(float(s) * mult)

def walk(root):
    for dp, dirs, files in os.walk(root):
        dirs[:] = [d for d in dirs if d not in SKIP_DIRS and not d.startswith(".")]
        yield dp, files

def find_models(root):
    """One model file per folder -- the LARGEST .gltf/.glb (else largest .obj).
    Picking the first match risks parsing a small preview/LOD file while the real
    geometry sits in another file in the same folder."""
    out = []
    for dp, files in walk(root):
        gl = [f for f in files if f.lower().endswith((".gltf", ".glb"))]
        ob = [f for f in files if f.lower().endswith(".obj")]
        cand = gl or ob
        if not cand: continue
        def _sz(f):
            try: return os.path.getsize(os.path.join(dp, f))
            except OSError: return 0
        out.append(os.path.join(dp, max(cand, key=_sz)))
    return out

# ---------------------------------------------------------------- GLB writing
def write_glb(path, groups, mat_tex, tex_dir_rel=""):
    """Write a .glb with one primitive per material group, preserving UVs and
    external texture references. groups: {(matname, i): (V, UV, F)}."""
    import numpy as np
    bin_parts = []; offset = 0
    bufferViews = []; accessors = []; meshes_prims = []
    materials = []; images = []; textures = []
    img_index = {}

    def add_view(data, target=None):
        nonlocal offset
        pad = (-len(data)) % 4
        bin_parts.append(data + b"\x00"*pad)
        bv = {"buffer": 0, "byteOffset": offset, "byteLength": len(data)}
        if target: bv["target"] = target
        bufferViews.append(bv)
        offset += len(data) + pad
        return len(bufferViews) - 1

    for (mat, _mi), (V, UV, F) in groups.items():
        V = np.asarray(V, "<f4"); UV = np.asarray(UV, "<f4"); F = np.asarray(F, "<u4")
        if len(F) == 0: continue
        v_bv = add_view(V.tobytes(), 34962)
        accessors.append({"bufferView": v_bv, "componentType": 5126, "count": int(len(V)),
                          "type": "VEC3",
                          "min": [float(x) for x in V.min(axis=0)],
                          "max": [float(x) for x in V.max(axis=0)]})
        a_pos = len(accessors) - 1
        t_bv = add_view(UV.tobytes(), 34962)
        accessors.append({"bufferView": t_bv, "componentType": 5126, "count": int(len(UV)),
                          "type": "VEC2"})
        a_uv = len(accessors) - 1
        i_bv = add_view(F.reshape(-1).tobytes(), 34963)
        accessors.append({"bufferView": i_bv, "componentType": 5125,
                          "count": int(F.size), "type": "SCALAR"})
        a_idx = len(accessors) - 1

        m = {"name": str(mat)[:120], "pbrMetallicRoughness": {"metallicFactor": 0.4,
             "roughnessFactor": 0.6}, "doubleSided": True}
        slots = mat_tex.get(mat) or {}
        if isinstance(slots, str): slots = {"base": slots}   # tolerate flat form

        def _tex_index(fp):
            uri = os.path.join(tex_dir_rel, os.path.basename(fp)).replace("\\", "/") \
                  if tex_dir_rel else os.path.basename(fp)
            if uri not in img_index:
                images.append({"uri": uri}); textures.append({"source": len(images)-1})
                img_index[uri] = len(textures) - 1
            return img_index[uri]

        if slots.get("base"):
            m["pbrMetallicRoughness"]["baseColorTexture"] = {"index": _tex_index(slots["base"])}
        if slots.get("normal"):
            m["normalTexture"] = {"index": _tex_index(slots["normal"])}
        if slots.get("emissive"):
            m["emissiveTexture"] = {"index": _tex_index(slots["emissive"])}
            m["emissiveFactor"] = [1.0, 1.0, 1.0]
        materials.append(m)
        meshes_prims.append({"attributes": {"POSITION": a_pos, "TEXCOORD_0": a_uv},
                             "indices": a_idx, "material": len(materials)-1, "mode": 4})

    if not meshes_prims: return False
    blob = b"".join(bin_parts)
    g = {"asset": {"version": "2.0", "generator": "library_shrink.py"},
         "buffers": [{"byteLength": len(blob)}],
         "bufferViews": bufferViews, "accessors": accessors,
         "materials": materials, "meshes": [{"primitives": meshes_prims}],
         "nodes": [{"mesh": 0}], "scenes": [{"nodes": [0]}], "scene": 0}
    if images:  g["images"] = images
    if textures: g["textures"] = textures
    js = json.dumps(g, separators=(",", ":")).encode()
    js += b" " * ((-len(js)) % 4)
    total = 12 + 8 + len(js) + 8 + len(blob)
    with open(path, "wb") as fh:
        fh.write(b"glTF" + struct.pack("<II", 2, total))
        fh.write(struct.pack("<I", len(js)) + b"JSON" + js)
        fh.write(struct.pack("<I", len(blob)) + b"BIN\x00" + blob)
    return True

# ---------------------------------------------------------------- geometry
_HELPERS = {"te": None, "pm": None, "err": None}

def load_helpers():
    """Import texture_export / process_models from beside this script, tolerating
    VERSIONED filenames (texture_export_v9.py, process_models_v6.py -- pick the
    highest version). Resolved once so failure is reported, not silently zeroed."""
    import importlib.util, glob, re
    if _HELPERS["te"] and _HELPERS["pm"]:
        return _HELPERS["te"], _HELPERS["pm"]
    here = os.path.dirname(os.path.abspath(__file__))
    if here not in sys.path: sys.path.insert(0, here)

    def _imp(stem):
        try:
            return __import__(stem)
        except Exception:
            pass
        best, bestv = None, -1
        for c in glob.glob(os.path.join(here, stem + "*.py")):
            b = os.path.basename(c)[:-3]
            if b == os.path.splitext(os.path.basename(__file__))[0]: continue
            m = re.search(r"_v(\d+)$", b)
            v = int(m.group(1)) if m else 0
            if v > bestv: bestv, best = v, b
        if not best: return None
        try:
            spec = importlib.util.spec_from_file_location(best, os.path.join(here, best + ".py"))
            mod = importlib.util.module_from_spec(spec)
            sys.modules[best] = mod
            spec.loader.exec_module(mod)
            return mod
        except Exception as e:
            _HELPERS["err"] = f"{best}.py failed to import: {type(e).__name__}: {e}"
            return None

    te = _imp("texture_export"); pm = _imp("process_models")
    missing = [n for n, m in (("texture_export", te), ("process_models", pm)) if m is None]
    if missing and not _HELPERS["err"]:
        _HELPERS["err"] = ("missing helper tool(s): " + ", ".join(missing) +
                           " -- put them (any _vN suffix is fine) beside this script")
    _HELPERS["te"], _HELPERS["pm"] = te, pm
    return te, pm

def _resolve_tex(path):
    """Return the texture file that actually exists -- textures may have been
    converted .png <-> .jpg by the texture pass, so try both."""
    if os.path.exists(path): return path
    stem, ext = os.path.splitext(path)
    for alt in (".jpg", ".png", ".jpeg"):
        if alt != ext.lower() and os.path.exists(stem + alt):
            return stem + alt
    return None

def slim_geometry(model_path, tri_budget, apply=False):
    """Decimate a model's geometry into a slim .glb beside it. Returns
    (before_bytes, after_bytes, note)."""
    import numpy as np
    te, pm = load_helpers()
    if te is None or pm is None:
        return 0, 0, "helpers unavailable"

    d = os.path.dirname(model_path)

    def _consumed_files(path):
        """The files this parse actually reads: the model file + any buffer/.bin
        (and .mtl) it references. NOTHING else in the folder is ours to delete."""
        got = []
        try: got.append((path, os.path.getsize(path)))
        except OSError: return []
        low = path.lower()
        try:
            if low.endswith(".gltf"):
                gj = json.loads(open(path, "rb").read().decode("utf-8", "replace"))
                for b in gj.get("buffers", []):
                    uri = b.get("uri", "")
                    if uri and not uri.startswith("data:"):
                        p = os.path.join(os.path.dirname(path), uri)
                        if os.path.exists(p): got.append((p, os.path.getsize(p)))
            elif low.endswith(".obj"):
                # multi-part OBJ: every sibling piece belongs to this model
                dd = os.path.dirname(path)
                for f in sorted(os.listdir(dd)):
                    fp = os.path.join(dd, f)
                    if f.lower().endswith(".obj") and os.path.abspath(fp) != os.path.abspath(path):
                        try: got.append((fp, os.path.getsize(fp)))
                        except OSError: pass
                seen_mtl = set()
                for op, _sz in list(got):
                    if not op.lower().endswith(".obj"): continue
                    try:
                        for line in open(op, "r", errors="ignore"):
                            if line.startswith("mtllib"):
                                mp = os.path.join(dd, line.split(maxsplit=1)[1].strip())
                                if os.path.exists(mp) and mp not in seen_mtl:
                                    seen_mtl.add(mp); got.append((mp, os.path.getsize(mp)))
                                break
                    except Exception:
                        pass
        except Exception:
            pass
        return got

    consumed = _consumed_files(model_path)
    before = sum(sz for _p, sz in consumed)
    if before == 0: return 0, 0, "no geometry"

    try:
        if model_path.lower().endswith(".obj"):
            groups, mat_tex, _base = te.load_obj_by_material(model_path)
            mat_tex = {k: {"base": _resolve_tex(v)} for k, v in mat_tex.items()}
            mat_tex = {k: v for k, v in mat_tex.items() if v.get("base")}
        else:
            groups, g, base = te.load_by_material(model_path)
            mat_tex = {}
            def _uri_for(mi, slot):
                """material -> texture file, resolved against what's on disk."""
                try:
                    mm = g["materials"][mi]
                    if slot == "base":
                        ti = mm.get("pbrMetallicRoughness", {}).get("baseColorTexture", {}).get("index")
                    elif slot == "normal":
                        ti = mm.get("normalTexture", {}).get("index")
                    else:
                        ti = mm.get("emissiveTexture", {}).get("index")
                    if ti is None: return None
                    src = g["textures"][ti].get("source")
                    uri = g["images"][src].get("uri")
                    if not uri: return None
                    return _resolve_tex(os.path.join(base, uri))
                except Exception:
                    return None
            for (mat, mi) in list(groups.keys()):
                if mi is None: continue
                slots = {k: _uri_for(mi, k) for k in ("base", "normal", "emissive")}
                slots = {k: v for k, v in slots.items() if v}
                if slots: mat_tex[mat] = slots
    except Exception as e:
        return before, before, f"parse failed: {type(e).__name__}"
    if not groups: return before, before, "no geometry parsed"

    total_tris = sum(len(F) for (_V, _UV, F) in groups.values())
    if total_tris == 0: return before, before, "no triangles"

    # decimate each group proportionally to its share of the budget
    slim = {}
    for key, (V, UV, F) in groups.items():
        share = max(64, int(tri_budget * (len(F) / total_tris)))
        if len(F) > share:
            try:
                Vd, Fd = pm._decimate(np.asarray(V, "f4"), np.asarray(F, "u4"), share)
                # decimation drops UVs -> nearest-source-vertex UV transfer
                UVd = _transfer_uv(np.asarray(V, "f4"), np.asarray(UV, "f4"), Vd)
                slim[key] = (Vd, UVd, Fd)
            except Exception:
                slim[key] = (V, UV, F)
        else:
            slim[key] = (V, UV, F)

    kept = sum(len(F) for (_V, _UV, F) in slim.values())
    # EXACT slim payload size: we keep only POSITION(12B) + TEXCOORD_0(8B) + u32 indices
    payload = sum(len(V)*12 + len(UV)*8 + F.size*4 for (V, UV, F) in
                  ((np.asarray(v, "f4"), np.asarray(u, "f4"), np.asarray(f, "u4"))
                   for (v, u, f) in slim.values()))
    est = int(payload * 1.02) + 2048          # + json/chunk overhead
    if kept == total_tris:
        note = f"{kept} tris, attrs stripped"
    else:
        note = f"{total_tris}->{kept} tris"
        # decimation can undershoot badly: proportional per-group budgets plus the
        # decimator's clustering fallback. If the result lands under the top export
        # tier, the slimmed source would silently cap quality downstream.
        if kept < tri_budget * 0.5:
            note += f"  !! UNDERSHOT budget ({100.0*kept/max(1,tri_budget):.0f}% of {tri_budget})"

    # PLAUSIBILITY GUARD: a real mesh runs ~40-80 bytes/triangle. If the parse
    # captured far too little for the bytes consumed, it is PARTIAL -- replacing
    # the source would destroy geometry we never read. Refuse.
    # Bytes we actually READ. For multi-part OBJ, duplicate sibling files are
    # skipped by the loader, so their bytes must not count against the parse.
    parse_bytes = before
    if model_path.lower().endswith(".obj"):
        try:
            seen = set(); pb = 0
            for f in sorted(os.listdir(d)):
                if not f.lower().endswith(".obj"): continue
                fp = os.path.join(d, f)
                sig = te._obj_signature(fp)
                if sig[0] is None or sig in seen: continue
                seen.add(sig); pb += os.path.getsize(fp)
            if pb > 0: parse_bytes = pb
        except Exception:
            pass

    # PLAUSIBILITY GUARD: judge on triangles PARSED (pre-decimation) against the
    # bytes actually read. Using post-decimation counts made every model that hit
    # the tri budget look like a failed parse.
    bytes_per_tri = parse_bytes / max(1, total_tris)
    if parse_bytes > 8*1024*1024 and bytes_per_tri > 4000:
        why = ""
        try:
            if model_path.lower().endswith(".gltf"):
                head = open(model_path, "rb").read(200000).decode("utf-8", "replace")
                if "KHR_draco_mesh_compression" in head:
                    why = " [Draco-compressed - needs a Draco decoder]"
                elif "EXT_meshopt_compression" in head:
                    why = " [meshopt-compressed]"
                elif "\"sparse\"" in head:
                    why = " [sparse accessors]"
        except Exception:
            pass
        return before, before, (f"SUSPECT parse ({total_tris} tris parsed from "
                                f"{human(parse_bytes)} read){why} - skipped")

    out_glb = os.path.join(d, "slim.glb")
    if not apply:
        # never claim a saving if the rewrite would be larger
        return before, min(before, est), note

    tex_rel = ""
    if any(os.path.isdir(os.path.join(d, x)) for x in ("textures", "Textures")):
        tex_rel = "textures" if os.path.isdir(os.path.join(d, "textures")) else "Textures"
    if not write_glb(out_glb, slim, mat_tex, tex_rel):
        return before, before, "glb write produced nothing"

    # remove ONLY the files this parse consumed -- never other models in the folder
    for p, _sz in consumed:
        if os.path.abspath(p) == os.path.abspath(out_glb): continue
        try: os.remove(p)
        except OSError: pass
    after = os.path.getsize(out_glb)
    return before, after, note

def _transfer_uv(V_src, UV_src, V_dst, chunk=4096):
    """Nearest-neighbour UV transfer from the original vertices to the decimated
    ones (decimation returns positions only)."""
    import numpy as np
    if len(UV_src) != len(V_src) or len(V_src) == 0:
        return np.zeros((len(V_dst), 2), "f4")
    out = np.zeros((len(V_dst), 2), "f4")
    try:
        from scipy.spatial import cKDTree
        _d, idx = cKDTree(V_src).query(V_dst)
        return UV_src[idx].astype("f4")
    except Exception:
        pass
    for i in range(0, len(V_dst), chunk):
        blk = V_dst[i:i+chunk]
        d2 = ((blk[:, None, :] - V_src[None, :, :])**2).sum(-1)
        out[i:i+chunk] = UV_src[d2.argmin(1)]
    return out

# ---------------------------------------------------------------- textures
def slim_texture(path, cap, quality=92, apply=False, fast=False):
    """Downscale (and where safe, JPEG-ify) one texture. Returns (before, after).

    In dry-run mode the result is EXACT by default: the image is encoded to memory
    and measured. Compression depends on image content, so a fixed ratio is wrong
    for some files -- and this number drives an irreversible decision. `fast=True`
    falls back to a rough ratio estimate."""
    try:
        before = os.path.getsize(path)
    except OSError:
        return 0, 0
    try:
        from PIL import Image
    except ImportError:
        return before, before
    base = os.path.basename(path).lower()
    is_map = any(k in base for k in ("norm", "nrm", "_n.", "spec", "rough", "metal", "orm"))
    try:
        im = Image.open(path)
        w, h = im.size
        need_resize = max(w, h) > cap
        im2 = im.convert("RGBA") if im.mode in ("RGBA", "LA", "P") else im.convert("RGB")
        has_alpha = (im2.mode == "RGBA" and im2.getextrema()[3][0] < 255)
        use_jpeg = (not is_map) and (not has_alpha)

        if not apply and fast:
            scale = (cap/max(w, h))**2 if need_resize else 1.0
            return before, int(before * scale * (0.16 if use_jpeg else 0.62))

        if need_resize:
            r = cap / max(w, h)
            im2 = im2.resize((max(1, int(w*r)), max(1, int(h*r))), Image.LANCZOS)

        if not apply:
            import io
            buf = io.BytesIO()
            if use_jpeg: im2.convert("RGB").save(buf, "JPEG", quality=quality, optimize=True)
            else:        im2.save(buf, "PNG", optimize=True)
            # never claim a saving if the rewrite would be larger
            return before, min(before, buf.tell())
        if use_jpeg:
            dst = os.path.splitext(path)[0] + ".jpg"
            im2.convert("RGB").save(dst, "JPEG", quality=quality, optimize=True)
            if dst != path: os.remove(path)
        else:
            dst = os.path.splitext(path)[0] + ".png"
            im2.save(dst, "PNG", optimize=True)
            if dst != path: os.remove(path)
        return before, os.path.getsize(dst)
    except Exception:
        return before, before

# ---------------------------------------------------------------- main
def _tex_job(args):
    """Picklable worker: shrink one texture. Returns (path, before, after)."""
    path, cap, quality, apply, fast = args
    b, a = slim_texture(path, cap, quality, apply, fast)
    return path, b, a

def _geo_job(args):
    """Picklable worker: slim one model's geometry. Returns (path, before, after, note)."""
    model_path, tri_budget, apply = args
    b, a, note = slim_geometry(model_path, tri_budget, apply)
    return model_path, b, a, note

def _run_pool(fn, jobs, workers, on_result, label):
    """Run fn over jobs with `workers` processes (1 = in-process), calling
    on_result(result) as each completes. Falls back to serial if pools fail."""
    done = 0; total = len(jobs)
    if workers <= 1:
        for j in jobs:
            on_result(fn(j)); done += 1
            if done % 25 == 0:
                print(f"  ...{done}/{total} {label}", end="\r", flush=True)
        return
    try:
        from concurrent.futures import ProcessPoolExecutor, as_completed
        with ProcessPoolExecutor(max_workers=workers) as ex:
            futs = [ex.submit(fn, j) for j in jobs]
            for fut in as_completed(futs):
                on_result(fut.result()); done += 1
                if done % 25 == 0:
                    print(f"  ...{done}/{total} {label}", end="\r", flush=True)
    except Exception as e:
        print(f"  (parallel pool unavailable: {type(e).__name__}; falling back to serial)")
        for j in jobs[done:]:
            on_result(fn(j)); done += 1

def report_unconsumed(root, models, out_lines=12):
    """Geometry bytes that NO model parse reads. Downloads often ship the same
    model in several formats (.fbx/.obj/.dae beside the .gltf); those copies are
    invisible to the pipeline but dominate disk. Report, never auto-delete."""
    consumed = set()
    for m in models:
        d = os.path.dirname(m)
        consumed.add(os.path.abspath(m))
        low = m.lower()
        try:
            if low.endswith(".gltf"):
                gj = json.loads(open(m, "rb").read().decode("utf-8", "replace"))
                for b in gj.get("buffers", []):
                    uri = b.get("uri", "")
                    if uri and not uri.startswith("data:"):
                        consumed.add(os.path.abspath(os.path.join(d, uri)))
            elif low.endswith(".obj"):
                for f in os.listdir(d):
                    if f.lower().endswith(".obj"):
                        consumed.add(os.path.abspath(os.path.join(d, f)))
                for line in open(m, "r", errors="ignore"):
                    if line.startswith("mtllib"):
                        consumed.add(os.path.abspath(os.path.join(d, line.split(maxsplit=1)[1].strip())))
                        break
        except Exception:
            pass
    by_ext = {}; biggest = []
    for dp, files in walk(root):
        for f in files:
            if not f.lower().endswith(GEO_EXT): continue
            p = os.path.abspath(os.path.join(dp, f))
            if p in consumed: continue
            try: sz = os.path.getsize(p)
            except OSError: continue
            ext = os.path.splitext(f)[1].lower()
            by_ext[ext] = by_ext.get(ext, [0, 0])
            by_ext[ext][0] += 1; by_ext[ext][1] += sz
            biggest.append((sz, os.path.relpath(p, root)))
    total = sum(v[1] for v in by_ext.values())
    if total == 0: return 0
    print(f"UNCONSUMED GEOMETRY  {human(total)} in files no parse reads")
    for ext, (n, sz) in sorted(by_ext.items(), key=lambda kv: -kv[1][1]):
        print(f"    {ext:8s} {n:6d} files  {human(sz):>8}")
    biggest.sort(reverse=True)
    for sz, rel in biggest[:out_lines]:
        print(f"      {human(sz):>8}  {rel[:78]}")
    if len(biggest) > out_lines:
        print(f"      ...and {len(biggest)-out_lines} more")
    print("    ^ these are NOT read by the pipeline. They may be alternate-format")
    print("      exports (safe to drop) OR real model data this tool cannot parse")
    print("      (e.g. .fbx). INSPECT before deleting anything -- multi-part .obj")
    print("      pieces are model data and are already handled, not listed here.\n")
    return total

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("root")
    ap.add_argument("--target", default=None, help="desired total size, e.g. 15G")
    ap.add_argument("--tri-budget", type=int, default=250000,
                    help="source working triangle budget per model (default 250k; "
                         "the ultra export tier only uses 32k)")
    ap.add_argument("--cap", type=int, default=2048, help="source texture cap (default 2048)")
    ap.add_argument("--quality", type=int, default=92)
    ap.add_argument("--apply", action="store_true", help="actually rewrite/delete (default: dry run)")
    ap.add_argument("--fast", action="store_true",
                    help="rough ratio estimate in dry run instead of exact encoding")
    ap.add_argument("-j", "--jobs", type=int, default=0,
                    help="parallel workers (default: all cores; 1 = serial)")
    ap.add_argument("--geometry-only", action="store_true")
    ap.add_argument("--textures-only", action="store_true")
    a = ap.parse_args()
    if a.jobs <= 0:
        a.jobs = max(1, (os.cpu_count() or 4))

    total = 0; tex_files = []; 
    for dp, files in walk(a.root):
        for f in files:
            p = os.path.join(dp, f)
            try: sz = os.path.getsize(p)
            except OSError: continue
            total += sz
            if f.lower().endswith(IMG_EXT): tex_files.append(p)
    models = find_models(a.root)
    print(f"library: {human(total)}, {len(models)} models, {len(tex_files)} textures")
    if a.target:
        print(f"target:  {human(parse_size(a.target))}")
    print(f"mode:    {'APPLY (rewrites + deletes)' if a.apply else 'DRY RUN (nothing changed)'}\n")

    tex_before = tex_after = 0
    if not a.geometry_only:
        print(f"TEXTURES  (cap {a.cap}px, colour -> JPEG q{a.quality}, {a.jobs} workers)")
        def _tex_result(r):
            nonlocal tex_before, tex_after
            _p, b, af = r
            tex_before += b; tex_after += af
        _run_pool(_tex_job, [(t, a.cap, a.quality, a.apply, a.fast) for t in tex_files],
                  a.jobs, _tex_result, "textures")
        print(f"  textures {human(tex_before)} -> {human(tex_after)} "
              f"(saves {human(tex_before-tex_after)})" + " "*20 + "\n")

    geo_before = geo_after = 0
    if not a.textures_only:
        te, pm = load_helpers()
        if te is None or pm is None:
            print("GEOMETRY  SKIPPED -- " + (_HELPERS["err"] or "helpers unavailable"))
            print("  (geometry is usually the LARGER lever; fix this before trusting the plan)\n")
        else:
            print(f"GEOMETRY  (working budget {a.tri_budget} tris/model, {a.jobs} workers)")
            suspects = []; undershot = []
            def _geo_result(r):
                nonlocal geo_before, geo_after
                m, b, af, note = r
                geo_before += b; geo_after += af
                if "SUSPECT" in note or "no geometry parsed" in note:
                    suspects.append((os.path.basename(os.path.dirname(m)), b, note))
                elif "UNDERSHOT" in note:
                    undershot.append((os.path.basename(os.path.dirname(m)), b, note))
                elif b > 20*1024*1024:      # only chatter about the big ones
                    print(f"  {os.path.basename(os.path.dirname(m))[:52]:52s} "
                          f"{human(b):>7} -> {human(af):>7}  {note}")
            _run_pool(_geo_job, [(m, a.tri_budget, a.apply) for m in models],
                      a.jobs, _geo_result, "models")
            print(f"  geometry {human(geo_before)} -> {human(geo_after)} "
                  f"(saves {human(geo_before-geo_after)})" + " "*20)
            if undershot:
                print(f"\n  ~ {len(undershot)} model(s) decimated well below the budget "
                      f"(quality may be capped downstream):")
                for n, b, t in sorted(undershot, key=lambda x: -x[1])[:8]:
                    print(f"       {n[:52]:52s} {human(b):>8}  {t.split('!!')[1].strip()}")
                if len(undershot) > 8:
                    print(f"       ...and {len(undershot)-8} more")
                print("    (re-run just these with a higher --tri-budget if they matter)")
            if suspects:
                sus_bytes = sum(b for _n, b, _t in suspects)
                print(f"\n  !! {len(suspects)} model(s) LEFT UNTOUCHED ({human(sus_bytes)}) -- "
                      f"the parse looked partial, so replacing them was refused:")
                for n, b, t in sorted(suspects, key=lambda x: -x[1])[:12]:
                    print(f"       {n[:52]:52s} {human(b):>8}  {t}")
                if len(suspects) > 12:
                    print(f"       ...and {len(suspects)-12} more")
            print()

    unconsumed = report_unconsumed(a.root, models) if not a.textures_only else 0

    saved = (geo_before-geo_after) + (tex_before-tex_after)
    projected = total - saved
    print("-"*60)
    print(f"{'projected' if not a.apply else 'result'}: {human(total)} -> {human(projected)} "
          f"(saved {human(saved)}, {100*saved/max(1,total):.0f}%)")
    if a.target:
        tgt = parse_size(a.target)
        if projected <= tgt:
            print(f"target {human(tgt)} MET")
        else:
            gap = projected - tgt
            print(f"target {human(tgt)} not met (over by {human(gap)})")
            if unconsumed > gap * 0.5:
                print(f"  -> the UNCONSUMED GEOMETRY above ({human(unconsumed)}) would close it;"
                      f" that is dead weight the pipeline never reads.")
            else:
                print(f"  -> try --tri-budget {max(20000, a.tri_budget//4)} "
                      f"and/or --cap {max(512, a.cap//2)}")
    if not a.apply:
        print("\nthis was a DRY RUN. re-run with --apply to execute.")

if __name__ == "__main__":
    main()
