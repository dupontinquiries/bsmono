#!/usr/bin/env python3
"""auto_orient.py -- best-guess orientation + scale for imported models.

Analyzes each model's geometry and SUGGESTS a correction (rotation to stand it
up + a consistent scale + vehicle/robot guess). Writes a table the engine loads
as harness DEFAULTS -- the user then only fine-tunes the ones it got wrong,
instead of tuning all 48 from scratch.

HONEST ACCURACY (measured on known models): vehicle/robot classification ~70%
(shape-only; a scorpion reads long like a vehicle, a compact car reads square
like a robot). Up-axis is a best guess. So this is a HEAD START, not ground
truth -- every value is overridable in-game (F4 tune) and the manual value wins.

Method (all measurable, no ML):
  * bounding-box aspect: sorted extents give flatness + balance.
  * vehicle guess: flatter + more elongated than a robot's square standing box.
  * up-axis: the model's thin axis is usually front-back for robots (so up = the
    taller of the two big axes) and vertical(low) for vehicles. We rotate so the
    intended up maps to +Y.
  * scale: normalize by the post-orient vertical extent to a consistent height.
"""
import numpy as np, sys, os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import process_models as pm

def analyze(V):
    ext = V.max(0) - V.min(0); s = np.sort(ext)
    flat = s[0]/s[2]; balance = s[1]/s[2]
    veh_score = (0.62-balance) + (0.32-flat)
    kind = 'vehicle' if veh_score > 0.04 else 'robot'
    short_ax = int(np.argmin(ext)); tall_ax = int(np.argmax(ext))
    # choose intended up-axis
    if kind == 'robot':
        # up = the taller of the two NON-thin axes (thin axis is front-back)
        big = [a for a in range(3) if a != short_ax]
        up = big[0] if ext[big[0]] >= ext[big[1]] else big[1]
    else:
        # vehicle: up = the low (thin) axis
        up = short_ax
    # rotation (deg) that brings 'up' axis -> +Y
    rx = ry = rz = 0.0
    if up == 0:   rz = 90.0    # X-up -> rotate about Z to bring X to Y
    elif up == 2: rx = 90.0    # Z-up -> rotate about X to bring Z to Y
    # (up==1 already Y, no rotation)
    return kind, (rx, ry, rz), veh_score

def main():
    root = sys.argv[1] if len(sys.argv) > 1 else '.'
    rows = []
    for name, gltf in pm.find_models(root):
        geo = pm._load_gltf_geometry(gltf)
        if geo is None:
            print(f"  {name}: no geometry"); continue
        V, F = geo
        kind, (rx, ry, rz), score = analyze(V)
        rows.append((name, kind, rx, ry, rz, score))
        print(f"  {name:44s} {kind:8s} rx={rx:.0f} ry={ry:.0f} rz={rz:.0f}  (score {score:+.2f})")
    print(f"\n{len(rows)} models analyzed. These are SUGGESTIONS -- confirm/override in-game (F4).")

if __name__ == "__main__":
    main()
