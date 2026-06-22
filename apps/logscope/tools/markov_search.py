#!/usr/bin/env python3
"""Markov-chain (Metropolis / simulated-annealing) search over a WIDE continuous parameter space, rendering the
WORLD-ONLY scene each step and scoring it (optionally against a reference). Far more sample-efficient than a grid:
the chain follows promising trajectories and concentrates samples in good regions.

Usage:
  markov_search.py "<Scene>" --steps 8 [--ref tools/refs/<key>.png] [--ref-weight 0.3] [--fresh] [--seed 0x2222]

State is checkpointed to /home/claude/candidates/<key>/mcmc_state.json so the chain RESUMES across invocations
(rendering is slow — run a few steps per call). Trajectory -> mcmc_traj.csv; best render kept as best.png.
Candidates are pure env overrides into a scratch dir; the committed scene code is never touched.
"""
import sys, os, json, math, random, subprocess, time
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import score_render as SR

SUITE = "/home/claude/logscope-suite"
BIN = f"{SUITE}/target/debug/scene-view"

# name: (lo, hi, gaussian step). 'yaw'/'pitch' fold into POS_ANGLE_INIT; the rest are LOGSCOPE_* env vars.
PARAMS = {
    "LOGSCOPE_TUNE_SCALE":   (0.70, 1.80, 0.15),
    "LOGSCOPE_TUNE_HEIGHT":  (0.60, 2.20, 0.22),
    "LOGSCOPE_TUNE_DENSITY": (0.60, 2.00, 0.22),
    "LOGSCOPE_HOUR":         (5.0, 22.0, 2.5),
    "yaw":                   (0.20, 2.60, 0.30),
    "pitch":                 (0.12, 0.55, 0.06),
}
T0, COOL = 0.06, 0.90   # annealing temperature + per-step cooling

# Learned from RULE_MAP.md: bright-material scenes are time-of-day FRAGILE (wash out in daylight, Mansion blacks
# out at dawn). Keep the chain in each scene's safe HOUR window instead of re-discovering the washouts.
SCENE_HOUR_RANGE = {
    "mansion_garden": (18.0, 23.0),
    "reading_room":   (18.0, 23.0),
    "atlantis":       (17.0, 23.0),
}


def clamp(v, lo, hi):
    return max(lo, min(hi, v))


def env_of(p):
    e = {k: f"{p[k]:.3f}" for k in p if k not in ("yaw", "pitch")}
    e["POS_ANGLE_INIT"] = f"{p['yaw']:.3f},{p['pitch']:.3f},40"
    return e


def render(scene, p, seed, outpath):
    env = dict(os.environ)
    env.update(dict(DISPLAY=":99", LIBGL_ALWAYS_SOFTWARE="1", GALLIUM_DRIVER="llvmpipe",
                    LOGSCOPE_WORLD_ONLY="1", LOGSCOPE_THEME=scene, LOGSCOPE_SEED=str(seed),
                    LOGSCOPE_SAT="1.2", LOGSCOPE_EXP="1.0"))
    env.update(env_of(p))
    subprocess.run("pkill -9 Xvfb 2>/dev/null; pkill -9 scene-view 2>/dev/null; sleep 1", shell=True)
    subprocess.Popen("Xvfb :99 -screen 0 560x440x24 -ac +extension GLX +render -noreset >/dev/null 2>&1 &", shell=True)
    time.sleep(2)
    proc = subprocess.Popen([BIN], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(6)
    subprocess.run(f"import -display :99 -window root {outpath} 2>/dev/null", shell=True)
    proc.kill()
    subprocess.run("pkill -9 scene-view 2>/dev/null; pkill -9 Xvfb 2>/dev/null; sleep 1", shell=True)


def score_of(img, cfg, ref, ref_w):
    m = SR.metrics(img, cfg)
    rule, _, note = SR.score(m, cfg)
    if ref and os.path.exists(ref):
        rs, _ = SR.ref_similarity(img, ref)
        gate = 1.0 if note == "" else 0.4
        return ((1 - ref_w) * rule + ref_w * rs) * gate, note
    return rule * (1.0 if note == "" else (0.0 if note == "BLACK" else 0.4)), note


def main():
    scene = sys.argv[1]
    key = scene.lower().replace(" ", "_").replace("@", "_")
    steps = int(sys.argv[sys.argv.index("--steps") + 1]) if "--steps" in sys.argv else 8
    ref = sys.argv[sys.argv.index("--ref") + 1] if "--ref" in sys.argv else f"{SUITE}/tools/refs/{key}.png"
    if not os.path.exists(ref):
        ref = None
    ref_w = float(sys.argv[sys.argv.index("--ref-weight") + 1]) if "--ref-weight" in sys.argv else 0.3
    seed = int(sys.argv[sys.argv.index("--seed") + 1], 0) if "--seed" in sys.argv else 0x2222
    fresh = "--fresh" in sys.argv
    out = f"/home/claude/candidates/{key}"
    os.makedirs(out, exist_ok=True)
    if key in SCENE_HOUR_RANGE:                       # respect the learned safe-hour window for fragile scenes
        lo, hi = SCENE_HOUR_RANGE[key]
        PARAMS["LOGSCOPE_HOUR"] = (lo, hi, (hi - lo) / 5.0)
        print(f"[rule] {key} is time-of-day fragile → constraining HOUR to [{lo},{hi}]")
    statef = f"{out}/mcmc_state.json"
    cfg = SR.load_cfg(SR.DEFAULT_RULES)

    if fresh or not os.path.exists(statef):
        cur = {k: (lo + hi) / 2 for k, (lo, hi, _) in PARAMS.items()}
        img = f"{out}/mcmc_cur.png"; render(scene, cur, seed, img)
        cs, note = score_of(img, cfg, ref, ref_w)
        st = dict(cur=cur, cs=cs, best=cur, bs=cs, it=0)
        open(f"{out}/mcmc_traj.csv", "w").write("it,score,note,accepted," + ",".join(PARAMS) + "\n")
        open(f"{out}/mcmc_traj.csv", "a").write(f"0,{cs:.4f},{note},init," + ",".join(f'{cur[k]:.3f}' for k in PARAMS) + "\n")
        subprocess.run(f"cp {img} {out}/best.png", shell=True)
        print(f"[init] score={cs:.4f} {note}  ref={'yes' if ref else 'no'}")
    else:
        st = json.load(open(statef))

    cur, cs, best, bs, it = st["cur"], st["cs"], st["best"], st["bs"], st["it"]

    for _ in range(steps):
        it += 1
        rng = random.Random(seed * 100003 + it * 7919 + 13)  # deterministic per-iteration (resumable, no pickling)
        T = T0 * (COOL ** it)
        prop = dict(cur)
        for k, (lo, hi, step) in PARAMS.items():
            prop[k] = clamp(cur[k] + rng.gauss(0, step), lo, hi)
        img = f"{out}/mcmc_it{it:03d}.png"
        render(scene, prop, seed, img)
        ps, note = score_of(img, cfg, ref, ref_w)
        dE = ps - cs
        accepted = dE > 0 or rng.random() < math.exp(dE / max(T, 1e-6))
        if accepted:
            cur, cs = prop, ps
        if ps > bs:
            bs, best = ps, dict(prop)
            subprocess.run(f"cp {img} {out}/best.png", shell=True)
        open(f"{out}/mcmc_traj.csv", "a").write(f"{it},{ps:.4f},{note},{int(accepted)}," + ",".join(f'{prop[k]:.3f}' for k in PARAMS) + "\n")
        print(f"[it {it}] prop={ps:.4f} {note} {'ACC' if accepted else 'rej'}  cur={cs:.4f}  best={bs:.4f}  T={T:.3f}")

    st = dict(cur=cur, cs=cs, best=best, bs=bs, it=it)
    json.dump(st, open(statef, "w"), indent=1)
    print(f"\nbest score={bs:.4f}  params=" + " ".join(f"{k}={best[k]:.3f}" for k in PARAMS))
    print(f"best render: {out}/best.png   trajectory: {out}/mcmc_traj.csv")


if __name__ == "__main__":
    main()
