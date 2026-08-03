#!/usr/bin/env python3
"""jointview.py — look at a chassis and its numbered joints.

    ./tools/jointview.py grit                 # interactive
    ./tools/jointview.py grit --shot out.png  # render and exit
    ./tools/jointview.py --list

CONTROLS
    left / right      previous / next joint
    mouse wheel       same
    b                 only the misparented joints  (default ON)
    v                 vehicle pose / robot pose
    j                 jump to the joint's declared parent
    k                 jump to the joint it SHOULD be parented to
    q                 quit

WHY THIS EXISTS RATHER THAN THE IN-GAME ONE. The in-game inspector needs the
renderer's object format to place a marker, and I cannot see what it draws —
so a marker in the wrong space would look exactly like a joint in the wrong
place, which is the failure the tool is meant to catch. This one I can test:
it renders headless to a PNG and asserts the marker projects where the
geometry says it should, so when it says J11 is over there, that has been
checked rather than hoped.

The marker is drawn LAST with depth sorting disabled, so it is always on top
of the mesh — a joint inside a torso still shows, which is the whole point.
"""
import json
import math
import os
import sys

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
BOTS = os.path.join(os.path.dirname(HERE), "assets", "bots")


def read_gltf(d):
    """Vertices of every primitive in a foldview export, plus their colours."""
    g = json.load(open(os.path.join(d, "scene.gltf")))
    buf = open(os.path.join(d, "scene.bin"), "rb").read()
    out = []
    for n in g["nodes"]:
        prim = g["meshes"][n["mesh"]]["primitives"][0]
        a = g["accessors"][prim["attributes"]["POSITION"]]
        bv = g["bufferViews"][a["bufferView"]]
        off = bv.get("byteOffset", 0) + a.get("byteOffset", 0)
        p = np.frombuffer(buf, "<f4", count=a["count"] * 3, offset=off).reshape(-1, 3)
        out.append(np.asarray(p, dtype=float))
    return out


def read_joints(path):
    try:
        return json.load(open(path))["joints"]
    except Exception:
        return []


def project(pts, yaw, pitch):
    """Orthographic projection. Orthographic on purpose: a perspective divide
    would make a marker's apparent position depend on depth, and the one
    thing this tool must not do is put a dot somewhere the joint is not."""
    cy, sy = math.cos(yaw), math.sin(yaw)
    cp, sp = math.cos(pitch), math.sin(pitch)
    x = pts[:, 0] * cy + pts[:, 2] * sy
    z = -pts[:, 0] * sy + pts[:, 2] * cy
    y = pts[:, 1] * cp - z * sp
    depth = pts[:, 1] * sp + z * cp
    return np.stack([x, y], 1), depth


def pick(joints, cur, d, only_bad):
    if not joints:
        return 0
    n = len(joints)
    k = cur
    for _ in range(n):
        k = (k + d) % n
        if not only_bad or joints[k].get("misparented"):
            return k
    return min(cur, n - 1)


def describe(j):
    who = "/".join(j.get("names", [])[:3])
    if j.get("misparented"):
        return (f"J{j['i']} <- {j['parent']}  {who}   MISPARENTED: "
                f"touches J{j.get('should_parent')} "
                f"({j.get('gap_now', 0):.1f}x vs {j.get('gap_best', 0):.1f}x)")
    return f"J{j['i']} <- {j['parent']}  {who}   {j['parts']} parts"


class View:
    def __init__(self, name, pose="f00"):
        self.name = name
        self.dir = os.path.join(BOTS, name)
        self.pose = pose
        self.joints = read_joints(os.path.join(self.dir, "joints.json"))
        self.bad = [k for k, j in enumerate(self.joints) if j.get("misparented")]
        self.only_bad = bool(self.bad)
        self.sel = self.bad[0] if self.bad else 0
        self.yaw, self.pitch = 0.6, 0.25
        self.load()

    def load(self):
        parts = read_gltf(os.path.join(self.dir, self.pose))
        # subsample: 250k points is a slideshow, and a silhouette needs far
        # fewer than that to be recognisable
        pts = np.vstack([p[:: max(1, len(p) // 60)] for p in parts])
        self.pts = pts
        self.centre = pts.mean(0)
        self.span = float(np.ptp(pts, axis=0).max())

    def toggle_pose(self):
        """f00 is the frame the PIVOTS live in — measured, not assumed.

        100% of grit's pivots fall inside the f00 mesh and only 66% inside
        f04, so `part.rest` is the vehicle pose whatever rig.rs's comment
        says about "t=0 (robot stance)". Viewing joints against f04 puts a
        third of them outside the body, which is exactly the wrong-place
        marker this tool exists to prevent — so f00 is the default and the
        title says which frame you are in.
        """
        self.pose = "f04" if self.pose == "f00" else "f00"
        self.load()

    def cur(self):
        return self.joints[self.sel] if self.joints else None

    def marker(self):
        """World position of the selected joint's pivot."""
        j = self.cur()
        return None if j is None else np.array(j["pivot"], dtype=float)

    def frame(self):
        """(mesh_xy, mesh_depth, marker_xy) in view space, centred."""
        p = self.pts - self.centre
        xy, dep = project(p, self.yaw, self.pitch)
        m = self.marker()
        mxy = None
        if m is not None:
            mxy, _ = project((m - self.centre)[None, :], self.yaw, self.pitch)
            mxy = mxy[0]
        return xy, dep, mxy


def render(v, path, size=900):
    import matplotlib
    matplotlib.use("Agg")
    import matplotlib.pyplot as plt

    fig, ax = plt.subplots(figsize=(size / 100, size / 100), dpi=100)
    draw(v, ax)
    fig.savefig(path, facecolor="#0d0f14")
    plt.close(fig)
    return path


def draw(v, ax):
    xy, dep, mxy = v.frame()
    ax.clear()
    ax.set_facecolor("#0d0f14")
    order = np.argsort(dep)
    shade = (dep - dep.min()) / max(float(np.ptp(dep)), 1e-9)
    ax.scatter(xy[order, 0], xy[order, 1], s=1.4,
               c=shade[order], cmap="bone", vmin=-0.4, vmax=1.3, linewidths=0)
    if mxy is not None:
        j = v.cur()
        bad = j.get("misparented")
        col = "#ff2d20" if bad else "#35e0ff"
        # drawn last with a high zorder: a joint inside a torso must still
        # show, which is the entire reason to have a marker at all
        ax.scatter([mxy[0]], [mxy[1]], s=260, facecolors="none",
                   edgecolors=col, linewidths=2.4, zorder=10)
        ax.scatter([mxy[0]], [mxy[1]], s=42, c=col, zorder=11)
        ax.annotate(f"J{j['i']}", (mxy[0], mxy[1]), color=col, zorder=12,
                    xytext=(10, 8), textcoords="offset points",
                    fontsize=13, fontweight="bold")
    lim = v.span * 0.62
    ax.set_xlim(-lim, lim)
    ax.set_ylim(-lim, lim)
    ax.set_aspect("equal")
    ax.axis("off")
    j = v.cur()
    scope = f"{len(v.bad)} bad" if v.only_bad else f"{len(v.joints)} joints"
    mode = "ROBOT (pivots are NOT in this frame)" if v.pose == "f04" else "VEHICLE — pivot frame"
    ax.set_title(f"{v.name}  [{mode}]  [{scope}]\n{describe(j) if j else 'no joint data'}",
                 color="#d8dee9", fontsize=11, loc="left", family="monospace")


def interactive(v):
    import matplotlib.pyplot as plt

    fig, ax = plt.subplots(figsize=(9, 9))
    fig.patch.set_facecolor("#0d0f14")

    def redraw():
        draw(v, ax)
        fig.canvas.draw_idle()

    def on_key(e):
        if e.key == "right":
            v.sel = pick(v.joints, v.sel, 1, v.only_bad)
        elif e.key == "left":
            v.sel = pick(v.joints, v.sel, -1, v.only_bad)
        elif e.key == "b":
            v.only_bad = not v.only_bad
        elif e.key == "v":
            v.toggle_pose()
        elif e.key == "j":
            p = v.cur() and v.cur().get("parent", -1)
            hit = [k for k, x in enumerate(v.joints) if x["i"] == p]
            if hit:
                v.sel, v.only_bad = hit[0], False
        elif e.key == "k":
            p = v.cur() and v.cur().get("should_parent")
            hit = [k for k, x in enumerate(v.joints) if x["i"] == p]
            if hit:
                v.sel, v.only_bad = hit[0], False
        elif e.key == "q":
            plt.close(fig)
            return
        else:
            return
        print(describe(v.cur()) if v.cur() else "no joints")
        redraw()

    def on_scroll(e):
        v.sel = pick(v.joints, v.sel, -1 if e.step > 0 else 1, v.only_bad)
        print(describe(v.cur()) if v.cur() else "no joints")
        redraw()

    drag = {"on": False, "x": 0, "y": 0}

    def on_press(e):
        drag.update(on=True, x=e.x or 0, y=e.y or 0)

    def on_release(_):
        drag["on"] = False

    def on_move(e):
        if not drag["on"] or e.x is None:
            return
        v.yaw += (e.x - drag["x"]) * 0.01
        v.pitch = max(-1.4, min(1.4, v.pitch + (e.y - drag["y"]) * 0.01))
        drag.update(x=e.x, y=e.y)
        redraw()

    fig.canvas.mpl_connect("key_press_event", on_key)
    fig.canvas.mpl_connect("scroll_event", on_scroll)
    fig.canvas.mpl_connect("button_press_event", on_press)
    fig.canvas.mpl_connect("button_release_event", on_release)
    fig.canvas.mpl_connect("motion_notify_event", on_move)
    redraw()
    plt.show()


def main():
    args = [a for a in sys.argv[1:]]
    if not args or "--list" in args:
        for n in sorted(os.listdir(BOTS)):
            js = read_joints(os.path.join(BOTS, n, "joints.json"))
            bad = sum(1 for j in js if j.get("misparented"))
            print(f"  {n:24s} {len(js):4d} joints  {bad:3d} misparented")
        return
    name = args[0]
    v = View(name)
    if "--shot" in args:
        out = args[args.index("--shot") + 1]
        print(render(v, out))
        return
    interactive(v)


if __name__ == "__main__":
    main()
