#!/usr/bin/env python3
"""glb.py — write forge models as GLB with NAMED, TRANSFORMED nodes.

## Why named nodes are the point

`blackstar-rs/src/model.rs` reads a `.glb` and then spends most of its 1500
lines guessing at things a generated model already knows:

    find_orientation / find_upright   which way is up, tried per axis
    wheel_score / the wheel split     which lumps are wheels, by clustering
    partdetect                        what each prim is, one prim at a time
    skeletonize                       what it is attached to

All of that exists for one stated reason, in the loader's own words:

    "Named nodes solve it when they exist; these do not have them, so find
     the wheels geometrically instead."

A forge model HAS them. It placed every part; it knows the axle position, the
hip joint, which way the bot stands. Emitting that is a bigger win than the
compression was — it skips an entire inference layer whose failure modes are
already in the record: round greebles detected as wheels, models arriving
rolled or nose-aft, whole models returned as a single "part".

## What this writes

One glTF node per placement:

  * `name`  — the role, with wheels in `detect_tag`'s existing `-FL/-FR/
    -BL/-BR` convention so they are tagged without touching `model.rs`.
    Everything else carries a plain role name (`torso`, `thigh_l`) which
    `detect_tag` returns None for, so it falls through to `inherited` —
    the current behaviour, unchanged.
  * `translation` / `rotation` / `scale` — the node transform. `collect_node`
    multiplies this into `world` and bakes it into the vertices, so the
    geometry can stay in part-local space where the hinge is at the origin.
    That is what makes a pivot meaningful rather than inferred.

Geometry is written once per distinct part mesh and referenced by every node
that uses it, which is the same store-once-reference-many the parts web does.
"""
import base64, json, struct
from collections import OrderedDict


# glTF component/type constants
F32, U32 = 5126, 5125
ARRAY_BUFFER, ELEMENT_ARRAY_BUFFER = 34962, 34963


class Glb:
    def __init__(self):
        self.bin = bytearray()
        self.buffer_views = []
        self.accessors = []
        self.meshes = []
        self.nodes = []
        self._mesh_cache = {}

    # ---------------------------------------------------------------- bin

    def _view(self, data: bytes, target=None):
        while len(self.bin) % 4:
            self.bin.append(0)
        off = len(self.bin)
        self.bin += data
        v = {"buffer": 0, "byteOffset": off, "byteLength": len(data)}
        if target:
            v["target"] = target
        self.buffer_views.append(v)
        return len(self.buffer_views) - 1

    def _acc_vec3(self, verts):
        data = b"".join(struct.pack("<3f", *v) for v in verts)
        vi = self._view(data, ARRAY_BUFFER)
        xs, ys, zs = zip(*verts) if verts else ((0,), (0,), (0,))
        self.accessors.append({
            "bufferView": vi, "componentType": F32, "count": len(verts),
            "type": "VEC3",
            # min/max on POSITION is REQUIRED by the spec; a viewer that
            # computes bounds from it gets a wrong frustum without it
            "min": [min(xs), min(ys), min(zs)],
            "max": [max(xs), max(ys), max(zs)],
        })
        return len(self.accessors) - 1

    def _acc_idx(self, idx):
        data = b"".join(struct.pack("<I", i) for i in idx)
        vi = self._view(data, ELEMENT_ARRAY_BUFFER)
        self.accessors.append({"bufferView": vi, "componentType": U32,
                               "count": len(idx), "type": "SCALAR"})
        return len(self.accessors) - 1

    # -------------------------------------------------------------- build

    def add_mesh(self, key, mesh, name=None):
        """Register a part mesh once; later nodes reference it by index."""
        if key in self._mesh_cache:
            return self._mesh_cache[key]
        pos = self._acc_vec3(mesh.v)
        nrm = self._acc_vec3(_normals(mesh))
        ind = self._acc_idx([i for tri in mesh.f for i in tri])
        self.meshes.append({
            "name": name or key,
            "primitives": [{"attributes": {"POSITION": pos, "NORMAL": nrm},
                            "indices": ind}],
        })
        mi = len(self.meshes) - 1
        self._mesh_cache[key] = mi
        return mi

    def add_node(self, name, mesh_index, translation=(0, 0, 0),
                 rotation=(0, 0, 0, 1), scale=(1, 1, 1)):
        n = {"name": name, "mesh": mesh_index}
        if tuple(translation) != (0, 0, 0):
            n["translation"] = [float(x) for x in translation]
        if tuple(rotation) != (0, 0, 0, 1):
            n["rotation"] = [float(x) for x in rotation]
        if tuple(scale) != (1, 1, 1):
            n["scale"] = [float(x) for x in scale]
        self.nodes.append(n)
        return len(self.nodes) - 1

    def write(self, path, asset_name="forge"):
        gltf = {
            "asset": {"version": "2.0", "generator": f"forge/{asset_name}"},
            "scene": 0,
            "scenes": [{"nodes": list(range(len(self.nodes)))}],
            "nodes": self.nodes,
            "meshes": self.meshes,
            "accessors": self.accessors,
            "bufferViews": self.buffer_views,
            "buffers": [{"byteLength": len(self.bin)}],
        }
        js = json.dumps(gltf, separators=(",", ":")).encode()
        while len(js) % 4:
            js += b" "
        bn = bytes(self.bin)
        while len(bn) % 4:
            bn += b"\0"
        total = 12 + 8 + len(js) + 8 + len(bn)
        with open(path, "wb") as f:
            f.write(struct.pack("<III", 0x46546C67, 2, total))     # "glTF", v2
            f.write(struct.pack("<II", len(js), 0x4E4F534A))        # "JSON"
            f.write(js)
            f.write(struct.pack("<II", len(bn), 0x004E4942))        # "BIN"
            f.write(bn)
        return total


def _normals(mesh):
    """Area-weighted vertex normals. The loader synthesises these when
    absent, but doing it here means the two agree."""
    acc = [[0.0, 0.0, 0.0] for _ in mesh.v]
    for (a, b, c) in mesh.f:
        pa, pb, pc = mesh.v[a], mesh.v[b], mesh.v[c]
        u = (pb[0]-pa[0], pb[1]-pa[1], pb[2]-pa[2])
        w = (pc[0]-pa[0], pc[1]-pa[1], pc[2]-pa[2])
        n = (u[1]*w[2]-u[2]*w[1], u[2]*w[0]-u[0]*w[2], u[0]*w[1]-u[1]*w[0])
        for i in (a, b, c):
            acc[i][0] += n[0]; acc[i][1] += n[1]; acc[i][2] += n[2]
    out = []
    for n in acc:
        L = (n[0]*n[0] + n[1]*n[1] + n[2]*n[2]) ** 0.5 or 1.0
        out.append((n[0]/L, n[1]/L, n[2]/L))
    return out


# ------------------------------------------------------------ role naming

def wheel_name(side, front):
    """`detect_tag` in model.rs matches "- FR"/"-FL"/"- BR"/"-BL" and nothing
    else, so wheels are named in that convention. Every other role falls
    through to None, i.e. the existing inherited-tag behaviour — this file
    adds names without changing how untagged parts are handled."""
    return f"wheel-{'F' if front else 'B'}{'L' if side < 0 else 'R'}"
