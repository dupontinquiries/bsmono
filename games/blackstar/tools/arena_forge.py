#!/usr/bin/env python3
"""
arena_forge.py — the Cybertron arena forge: a JSON instruction pipeline plus a
top-down GUI for authoring playable arenas, in the mould of soldier's map forge
(`buildscript.rs`) and paperbound's `rts_editor.py`.

A document is a SEQUENCE OF OPS, not a snapshot — natural to generate
incrementally, easy to review as a diff, and authorable by an LLM given only
the schema below (names, not IDs; unknown/bad ops are collected as errors and
skipped, never aborting the whole script). `apply()` turns the document into
the same vertex/tri/mesh soup gen_arena.py builds by hand, and `write_iqm()`
emits a static IQM v2 the engine loads exactly like the stock arena.

Document:
    {
      "name": "ashfall",                    // → engine_src/data/<name>.iqm
      "size": 60.0,                         // half-extent; arena is 2S × 2S
      "wall_height": 8.0,
      "textures": {"floor": "floor.png", "wall": "wall.png", "rock": "rock.png"},
      "ops": [
        {"op": "platform", "x": 0, "z": 0, "w": 8, "d": 8, "h": 2},
        {"op": "ramp", "x": 0, "z": 11.5, "w": 3, "run": 3.5, "h": 2, "facing": "+z"},
        {"op": "pillar", "x": 20, "z": -14, "w": 1.8, "d": 1.8, "h": 5},
        {"op": "box", "x": -18, "z": 6, "w": 3, "d": 2, "h": 1.5, "mat": "wall"},
        {"op": "scatter_pillars", "count": 8, "r_min": 22, "r_max": 48,
         "h_min": 3, "h_max": 7, "seed": 7},
        {"op": "spawn", "x": -40, "z": -40},          // recorded in the sidecar
        {"op": "spawn", "x": 40, "z": 40}
      ]
    }

Every geometry op defaults `mat` to "rock". `apply()` also emits a compact
SIDECAR (<name>.arena.json beside the IQM) holding spawns + a flat obstacle
list (x, z, half-w, half-d, h) so game code / bots can consume the layout
without parsing the mesh.

Run:
    python3 tools/arena_forge.py build doc.json        # doc → IQM + sidecar
    python3 tools/arena_forge.py example > doc.json    # a starter document
    python3 tools/arena_forge.py edit [doc.json]       # top-down GUI editor
    python3 tools/arena_forge.py --selftest            # headless verification

GUI (edit mode):
    Left-click empty     — place the selected brush (1 pillar / 2 box /
                           3 platform / 4 ramp / 5 spawn)
    Left-click a shape   — select it;  drag moves it
    R                    — rotate ramp facing;  [ ] — shrink / grow selection
    Delete               — remove selection;  Ctrl+S — save doc;  E — export IQM
    Scroll               — zoom;  ESC — quit
"""

import os, sys, json, math, random, struct

HERE = os.path.dirname(os.path.abspath(__file__))
DATA_DIR = os.path.normpath(os.path.join(HERE, "..", "engine_src", "data"))

FACINGS = ("+z", "-z", "+x", "-x")


# ── geometry kernel (gen_arena.py's primitives, packaged) ────────────────────

class Builder:
    def __init__(self):
        self.verts = []     # (pos3, uv2, normal3)
        self.tris = []      # (a, b, c)
        self.buckets = {"floor": [], "wall": [], "rock": []}

    def quad(self, mat, a, b, c, d, n, uvscale=0.25):
        bucket = self.buckets[mat]
        base = len(self.verts)
        ax = 0 if abs(n[0]) < 0.9 else 2
        ay = 1 if abs(n[1]) < 0.9 else 2
        for p in (a, b, c, d):
            self.verts.append((p, (p[ax] * uvscale, p[ay] * uvscale), n))
        self.tris.append((base, base + 1, base + 2))
        self.tris.append((base, base + 2, base + 3))
        bucket.append((base, 4, len(self.tris) - 2, 2))

    def box(self, mat, cx, cy, cz, hx, hy, hz):
        x0, x1, y0, y1, z0, z1 = cx - hx, cx + hx, cy - hy, cy + hy, cz - hz, cz + hz
        q = self.quad
        q(mat, (x0, y1, z0), (x0, y1, z1), (x1, y1, z1), (x1, y1, z0), (0, 1, 0))
        q(mat, (x0, y0, z1), (x0, y0, z0), (x1, y0, z0), (x1, y0, z1), (0, -1, 0))
        q(mat, (x0, y0, z1), (x1, y0, z1), (x1, y1, z1), (x0, y1, z1), (0, 0, 1))
        q(mat, (x1, y0, z0), (x0, y0, z0), (x0, y1, z0), (x1, y1, z0), (0, 0, -1))
        q(mat, (x1, y0, z1), (x1, y0, z0), (x1, y1, z0), (x1, y1, z1), (1, 0, 0))
        q(mat, (x0, y0, z0), (x0, y0, z1), (x0, y1, z1), (x0, y1, z0), (-1, 0, 0))

    def wedge(self, mat, cx, cz, w, run, h, facing):
        def rot(p):
            x, y, z = p
            if facing == "+z": return (x, y, z)
            if facing == "-z": return (-x, y, -z)
            if facing == "+x": return (z, y, x)
            return (-z, y, -x)
        def P(x, y, z):
            r = rot((x, y, z)); return (cx + r[0], y, cz + r[2])
        lo0, lo1 = P(-w, 0, run), P(w, 0, run)
        hi0, hi1 = P(-w, h, 0), P(w, h, 0)
        g0, g1 = P(-w, 0, 0), P(w, 0, 0)
        n = (0, 0.8, 0.6) if facing in ("+z", "-z") else (0.6, 0.8, 0)
        self.quad(mat, lo0, lo1, hi1, hi0, n)
        for a, b, c in ((lo0, hi0, g0), (lo1, g1, hi1)):
            base = len(self.verts)
            for p in (a, b, c):
                self.verts.append((p, (p[0] * 0.25, p[1] * 0.25), (1, 0, 0)))
            self.tris.append((base, base + 1, base + 2))
            self.buckets[mat].append((base, 3, len(self.tris) - 1, 1))


# ── the op pipeline ──────────────────────────────────────────────────────────

def default_doc(name="forged"):
    return {"name": name, "size": 60.0, "wall_height": 8.0,
            "textures": {"floor": "floor.png", "wall": "wall.png", "rock": "rock.png"},
            "ops": []}


def apply(doc):
    """doc → (Builder, sidecar, errors). Per-op failures are collected, the rest
    of the script still applies — an LLM's one typo shouldn't void the map."""
    b = Builder()
    errors = []
    side = {"name": doc.get("name", "forged"), "size": float(doc.get("size", 60.0)),
            "spawns": [], "obstacles": []}
    S = side["size"]
    WH = float(doc.get("wall_height", 8.0)); WT = 1.0
    # implicit base: floor + perimeter walls (every arena has them)
    b.quad("floor", (-S, 0, -S), (-S, 0, S), (S, 0, S), (S, 0, -S), (0, 1, 0), 0.125)
    b.box("wall", 0, WH / 2, S + WT, S + 2 * WT, WH / 2, WT)
    b.box("wall", 0, WH / 2, -S - WT, S + 2 * WT, WH / 2, WT)
    b.box("wall", S + WT, WH / 2, 0, WT, WH / 2, S)
    b.box("wall", -S - WT, WH / 2, 0, WT, WH / 2, S)

    def obst(x, z, hw, hd, h):
        side["obstacles"].append([round(x, 3), round(z, 3),
                                  round(hw, 3), round(hd, 3), round(h, 3)])

    for i, op in enumerate(doc.get("ops", [])):
        try:
            kind = op.get("op")
            mat = op.get("mat", "rock")
            if mat not in ("floor", "wall", "rock"):
                raise ValueError(f"unknown mat '{mat}'")
            if kind in ("pillar", "box", "platform"):
                x, z = float(op["x"]), float(op["z"])
                w = float(op.get("w", 1.6)); d = float(op.get("d", w))
                h = float(op.get("h", 2.0 if kind == "platform" else 4.0))
                b.box(mat, x, h / 2, z, w, h / 2, d)
                obst(x, z, w, d, h)
            elif kind == "ramp":
                facing = op.get("facing", "+z")
                if facing not in FACINGS:
                    raise ValueError(f"facing must be one of {FACINGS}")
                b.wedge(mat, float(op["x"]), float(op["z"]),
                        float(op.get("w", 3.0)), float(op.get("run", 3.5)),
                        float(op.get("h", 2.0)), facing)
            elif kind == "scatter_pillars":
                rng = random.Random(int(op.get("seed", 7)))
                for _ in range(int(op.get("count", 8))):
                    a = rng.uniform(0, 2 * math.pi)
                    r = rng.uniform(float(op.get("r_min", 20)), float(op.get("r_max", S * 0.8)))
                    px, pz = r * math.cos(a), r * math.sin(a)
                    h = rng.uniform(float(op.get("h_min", 3.0)), float(op.get("h_max", 7.0)))
                    hw = rng.uniform(1.2, 2.4); hd = rng.uniform(1.2, 2.4)
                    b.box(mat, px, h / 2, pz, hw, h / 2, hd)
                    obst(px, pz, hw, hd, h)
            elif kind == "spawn":
                side["spawns"].append([float(op["x"]), float(op["z"])])
            else:
                raise ValueError(f"unknown op '{kind}'")
        except Exception as ex:
            errors.append(f"op[{i}] {op.get('op')}: {ex}")
    return b, side, errors


def export(doc_name, side):
    """Round-trip companion: rebuild a BuildDoc from a sidecar (obstacles become
    box ops, spawns become spawn ops) — read the current arena, propose a
    change, hand back a full script rather than an ad-hoc patch."""
    doc = default_doc(doc_name)
    doc["size"] = side.get("size", 60.0)
    for (x, z, hw, hd, h) in side.get("obstacles", []):
        doc["ops"].append({"op": "box", "x": x, "z": z, "w": hw, "d": hd, "h": h})
    for (x, z) in side.get("spawns", []):
        doc["ops"].append({"op": "spawn", "x": x, "z": z})
    return doc


# ── IQM writer (identical layout to gen_arena.py's, double-sided) ────────────

def write_iqm(b, doc, out_dir=DATA_DIR):
    tex = doc.get("textures", {})
    groups = [(tex.get("floor", "floor.png"), b.buckets["floor"]),
              (tex.get("wall", "wall.png"), b.buckets["wall"]),
              (tex.get("rock", "rock.png"), b.buckets["rock"])]
    new_verts, new_tris, mesh_recs = [], [], []
    for mat, bucket in groups:
        fv, ft = len(new_verts), len(new_tris)
        for (v0, nv, t0, nt) in bucket:
            remap = {}
            for i in range(nv):
                remap[v0 + i] = len(new_verts); new_verts.append(b.verts[v0 + i])
            for t in range(nt):
                a, bb, c = b.tris[t0 + t]
                new_tris.append((remap[a], remap[bb], remap[c]))
                new_tris.append((remap[a], remap[c], remap[bb]))   # back face
        mesh_recs.append((mat, fv, len(new_verts) - fv, ft, len(new_tris) - ft))
    verts, tris = new_verts, new_tris

    text = b"\x00"; offs = {}
    for mat, *_ in mesh_recs:
        if mat not in offs:
            offs[mat] = len(text); text += mat.encode() + b"\x00"
    name = doc.get("name", "forged")
    name_off = len(text); text += name.encode() + b"\x00"

    def u(*a): return struct.pack("<" + "I" * len(a), *a)
    num_v, num_t, num_m = len(verts), len(tris), len(mesh_recs)
    ofs = 16 + 4 + 4 * 26
    ofs_text = ofs; ofs += len(text)
    pad = (-ofs) % 4; ofs += pad
    ofs_meshes = ofs; ofs += num_m * 6 * 4
    ofs_va = ofs; ofs += 3 * 5 * 4
    ofs_pos = ofs; ofs += num_v * 12
    ofs_uv = ofs; ofs += num_v * 8
    ofs_nrm = ofs; ofs += num_v * 12
    ofs_tri = ofs; ofs += num_t * 12
    filesize = ofs

    out = b"INTERQUAKEMODEL\x00" + u(2, filesize, 0)
    out += u(len(text), ofs_text)
    out += u(num_m, ofs_meshes)
    out += u(3, num_v, ofs_va)
    out += u(num_t, ofs_tri, 0)
    out += u(0, 0) + u(0, 0) + u(0, 0)
    out += u(0, 0, 0, 0)
    out += u(0, 0) + u(0, 0)
    out += text + b"\x00" * pad
    for mat, fv, nv, ft, nt in mesh_recs:
        out += u(name_off, offs[mat], fv, nv, ft, nt)
    out += u(0, 0, 7, 3, ofs_pos)
    out += u(1, 0, 7, 2, ofs_uv)
    out += u(2, 0, 7, 3, ofs_nrm)
    for p, _, _ in verts: out += struct.pack("<3f", *p)
    for _, uv, _ in verts: out += struct.pack("<2f", *uv)
    for _, _, n in verts: out += struct.pack("<3f", *n)
    for t in tris: out += u(*t)
    assert len(out) == filesize, (len(out), filesize)

    os.makedirs(out_dir, exist_ok=True)
    dst = os.path.join(out_dir, name + ".iqm")
    with open(dst, "wb") as f:
        f.write(out)
    return dst, filesize, num_v, num_t, num_m


def build(doc, out_dir=DATA_DIR, quiet=False):
    b, side, errors = apply(doc)
    for e in errors:
        print(f"  ⚠ {e}")
    dst, size, nv, nt, nm = write_iqm(b, doc, out_dir)
    sidecar = os.path.join(out_dir, doc.get("name", "forged") + ".arena.json")
    with open(sidecar, "w") as f:
        json.dump(side, f, indent=1)
    if not quiet:
        print(f"wrote {dst}: {nv} verts, {nt} tris, {nm} meshes, {size} bytes")
        print(f"wrote {sidecar}: {len(side['obstacles'])} obstacles, "
              f"{len(side['spawns'])} spawns")
    return dst, sidecar, errors


def example_doc():
    d = default_doc("forged")
    d["ops"] = [
        {"op": "platform", "x": 0, "z": 0, "w": 8, "d": 8, "h": 2},
        {"op": "ramp", "x": 0, "z": 11.5, "w": 3, "run": 3.5, "h": 2, "facing": "+z"},
        {"op": "ramp", "x": 0, "z": -11.5, "w": 3, "run": 3.5, "h": 2, "facing": "-z"},
        {"op": "ramp", "x": 11.5, "z": 0, "w": 3, "run": 3.5, "h": 2, "facing": "+x"},
        {"op": "ramp", "x": -11.5, "z": 0, "w": 3, "run": 3.5, "h": 2, "facing": "-x"},
        {"op": "scatter_pillars", "count": 10, "seed": 7},
        {"op": "box", "x": -30, "z": 24, "w": 4, "d": 1.2, "h": 3, "mat": "wall"},
        {"op": "box", "x": 30, "z": -24, "w": 4, "d": 1.2, "h": 3, "mat": "wall"},
        {"op": "spawn", "x": -48, "z": -48},
        {"op": "spawn", "x": 48, "z": 48},
    ]
    return d


# ── GUI (top-down, pygame) ───────────────────────────────────────────────────

BRUSHES = ["pillar", "box", "platform", "ramp", "spawn"]

def run_editor(path=None):
    import pygame
    pygame.init()
    W, H = 1100, 820
    screen = pygame.display.set_mode((W, H), pygame.RESIZABLE)
    clock = pygame.time.Clock()
    font = pygame.font.Font(None, 20)

    if path and os.path.exists(path):
        doc = json.load(open(path))
    else:
        doc = example_doc()
        path = path or os.path.join(HERE, "forged.doc.json")
    brush = 0
    sel = None                 # index into doc["ops"], geometry ops only
    drag = None
    zoom = 5.0
    status, status_t = "", 0.0

    def flash(s):
        nonlocal status, status_t
        status, status_t = s, 2.5

    def w2s(x, z):
        return int(W / 2 + x * zoom), int(H / 2 + z * zoom)

    def s2w(px, py):
        return (px - W / 2) / zoom, (py - H / 2) / zoom

    def hit(pos):
        wx, wz = s2w(*pos)
        for i in range(len(doc["ops"]) - 1, -1, -1):
            op = doc["ops"][i]
            k = op.get("op")
            if k in ("pillar", "box", "platform"):
                if abs(wx - op["x"]) <= op.get("w", 1.6) and \
                   abs(wz - op["z"]) <= op.get("d", op.get("w", 1.6)):
                    return i
            elif k == "ramp":
                if abs(wx - op["x"]) <= op.get("w", 3.0) + 1 and \
                   abs(wz - op["z"]) <= op.get("run", 3.5) + 1:
                    return i
            elif k == "spawn":
                if math.hypot(wx - op["x"], wz - op["z"]) < 2.5:
                    return i
        return None

    running = True
    while running:
        dt = clock.tick(60) / 1000.0
        status_t = max(0, status_t - dt)
        for e in pygame.event.get():
            if e.type == pygame.QUIT:
                running = False
            elif e.type == pygame.VIDEORESIZE:
                W, H = e.w, e.h
            elif e.type == pygame.MOUSEWHEEL:
                zoom = max(2.0, min(14.0, zoom * (1.1 if e.y > 0 else 0.9)))
            elif e.type == pygame.KEYDOWN:
                mods = pygame.key.get_mods()
                if e.key == pygame.K_ESCAPE:
                    running = False
                elif pygame.K_1 <= e.key <= pygame.K_5:
                    brush = e.key - pygame.K_1
                    flash(f"Brush: {BRUSHES[brush]}")
                elif e.key == pygame.K_s and (mods & pygame.KMOD_CTRL):
                    json.dump(doc, open(path, "w"), indent=1)
                    flash(f"Saved doc → {path}")
                elif e.key == pygame.K_e:
                    dst, sc, errs = build(doc, quiet=True)
                    flash(f"Exported {os.path.basename(dst)}"
                          + (f" ({len(errs)} op errors)" if errs else ""))
                elif e.key == pygame.K_DELETE and sel is not None:
                    doc["ops"].pop(sel); sel = None
                elif e.key == pygame.K_r and sel is not None \
                        and doc["ops"][sel].get("op") == "ramp":
                    f0 = doc["ops"][sel].get("facing", "+z")
                    doc["ops"][sel]["facing"] = FACINGS[(FACINGS.index(f0) + 1) % 4]
                elif e.key in (pygame.K_LEFTBRACKET, pygame.K_RIGHTBRACKET) \
                        and sel is not None:
                    g = 1.15 if e.key == pygame.K_RIGHTBRACKET else 1 / 1.15
                    op = doc["ops"][sel]
                    for f in ("w", "d", "h", "run"):
                        if f in op: op[f] = round(op[f] * g, 2)
            elif e.type == pygame.MOUSEBUTTONDOWN and e.button == 1:
                i = hit(e.pos)
                if i is not None:
                    sel = i; drag = (e.pos, doc["ops"][i].get("x", 0), doc["ops"][i].get("z", 0))
                else:
                    wx, wz = s2w(*e.pos)
                    k = BRUSHES[brush]
                    op = {"op": k, "x": round(wx, 2), "z": round(wz, 2)}
                    if k == "pillar":   op.update(w=1.8, d=1.8, h=5.0)
                    elif k == "box":    op.update(w=3.0, d=2.0, h=1.5)
                    elif k == "platform": op.update(w=6.0, d=6.0, h=2.0)
                    elif k == "ramp":   op.update(w=3.0, run=3.5, h=2.0, facing="+z")
                    doc["ops"].append(op); sel = len(doc["ops"]) - 1
            elif e.type == pygame.MOUSEBUTTONUP and e.button == 1:
                drag = None
            elif e.type == pygame.MOUSEMOTION and drag and sel is not None:
                (px, py), x0, z0 = drag
                doc["ops"][sel]["x"] = round(x0 + (e.pos[0] - px) / zoom, 2)
                doc["ops"][sel]["z"] = round(z0 + (e.pos[1] - py) / zoom, 2)

        # draw
        screen.fill((16, 18, 24))
        S = doc.get("size", 60.0)
        x0, y0 = w2s(-S, -S); x1, y1 = w2s(S, S)
        pygame.draw.rect(screen, (30, 34, 44), (x0, y0, x1 - x0, y1 - y0))
        pygame.draw.rect(screen, (120, 140, 190), (x0, y0, x1 - x0, y1 - y0), 3)
        for i, op in enumerate(doc["ops"]):
            k = op.get("op"); selc = (i == sel)
            if k in ("pillar", "box", "platform"):
                w = op.get("w", 1.6); d = op.get("d", w)
                ax, ay = w2s(op["x"] - w, op["z"] - d)
                bx, by = w2s(op["x"] + w, op["z"] + d)
                col = {"pillar": (150, 130, 110), "box": (120, 140, 120),
                       "platform": (110, 120, 160)}[k]
                pygame.draw.rect(screen, col, (ax, ay, bx - ax, by - ay))
                pygame.draw.rect(screen, (250, 220, 120) if selc else (60, 66, 82),
                                 (ax, ay, bx - ax, by - ay), 2)
            elif k == "ramp":
                w = op.get("w", 3.0); run = op.get("run", 3.5)
                cx, cz = op["x"], op["z"]; f = op.get("facing", "+z")
                dx, dz = {"+z": (0, 1), "-z": (0, -1), "+x": (1, 0), "-x": (-1, 0)}[f]
                pts = [w2s(cx - w * dz - 0, cz + w * dx), w2s(cx + w * dz, cz - w * dx),
                       w2s(cx + w * dz + dx * run, cz - w * dx + dz * run),
                       w2s(cx - w * dz + dx * run, cz + w * dx + dz * run)]
                pygame.draw.polygon(screen, (160, 120, 90), pts)
                pygame.draw.polygon(screen, (250, 220, 120) if selc else (70, 60, 50), pts, 2)
            elif k == "spawn":
                px, py = w2s(op["x"], op["z"])
                pygame.draw.circle(screen, (90, 200, 130), (px, py), 8)
                pygame.draw.circle(screen, (250, 220, 120) if selc else (30, 60, 40),
                                   (px, py), 8, 2)
            elif k == "scatter_pillars":
                pass   # procedural — shown only after export
        hud = (f"brush [{'/'.join(str(n+1) for n in range(5))}]: {BRUSHES[brush]}   ·   "
               "click place · drag move · R rotate ramp · [ ] resize · Del delete · "
               "Ctrl+S save · E export IQM")
        screen.blit(font.render(hud, True, (150, 158, 178)), (14, H - 26))
        if status_t > 0:
            screen.blit(font.render(status, True, (240, 220, 150)), (14, 12))
        pygame.display.flip()
    pygame.quit()


# ── selftest ─────────────────────────────────────────────────────────────────

def _selftest():
    import tempfile
    doc = example_doc()
    with tempfile.TemporaryDirectory() as td:
        dst, sidecar, errors = build(doc, out_dir=td, quiet=True)
        assert not errors, errors
        raw = open(dst, "rb").read()
        assert raw[:16] == b"INTERQUAKEMODEL\x00", "bad IQM magic"
        ver, size = struct.unpack_from("<II", raw, 16)
        assert ver == 2 and size == len(raw), "IQM header size mismatch"
        side = json.load(open(sidecar))
        assert len(side["spawns"]) == 2
        assert len(side["obstacles"]) >= 11, "platform + pillars + 2 cover boxes expected"
        # bad ops are collected, good ones still land
        bad = default_doc("bad")
        bad["ops"] = [{"op": "nope"}, {"op": "pillar", "x": 0, "z": 0},
                      {"op": "ramp", "x": 1, "z": 1, "facing": "sideways"}]
        _, side2, errs2 = apply(bad)
        assert len(errs2) == 2 and len(side2["obstacles"]) == 1, (errs2, side2)
        # export round-trips the sidecar into a rebuildable doc
        doc2 = export("rt", side)
        _, side3, errs3 = apply(doc2)
        assert not errs3
        assert side3["obstacles"] == side["obstacles"], "export→apply drifted obstacles"
        assert side3["spawns"] == side["spawns"]
        # determinism: same doc → identical bytes
        d2, _, _ = build(doc, out_dir=td, quiet=True)
        assert open(d2, "rb").read() == raw, "build not deterministic"
    print("[arena_forge] selftest OK — IQM header, sidecar, error tolerance, "
          "export round-trip, determinism")


if __name__ == "__main__":
    args = sys.argv[1:]
    if "--selftest" in args:
        _selftest()
    elif args and args[0] == "example":
        print(json.dumps(example_doc(), indent=1))
    elif args and args[0] == "build" and len(args) > 1:
        build(json.load(open(args[1])))
    elif args and args[0] == "edit":
        run_editor(args[1] if len(args) > 2 or len(args) == 2 else None)
    else:
        print(__doc__)
