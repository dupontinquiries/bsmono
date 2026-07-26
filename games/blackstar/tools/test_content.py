#!/usr/bin/env python3
"""Self-test for the content pipeline: valid content passes; broken references are
caught; the resolved manifest has the expected shape."""
import json, os, subprocess, sys, tempfile, shutil

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)

def run(root):
    r = subprocess.run([sys.executable, os.path.join(HERE, "content_build.py"),
                        "--root", root, "--quiet"], capture_output=True, text=True)
    return r.returncode, r.stdout + r.stderr

def main():
    # 1) the real content validates cleanly
    rc, out = run(ROOT)
    assert rc == 0, f"real content should validate, got:\n{out}"
    man = json.load(open(os.path.join(ROOT, "content", "_manifest.json")))
    s = man["summary"]
    assert s["chassis"] == 7 and s["signatures"] == 10 and s["fire_modes"] == 10, s
    assert s["troops"] >= 17 and s["maps"] == 14 and s["missions"] == 8, s
    # every resolved troop got a concrete cannon + backpiece mesh
    for tn, t in man["troops"].items():
        assert t["cannon_mesh"] and t["backpiece_mesh"], f"troop {tn} unresolved: {t}"
    print(f"  real content OK: {s['troops']} troops resolved to concrete meshes")

    # 2) a broken reference must be caught -- copy the tree, corrupt a troop, expect failure
    tmp = tempfile.mkdtemp()
    try:
        shutil.copytree(os.path.join(ROOT, "content"), os.path.join(tmp, "content"))
        os.makedirs(os.path.join(tmp, "engine_src", "data"))
        for f in os.listdir(os.path.join(ROOT, "engine_src", "data")):
            if f.endswith(".iqm"):
                open(os.path.join(tmp, "engine_src", "data", f), "w").close()
        os.makedirs(os.path.join(tmp, "engine_src", "exengine"))
        for f in os.listdir(os.path.join(ROOT, "engine_src", "exengine")):
            if f.endswith(".h"):
                shutil.copy(os.path.join(ROOT, "engine_src", "exengine", f),
                            os.path.join(tmp, "engine_src", "exengine", f))
        tf = os.path.join(tmp, "content", "troops.json")
        d = json.load(open(tf))
        d["troops"]["vanguard"]["chassis"] = "nonexistent_frame"
        json.dump(d, open(tf, "w"))
        rc2, out2 = run(tmp)
        assert rc2 != 0 and "unknown chassis" in out2, f"validator should have caught the bad ref:\n{out2}"
        print("  broken-reference detection OK: dangling chassis caught")
    finally:
        shutil.rmtree(tmp, ignore_errors=True)

    print("\nCONTENT PIPELINE OK")

if __name__ == "__main__":
    main()
