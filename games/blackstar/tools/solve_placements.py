#!/usr/bin/env python3
"""solve_placements.py -- give every placement the rotation it was missing.

`partlib.json` records a placement as `sig`, `centroid` and `size`. The render
parity harness showed what that costs: with full part coverage the
reconstruction still only reached 0.509 mean silhouette IoU, because a part can
be in the right place at the right scale and turned the wrong way.

The fix is already in this repo's vocabulary. `derive_web.py` recovers a
character's motion by matching components across its two forms and solving each
pair with Kabsch, and states the principle plainly: *"the motion it undergoes is
not something to invent — it is the rigid transform that carries one onto the
other."* The same solve applies here, between an exemplar part and the component
it stands in for.

## Correspondence is the whole difficulty

Kabsch needs point pairs, and two meshes of the same part have no shared vertex
order. Matching by nearest-neighbour would be circular — you need the rotation
to find the neighbours. So correspondence comes from the invariant the harvester
already relies on: **sort both point sets by distance from their own centroid**.
That ordering is rotation- and translation-invariant by construction, so two
copies of the same shape produce the same sequence whatever their pose.

It is approximate for a part with rotational symmetry — a wheel's rim points sit
at equal radii and their relative order is arbitrary — which is why the residual
is written out per placement rather than assumed small. A high residual means
this part should keep its own geometry instead of being rebuilt from an
exemplar, and that is a decision the caller can now make from data.
"""
import sys, os, json, math, argparse

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)),
                                "..", "game-model-tools", "python"))


from transform_collage import weld, components as split_components


def real_components(verts, tris, eps_frac=0.002):
    """The model's actual connected pieces, not a sphere of nearby vertices.

    The first version took "vertices within 0.75 x size of the centroid", which
    is a ball that scoops in whatever else is nearby — a wheel arrives with a
    slice of the fender welded to it, and no rotation fits that. This is the
    same connected-component split the harvester uses, so the thing being posed
    is the thing that was clustered.
    """
    if not verts:
        return []
    lo = [min(v[i] for v in verts) for i in range(3)]
    hi = [max(v[i] for v in verts) for i in range(3)]
    span = max(hi[i] - lo[i] for i in range(3)) or 1.0
    flat = [i for t in tris for i in t]
    remap = weld(verts, span * eps_frac)
    return [[verts[i] for i in g] for g in split_components(len(verts), flat, remap)]


def load_obj(path):
    verts, tris = [], []
    with open(path) as f:
        for line in f:
            if line.startswith("v "):
                verts.append(tuple(float(x) for x in line.split()[1:4]))
            elif line.startswith("f "):
                idx = [int(p.split("/")[0]) - 1 for p in line.split()[1:]]
                for i in range(1, len(idx) - 1):
                    tris.append((idx[0], idx[i], idx[i + 1]))
    return verts, tris


def centroid(pts):
    n = len(pts)
    return [sum(p[i] for p in pts) / n for i in range(3)]


def ordered(pts, k):
    """A canonical, pose-independent ordering: by distance from the centroid.

    Resampled to exactly k points so two meshes of different density can still
    be paired — the exemplar and the component are the same shape but not
    necessarily the same tessellation.
    """
    c = centroid(pts)
    s = sorted(pts, key=lambda p: math.dist(p, c))
    if len(s) == k:
        return s
    return [s[min(len(s) - 1, int(i * len(s) / k))] for i in range(k)]


def kabsch(A, B):
    """Rotation carrying A onto B, both already centred. Returns a 3x3 matrix.

    Jacobi eigen-decomposition of the covariance rather than a full SVD, to
    stay dependency-free like the rest of these tools.
    """
    n = len(A)
    ca, cb = centroid(A), centroid(B)
    H = [[0.0] * 3 for _ in range(3)]
    for a, b in zip(A, B):
        for i in range(3):
            for j in range(3):
                H[i][j] += (a[i] - ca[i]) * (b[j] - cb[j])
    # Orthogonalise H by iterated averaging with its inverse-transpose, which
    # converges to the rotation factor of the polar decomposition.
    R = [row[:] for row in H]
    for _ in range(64):
        det = (R[0][0] * (R[1][1] * R[2][2] - R[1][2] * R[2][1])
               - R[0][1] * (R[1][0] * R[2][2] - R[1][2] * R[2][0])
               + R[0][2] * (R[1][0] * R[2][1] - R[1][1] * R[2][0]))
        if abs(det) < 1e-14:
            return None
        inv = [[0.0] * 3 for _ in range(3)]
        for i in range(3):
            for j in range(3):
                a1, a2 = (i + 1) % 3, (i + 2) % 3
                b1, b2 = (j + 1) % 3, (j + 2) % 3
                inv[j][i] = (R[a1][b1] * R[a2][b2] - R[a1][b2] * R[a2][b1]) / det
        nxt = [[0.5 * (R[i][j] + inv[j][i]) for j in range(3)] for i in range(3)]
        delta = sum(abs(nxt[i][j] - R[i][j]) for i in range(3) for j in range(3))
        R = nxt
        if delta < 1e-12:
            break
    return R


def apply_r(R, p):
    return [sum(R[i][j] * p[j] for j in range(3)) for i in range(3)]


def rmsd(A, B, R):
    ca, cb = centroid(A), centroid(B)
    tot = 0.0
    for a, b in zip(A, B):
        ra = apply_r(R, [a[i] - ca[i] for i in range(3)])
        tot += sum((ra[i] - (b[i] - cb[i])) ** 2 for i in range(3))
    return math.sqrt(tot / len(A))


def pca_axes(pts):
    """Principal axes of a point set, as three orthonormal rows.

    This is the second attempt. The first paired points by distance from the
    centroid and fed them to Kabsch, and it made the reconstruction WORSE
    (0.509 -> 0.495 mean IoU) because that ordering is not a correspondence:
    two copies of a part sorted by radius interleave differently, so Kabsch
    fitted noise and returned confident nonsense.

    Principal axes avoid the problem instead of solving it. The covariance's
    eigenvectors are a property of the SHAPE, not of any vertex pairing, so two
    copies of the same part yield the same frame however they are tessellated.
    """
    c = centroid(pts)
    C = [[0.0] * 3 for _ in range(3)]
    for p in pts:
        d = [p[i] - c[i] for i in range(3)]
        for i in range(3):
            for j in range(3):
                C[i][j] += d[i] * d[j]
    # Jacobi rotation for a symmetric 3x3 — dependency-free like the rest.
    V = [[1.0 if i == j else 0.0 for j in range(3)] for i in range(3)]
    A = [row[:] for row in C]
    for _ in range(64):
        p_, q_ = 0, 1
        best = abs(A[0][1])
        for i in range(3):
            for j in range(i + 1, 3):
                if abs(A[i][j]) > best:
                    best, p_, q_ = abs(A[i][j]), i, j
        if best < 1e-14:
            break
        th = 0.5 * math.atan2(2 * A[p_][q_], A[q_][q_] - A[p_][p_])
        cth, sth = math.cos(th), math.sin(th)
        for k in range(3):
            akp = cth * A[k][p_] - sth * A[k][q_]
            akq = sth * A[k][p_] + cth * A[k][q_]
            A[k][p_], A[k][q_] = akp, akq
        for k in range(3):
            apk = cth * A[p_][k] - sth * A[q_][k]
            aqk = sth * A[p_][k] + cth * A[q_][k]
            A[p_][k], A[q_][k] = apk, aqk
        for k in range(3):
            vkp = cth * V[k][p_] - sth * V[k][q_]
            vkq = sth * V[k][p_] + cth * V[k][q_]
            V[k][p_], V[k][q_] = vkp, vkq
    order = sorted(range(3), key=lambda i: -A[i][i])
    return [[V[r][i] for r in range(3)] for i in order]


def align_pca(ex, comp):
    """Rotation carrying the exemplar's principal frame onto the component's.

    An eigenvector's SIGN is arbitrary, so four proper-rotation sign
    combinations must be tried and the best kept — skipping that is how a part
    ends up mirrored or upside down while the axes technically agree.
    """
    Ea, Ca = pca_axes(ex), pca_axes(comp)
    ce, cc = centroid(ex), centroid(comp)
    exd = [[p[i] - ce[i] for i in range(3)] for p in ex]
    best, bestR = None, None
    for sx in (1, -1):
        for sy in (1, -1):
            sz = sx * sy  # keeps det(+1): a mirror is not a rotation
            S = [sx, sy, sz]
            E = [[Ea[r][i] * S[r] for i in range(3)] for r in range(3)]
            R = [[sum(Ca[k][i] * E[k][j] for k in range(3)) for j in range(3)]
                 for i in range(3)]
            # Score by how well the exemplar's extent matches the component's
            # once rotated -- cheap, and it does not need correspondence.
            rot = [apply_r(R, d) for d in exd]
            ext_r = [max(p[i] for p in rot) - min(p[i] for p in rot) for i in range(3)]
            ext_c = [max(p[i] for p in comp) - min(p[i] for p in comp) for i in range(3)]
            sc = max(1e-9, max(ext_c))
            err = sum(abs(ext_r[i] - ext_c[i]) for i in range(3)) / sc
            if best is None or err < best:
                best, bestR = err, R
    return bestR, best


def shape_modifier(ex, comp, R, size, bins=8):
    """A residual stored as a VECTOR FIELD over the part's volume, not a flat
    scalar.

    `residual_max` says only "this exemplar fits badly". That is enough to
    decide whether to keep the original geometry and nothing else. A modifier
    says WHERE and IN WHICH DIRECTION it differs, which is the difference
    between discarding a part and correcting it.

    The field is volumetric on purpose: the part is divided into an 8x8x8 grid
    of its own bounding box, and each occupied cell stores the mean
    displacement from the exemplar's surface to the component's. A per-vertex
    delta would be flat in the sense that matters — it would be tied to one
    tessellation, so it could not be applied to an exemplar with a different
    vertex count, which is exactly the case here. A field over the volume is
    tied to SPACE, so it transfers to any mesh occupying that space.

    Cost: at most 8^3 cells x 3 int8 = 1.5KB before compression, and in practice
    far less because only occupied cells are stored. Against a part that costs
    several KB of geometry, correcting is cheaper than storing a second copy.
    """
    ce, cc = centroid(ex), centroid(comp)
    ext = max(max(p[i] for p in ex) - min(p[i] for p in ex) for i in range(3)) or 1.0
    s = (size or ext) / ext
    posed = [[sum(R[i][j] * (p[j] - ce[j]) for j in range(3)) * s + cc[i]
              for i in range(3)] for p in ex]

    lo = [min(p[i] for p in posed + comp) for i in range(3)]
    hi = [max(p[i] for p in posed + comp) for i in range(3)]
    span = [max(hi[i] - lo[i], 1e-9) for i in range(3)]

    def cell_of(p):
        return tuple(min(bins - 1, max(0, int((p[i] - lo[i]) / span[i] * bins)))
                     for i in range(3))

    # Where the component's surface sits in each cell, against where the posed
    # exemplar's does. Mean-to-mean rather than nearest-point, because nearest
    # -point needs a correspondence and that is the trap this whole exercise
    # already fell into once.
    ca, cb = {}, {}
    for p in posed:
        ca.setdefault(cell_of(p), []).append(p)
    for p in comp:
        cb.setdefault(cell_of(p), []).append(p)

    mod, scale = {}, max(span)
    for k in cb:
        if k not in ca:
            continue
        ma = centroid(ca[k])
        mb = centroid(cb[k])
        d = [(mb[i] - ma[i]) / scale for i in range(3)]
        if max(abs(x) for x in d) < 0.01:
            continue    # below a hundredth of the part: not worth a byte
        mod["%d,%d,%d" % k] = [max(-127, min(127, int(round(x * 127)))) for x in d]
    return mod, len(cb), len(ca)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--lib", required=True)
    ap.add_argument("--meshes", required=True, help="dir of exemplar .obj")
    ap.add_argument("--originals", required=True, help="dir of <model>/<model>.obj")
    ap.add_argument("--out", required=True)
    ap.add_argument("--samples", type=int, default=64)
    ap.add_argument("--limit", type=int, default=0)
    ap.add_argument("--modifiers", action="store_true",
                    help="store a volumetric residual field per placement")
    a = ap.parse_args()

    lib = json.load(open(a.lib))
    # Exemplar geometry by signature, so a placement can find what it stands in
    # for.
    by_sig = {}
    for p in lib.get("parts", []):
        if p.get("mesh") and p.get("sig") and p["sig"] not in by_sig:
            mp = os.path.join(a.meshes, p["mesh"] + ".obj")
            if os.path.exists(mp):
                by_sig[p["sig"]] = load_obj(mp)[0]

    solved = unsolved = modcells = 0
    residuals = []
    items = list(lib.get("placements", {}).items())
    if a.limit:
        items = items[:a.limit]
    for name, pls in items:
        src = os.path.join(a.originals, name, name + ".obj")
        if not os.path.exists(src):
            continue
        mv, mt = load_obj(src)
        if not mv:
            continue
        comps = real_components(mv, mt)
        if not comps:
            continue
        for pl in pls:
            ex = by_sig.get(pl["sig"])
            if not ex:
                unsolved += 1
                continue
            # The real component nearest this placement's centroid. Matching by
            # centroid rather than by index because the placement records where
            # the piece IS, and the split does not preserve harvest order.
            comp = None
            best = None
            for g in comps:
                d = math.dist(centroid(g), pl["centroid"])
                if best is None or d < best:
                    best, comp = d, g
            if comp is None or len(comp) < 8:
                unsolved += 1
                continue
            R, err = align_pca(ex, comp)
            if R is None:
                unsolved += 1
                continue
            pl["rot"] = [[round(R[i][j], 6) for j in range(3)] for i in range(3)]
            pl["rot_residual"] = round(err, 5)
            residuals.append(err)
            if a.modifiers:
                mod, nb, na = shape_modifier(ex, comp, R, pl.get("size"))
                if mod:
                    pl["mod"] = mod
                    modcells += len(mod)
            solved += 1

    json.dump(lib, open(a.out, "w"), separators=(",", ":"))
    residuals.sort()
    print("placements solved : %d" % solved)
    print("unsolved          : %d" % unsolved)
    if residuals:
        mid = residuals[len(residuals) // 2]
        p90 = residuals[int(len(residuals) * 0.9)]
        print("residual / size   : median %.3f   p90 %.3f" % (mid, p90))
    if a.modifiers:
        print("modifier cells    : %d  (~%.1f KB raw)" % (modcells, modcells * 3 / 1024))
    print("wrote %s" % a.out)
    return 0


if __name__ == "__main__":
    sys.exit(main())
