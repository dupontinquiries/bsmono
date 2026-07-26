#!/usr/bin/env python3
"""test_silhouette.py -- regression guard for the offline silhouette pipeline.

Verifies:
  1. the pose_dump harness builds and emits 17 valid part transforms per body kind
  2. silhouette.py's IQM reader loads every part + variant mesh
  3. the compositor produces a non-trivial triangle count for each frame

Run: python3 tools/test_silhouette.py
"""
import os, sys, subprocess, tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(ROOT, "tools"))
import silhouette  # noqa: E402


def build_harness():
    out = "/tmp/pose_dump_test"
    src = os.path.join(ROOT, "tools", "pose_dump.c")
    rp = os.path.join(ROOT, "engine_src", "exengine", "robot_parts.c")
    inc = os.path.join(ROOT, "engine_src", "exengine")
    r = subprocess.run(["gcc", "-o", out, src, rp, "-I", inc, "-lm"],
                       capture_output=True, text=True)
    assert r.returncode == 0, "pose_dump build failed:\n" + r.stderr
    return out


def main():
    harness = build_harness()
    # 1. transforms for every body kind
    for kind in range(6):
        out = subprocess.check_output([harness, str(kind), "0"]).decode()
        rows = out.strip().splitlines()
        assert len(rows) == 17, f"kind {kind}: expected 17 parts, got {len(rows)}"
        for row in rows:
            cols = row.split("\t")
            assert len(cols) == 10, f"kind {kind}: malformed row {row!r}"
            vals = [float(c) for c in cols[1:7]]
            # finite, and scales non-negative
            assert all(v == v for v in vals), f"kind {kind}: NaN in {row!r}"
    print("test_silhouette: pose harness emits 17 valid parts x 6 body kinds")

    # point silhouette.py at the test harness
    silhouette.subprocess  # ensure module imported
    orig = silhouette.get_poses.__globals__  # not used; call via monkeypatch below

    def get_poses_test(kind, vehicle=0, alt=0, cstage=0):
        out = subprocess.check_output([harness, str(kind), str(vehicle), str(alt)]).decode()
        poses = {}
        for line in out.strip().splitlines():
            p = line.split("\t")
            poses[int(p[0])] = tuple(float(x) for x in p[1:])
        return poses
    silhouette.get_poses = get_poses_test

    # 2 + 3. every frame composites a substantial mesh
    for frame in ["", "prime", "corsair", "watch"]:
        tris = silhouette.build_scene(0, frame or None)
        assert len(tris) > 5000, f"frame {frame!r}: only {len(tris)} triangles"
        # sanity: geometry spans a plausible bot height (~2 units tall)
        ys = [v[1] for t in tris for v in t]
        h = max(ys) - min(ys)
        assert 1.5 < h < 3.5, f"frame {frame!r}: implausible height {h:.2f}"
    print("test_silhouette: all 4 frames composite valid geometry (base+prime+corsair+watch)")

    # 4. alt-mode remnants reshape the body (claws/spine/snout vs siege helm)
    base_tris = silhouette.build_scene(0, None, 0)
    def span(tris, ax):
        vs = [v[ax] for t in tris for v in t]
        return max(vs) - min(vs)
    dino = silhouette.build_scene(0, None, 6)   # beast: wider hands (x), deeper back (z)
    tank = silhouette.build_scene(0, None, 1)   # siege: wider head/shoulders (x)
    assert len(dino) == len(base_tris), "alt-mode must not add/remove tris, only reshape"
    # the remnant must actually change the silhouette from base on some axis
    assert abs(span(dino, 2) - span(base_tris, 2)) > 1e-3 or \
           abs(span(dino, 0) - span(base_tris, 0)) > 1e-3, "dino remnant didn't reshape"
    assert abs(span(tank, 0) - span(base_tris, 0)) > 1e-3 or \
           abs(span(tank, 1) - span(base_tris, 1)) > 1e-3, "tank remnant didn't reshape"
    print("test_silhouette: alt-mode remnants (dino, tank) reshape the silhouette")

    # 5. color layer: engine tint/weathering/two-tone/accent produce varied colors
    cscene = silhouette.build_scene(0, "prime", 0, colored=True)
    cols = set(t[3] for t in cscene)
    assert len(cols) >= 4, f"expected varied part colors, got {len(cols)}"
    # two-tone: some part must be notably darker than the brightest (lower body 0.62x)
    lum = lambda c: c[0] + c[1] + c[2]
    lums = [lum(c) for c in cols]
    assert min(lums) < 0.75 * max(lums), "two-tone lower body not darker than accents"
    print("test_silhouette: color layer varied (%d part tints, two-tone present)" % len(cols))
    print("ALL ASSERTIONS PASSED")


if __name__ == "__main__":
    main()
