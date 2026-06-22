#!/usr/bin/env python3
"""Sweep one parameter across many scenes, render WORLD-ONLY, score each, and record a matrix — to MAP which
knobs break which scenes (washout/void/monochrome) and which are safe. Reuses one Xvfb across renders (fast) and
is RESUMABLE (skips cells already in the CSV), so it can be run in batches.

Usage:
  sweep_matrix.py --param LOGSCOPE_HOUR --values 7,10,13,16,19,22 \
      --scenes "Mansion Garden,Reading Room,Mistfall,Slate,Folding@home,Atlantis" [--fixed "K=v K=v"]

Writes /home/claude/candidates/_matrix/<param>.csv: scene,value,score,note,weakest,<all sub-scores>.
"""
import sys, os, time, subprocess, json
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import score_render as SR

SUITE = "/home/claude/logscope-suite"; BIN = f"{SUITE}/target/debug/scene-view"
OUT = "/home/claude/candidates/_matrix"; os.makedirs(OUT, exist_ok=True)


def arg(flag, default=None):
    return sys.argv[sys.argv.index(flag) + 1] if flag in sys.argv else default


def render(scene, param, val, fixed, img):
    env = dict(os.environ)
    env.update(dict(DISPLAY=":99", LIBGL_ALWAYS_SOFTWARE="1", GALLIUM_DRIVER="llvmpipe",
                    LOGSCOPE_WORLD_ONLY="1", LOGSCOPE_THEME=scene, LOGSCOPE_SAT="1.2",
                    POS_ANGLE_INIT="0.8,0.32,42"))
    env[param] = str(val)
    for kv in fixed.split():
        if "=" in kv:
            k, v = kv.split("=", 1); env[k] = v
    subprocess.run("pkill -9 scene-view 2>/dev/null; sleep 0.5", shell=True)
    p = subprocess.Popen([BIN], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(6)
    subprocess.run(f"import -display :99 -window root {img} 2>/dev/null", shell=True)
    p.kill(); subprocess.run("pkill -9 scene-view 2>/dev/null", shell=True)


def main():
    param = arg("--param", "LOGSCOPE_HOUR")
    values = arg("--values", "7,10,13,16,19,22").split(",")
    scenes = arg("--scenes", "Mistfall,Slate").split(",")
    fixed = arg("--fixed", "")
    cfg = SR.load_cfg(SR.DEFAULT_RULES)
    csv = f"{OUT}/{param}.csv"
    done = set()
    if os.path.exists(csv):
        for ln in open(csv).read().splitlines()[1:]:
            c = ln.split(","); done.add((c[0], c[1]))
    else:
        open(csv, "w").write("scene,value,score,note,weakest," + ",".join(cfg["weights"]) + "\n")

    subprocess.run("pkill -9 Xvfb 2>/dev/null; sleep 1", shell=True)
    subprocess.Popen("Xvfb :99 -screen 0 560x440x24 -ac +extension GLX +render -noreset >/dev/null 2>&1 &", shell=True)
    time.sleep(3)
    for scene in scenes:
        for val in values:
            if (scene, val) in done:
                continue
            img = f"{OUT}/{scene.replace(' ', '_').replace('@', '_')}__{param}_{val}.png"
            render(scene, param, val, fixed, img)
            m = SR.metrics(img, cfg); total, sub, note = SR.score(m, cfg)
            weakest = min(sub, key=sub.get)
            open(csv, "a").write(f"{scene},{val},{total:.4f},{note or '-'},{weakest}," +
                                 ",".join(f"{sub[k]:.2f}" for k in cfg["weights"]) + "\n")
            print(f"{scene:16s} {param}={val:>4}  {total:.3f}  [{note or '-':7s}] weakest={weakest}")
    subprocess.run("pkill -9 scene-view 2>/dev/null; pkill -9 Xvfb 2>/dev/null", shell=True)
    print(f"\nmatrix -> {csv}")


if __name__ == "__main__":
    main()
