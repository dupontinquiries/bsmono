#!/usr/bin/env python3
"""fleet_index.py — one manifest for every chassis in assets/bots.

The previous fleet.json described only the 173 this session exported, which
left the 48 older chassis unlabelled and said nothing about which of them the
engine will actually morph. This walks the whole directory instead.

`morph` is the shape contract xform::Morph::build enforces: identical prim
count and identical per-prim vertex counts across the strip. It is read from
the glTF accessors, which is what makes it checkable for packed chassis whose
scene.bin no longer exists. Spot-checked against the real Morph compiled from
src/xform.rs — the two agree on every chassis where the geometry was still
present to run it.

`origin` says where a chassis came from, because provenance is what tells a
later session whether a surprise is a regression or just an untouched corner:

  legacy    already in the tree before this session; the exporter that made
            it merged colours per pose, so most refuse the morph
  fleet     the 173-vehicle export
  added     newer categories — walkers, helicopters, submarines, an airship

`duplicate_of` marks the 26 pairs where the same source model appears twice
under two names, once short (referenced by existing code) and once long.
"""
import json
import os
import sys
from collections import Counter, defaultdict

ROOT = sys.argv[1]


def shape(cdir, prefix):
    """Per-pose prim vertex counts, from the glTF alone."""
    poses = sorted(d for d in os.listdir(cdir)
                   if d.startswith(prefix) and d[1:].isdigit()
                   and os.path.isfile(os.path.join(cdir, d, "scene.gltf")))
    out = []
    for p in poses:
        g = json.load(open(os.path.join(cdir, p, "scene.gltf")))
        out.append((
            [g["accessors"][m["primitives"][0]["attributes"]["POSITION"]]["count"]
             for m in g["meshes"]],
            sum(g["accessors"][m["primitives"][0]["indices"]]["count"] for m in g["meshes"]) // 3,
        ))
    return poses, out


def morph_of(shapes):
    if len(shapes) < 2:
        return "no strip", None
    first = shapes[0][0]
    for k, (s, _) in enumerate(shapes[1:], 1):
        if len(s) != len(first):
            return "refused", f"pose {k} has {len(s)} prims, pose 0 has {len(first)}"
        for i, (a, b) in enumerate(zip(s, first)):
            if a != b:
                return "refused", f"pose {k} prim {i} has {a} vertices, pose 0 has {b}"
    return "accepted", None


def main():
    # The fleet list comes from the export manifest, NOT from the fleet.json
    # this script writes — reading its own output back makes every chassis
    # look like it came from the fleet the second time it runs.
    prev = {}
    src_manifest = os.environ.get("FLEET_MANIFEST", "")
    if src_manifest and os.path.isfile(src_manifest):
        for f in json.load(open(src_manifest)).get("fleet", []):
            prev[f["name"]] = f
    # Extra sweep reports (name -> metrics), for chassis the export manifest
    # does not cover. Same columns as qa.py writes.
    extra = {}
    for path in os.environ.get("FLEET_REPORTS", "").split(os.pathsep):
        if not path or not os.path.isfile(path):
            continue
        for ln in open(path):
            c = ln.split()
            if len(c) >= 6 and c[0].endswith(".iqm"):
                try:
                    extra[c[0]] = dict(blobs_at_vehicle=int(c[1]), blobs_mid_burst=int(c[2]),
                                       blobs_at_robot_worst_view=int(c[3]),
                                       symmetry=float(c[4]), grounded=float(c[5]))
                except ValueError:
                    pass

    added = {}
    if len(sys.argv) > 2 and os.path.isfile(sys.argv[2]):
        for f in json.load(open(sys.argv[2])):
            added[f["name"]] = f

    by_source = defaultdict(list)
    recs = []
    for name in sorted(os.listdir(ROOT)):
        cdir = os.path.join(ROOT, name)
        if not os.path.isdir(cdir):
            continue
        cj = os.path.join(cdir, "chassis.json")
        meta = json.load(open(cj)) if os.path.isfile(cj) else {}
        src = meta.get("source", "")
        fold_dirs, fold = shape(cdir, "f")
        walk_dirs, walk = shape(cdir, "w")
        m, why = morph_of(fold)
        wm, wwhy = morph_of(walk) if walk else ("none", None)

        old_rec = added.get(name) or prev.get(name) or {}
        origin = "added" if name in added else ("fleet" if name in prev else "legacy")
        rec = dict(
            name=name,
            origin=origin,
            category=old_rec.get("category", meta.get("foldview", {}).get("category", "unknown")),
            source=src,
            scale=meta.get("scale", 1.0),
            boss=bool(meta.get("boss", False)),
            poses=len(fold_dirs),
            prims=len(fold[0][0]) if fold else 0,
            vertices=sum(fold[0][0]) if fold else 0,
            triangles=fold[0][1] if fold else 0,
            morph=m,
            packed=os.path.isfile(os.path.join(cdir, "bots.pack")),
            walk_poses=len(walk_dirs),
            walk_morph=wm,
            walk_packed=os.path.isfile(os.path.join(cdir, "walk.pack")),
            sweep=old_rec.get("sweep", "none"),
            confidence=old_rec.get("confidence", "unknown"),
        )
        if why:
            rec["morph_detail"] = why
        if wwhy:
            rec["walk_morph_detail"] = wwhy
        if "metrics" in old_rec:
            rec["metrics"] = old_rec["metrics"]
        if "caveat" in old_rec:
            rec["caveat"] = old_rec["caveat"]
        if rec["confidence"] == "unknown" and src in extra:
            q = extra[src]
            rec["metrics"] = q
            rec["sweep"] = "cold"
            if q["blobs_at_robot_worst_view"] > 1:
                rec["confidence"] = "broken"
                rec["caveat"] = (f"{q['blobs_at_robot_worst_view']} silhouette blobs at t=1 — "
                                 "a piece separates and does not come back")
            elif q["blobs_mid_burst"] > 2:
                rec["confidence"] = "low"
                rec["caveat"] = f"mid-fold scatter ({q['blobs_mid_burst']} blobs during the burst)"
            elif q["symmetry"] < 0.70:
                rec["confidence"] = "low"
                rec["caveat"] = f"lands whole but reads asymmetric (symmetry {q['symmetry']:.2f})"
            elif q["symmetry"] < 0.85:
                rec["confidence"] = "medium"
                rec["caveat"] = f"solid silhouette, moderate symmetry ({q['symmetry']:.2f})"
            else:
                rec["confidence"] = "high"
            # The numbers describe the SOURCE model folded by foldview today,
            # not this directory's older geometry.
            rec["metrics_from"] = "source model, swept fresh"

        if src:
            by_source[src].append(name)
        recs.append(rec)

    # Two legacy chassis ship no chassis.json at all, so their provenance is
    # lost. Their directory names match corpus models exactly, which is a
    # strong hint and nothing more — recorded as a guess, in a field that
    # cannot be mistaken for the measured `source`.
    GUESS = {"jet_c_130j_super_hercules": "imported_flyer_c_130j_super_hercules.iqm",
             "jet_whistlewind": "imported_hovertank_whistlewind_aka_hover_ifv_pbr.iqm"}
    for r in recs:
        if not r["source"] and r["name"] in GUESS:
            r["probable_source"] = GUESS[r["name"]]
            r["provenance"] = "no chassis.json shipped; source inferred from the directory name"
            for other in recs:
                if other["source"] == GUESS[r["name"]] and other.get("confidence") != "unknown":
                    r["sweep"] = other["sweep"]
                    r["confidence"] = other["confidence"]
                    r["metrics"] = other.get("metrics", {})
                    r["metrics_from"] = f"{other['name']} (via inferred source)"
                    break

    dup_pairs = 0
    for src, names in by_source.items():
        if len(names) < 2:
            continue
        dup_pairs += 1
        for r in recs:
            if r["name"] in names:
                r["duplicate_of"] = [n for n in names if n != r["name"]]
    # A legacy chassis built from a model the fleet also exported inherits that
    # model's metrics: the numbers describe the SOURCE and the fold, which are
    # the same. Its own geometry is still the old per-pose merge, which is why
    # `morph` is measured per directory rather than copied.
    byname = {r["name"]: r for r in recs}
    for r in recs:
        if r["confidence"] != "unknown":
            continue
        for twin in r.get("duplicate_of", []):
            t = byname.get(twin, {})
            if t.get("confidence", "unknown") != "unknown":
                r["sweep"] = t["sweep"]
                r["confidence"] = t["confidence"]
                if "metrics" in t:
                    r["metrics"] = t["metrics"]
                if "caveat" in t:
                    r["caveat"] = t["caveat"]
                r["metrics_from"] = twin
                break

    manifest = dict(
        generator="foldview --strip, packed with tools/pack_bots.py",
        indexed=__import__("time").strftime("%Y-%m-%d"),
        chassis=len(recs),
        morph_accepted=sum(1 for r in recs if r["morph"] == "accepted"),
        morph_refused=sum(1 for r in recs if r["morph"] == "refused"),
        unpacked=sum(1 for r in recs if not r["packed"]),
        duplicate_pairs=dup_pairs,
        by_origin=dict(Counter(r["origin"] for r in recs)),
        by_confidence=dict(Counter(r["confidence"] for r in recs)),
        read_me={
            "layout": "<name>/f00..f04 (fold) and optional w00..w03 (walk), each with scene.gltf and orient.json; geometry lives in bots.pack / walk.pack, decoded by src/botpack.rs",
            "poses": "f00 is the vehicle, f04 the robot; the three between are the fold, evenly spaced in fold time",
            "morph": "accepted = the strip satisfies xform::Morph::build (same prims, same vertex counts, every pose); refused = the game steps the poses instead of folding them",
            "origin": {"legacy": "in the tree before the fleet export; merged per pose, so most refuse the morph",
                       "fleet": "the 173-vehicle export",
                       "added": "walkers, helicopters, submarines, airships, bikes"},
            "sweep": {"tuned": "in the 113-model line the fold was tuned against",
                      "cold": "swept once with the same metric, never tuned against",
                      "none": "no metrics exist for this chassis"},
            "confidence": {"high": "single blob both views at t=1, symmetry >= 0.85",
                           "medium": "single blob, symmetry 0.70-0.85",
                           "low": "single blob but asymmetric or scatters mid-fold",
                           "broken": "more than one blob at t=1 — expect a piece adrift",
                           "unknown": "never swept"},
            "duplicate_of": "same source model under a second directory name; the short name is the one older code references",
            "excluded": "Fall-of-Cybertron and other rip-derived models are deliberately absent",
        },
        fleet=recs,
    )
    json.dump(manifest, open(os.path.join(ROOT, "fleet.json"), "w"), indent=1)
    print(f"{len(recs)} chassis indexed")
    print(f"  morph accepted {manifest['morph_accepted']}  refused {manifest['morph_refused']}")
    print(f"  unpacked {manifest['unpacked']}  duplicate pairs {manifest['duplicate_pairs']}")
    print("  origin:", manifest["by_origin"])
    print("  confidence:", manifest["by_confidence"])


if __name__ == "__main__":
    main()
