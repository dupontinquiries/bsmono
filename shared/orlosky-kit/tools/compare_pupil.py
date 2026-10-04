#!/usr/bin/env python3
"""Compare orlosky-kit's pupil detector with the original Python (OrloskyPupilDetectorLite.py) on the same frames.

usage: compare_pupil.py <eyetracker checkout> <video> [every-nth=10] [frames=60]
Needs python3 + opencv + numpy, ffmpeg, and a built `pupil_frames` example (cargo build --release --example pupil_frames).
The original's get_darkest_area sums uint8 into a uint8-typed accumulator under numpy 2 and wraps, so it is run with
that one line fixed (int64): the comparison is against what his algorithm MEANS.
"""
import glob, json, os, subprocess, sys, tempfile, types
repo, video = sys.argv[1], sys.argv[2]
nth = int(sys.argv[3]) if len(sys.argv) > 3 else 10
count = int(sys.argv[4]) if len(sys.argv) > 4 else 60
here = os.path.dirname(os.path.abspath(__file__))
exe = os.environ.get("PUPIL_FRAMES", os.path.join(here, "..", "target", "release", "examples", "pupil_frames"))
work = tempfile.mkdtemp()
subprocess.run(["ffmpeg", "-v", "error", "-y", "-i", video, "-vf", f"select='not(mod(n,{nth}))'", "-vsync", "vfr", "-frames:v", str(count), f"{work}/f%03d.png"], check=True)
for m in ("tkinter", "tkinter.filedialog"): sys.modules[m] = types.ModuleType(m)
sys.modules["tkinter"].Tk = None; sys.modules["tkinter"].filedialog = sys.modules["tkinter.filedialog"]
import cv2, numpy as np
cv2.imshow = lambda *a, **k: None
src = open(os.path.join(repo, "OrloskyPupilDetectorLite.py")).read()
needle = "    gray = cv2.cvtColor(image, cv2.COLOR_BGR2GRAY)\n    min_sum = float('inf')"
assert needle in src, "the original changed; update this script"
src = src.replace(needle, needle.replace("COLOR_BGR2GRAY)", "COLOR_BGR2GRAY).astype(np.int64)"))
mod = types.ModuleType("lite"); mod.__dict__.update(cv2=cv2, np=np); exec(compile(src, "lite", "exec"), mod.__dict__)
ref = {}
for f in sorted(glob.glob(f"{work}/f*.png")):
    img = cv2.imread(f)
    cv2.imwrite(f.replace(".png", ".pgm"), cv2.cvtColor(img, cv2.COLOR_BGR2GRAY))
    (cx, cy), (w, h), _ = mod.process_frame(img)
    ref[os.path.basename(f).replace(".png", ".pgm")] = (cx, cy, (w + h) / 2) if w > 0 else None
out = subprocess.run([exe, work], capture_output=True, text=True, check=True).stdout.split("\n")
dc, dd, miss = [], [], 0
for line in out:
    t = line.split()
    if not t: continue
    r = ref[t[0]]
    if t[1] == "none" or r is None:
        miss += (t[1] == "none") != (r is None); continue
    cx, cy, major, minor = map(float, t[1:5])
    dc.append(((cx - r[0]) ** 2 + (cy - r[1]) ** 2) ** 0.5); dd.append(abs((major + minor) / 2 - r[2]))
dc.sort(); dd.sort()
print(f"frames {len(ref)}  compared {len(dc)}  disagreements on found/not-found {miss}")
print(f"centre distance px: median {dc[len(dc)//2]:.2f}  90th {dc[int(len(dc)*0.9)]:.2f}  max {dc[-1]:.2f}")
print(f"mean diameter difference px: median {dd[len(dd)//2]:.2f}  90th {dd[int(len(dd)*0.9)]:.2f}  max {dd[-1]:.2f}")
