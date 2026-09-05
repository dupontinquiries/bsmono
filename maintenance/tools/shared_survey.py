#!/usr/bin/env python3
"""shared_survey.py — which shared crates are real, and who uses them.

Written after five shared crates in a row turned out to hold the real
implementation of something a port had stubbed — nifty-core for the graph,
media/catalog for the library, device-kit for the camera, mic-kit for the
microphone, voice-kit for whisper filtering — and a sixth, snapcore, looked
identical from the outside and was `unimplemented!()` throughout its proximity
layer.

Wiring a view to a stubbed crate compiles and then panics in front of a user,
which is strictly worse than the placeholder it replaced. So "check shared/
first" has to mean READING it, and this is the read.

Two columns matter:

* **stubs** — `unimplemented!()` / `todo!()` anywhere under src/. Non-zero means
  find out WHICH functions before depending on it; a crate can be 90% real with
  the one part you want missing, which is exactly snapcore.
* **ports** — how many egui frontends depend on it. A high-value crate with zero
  consumers is either genuinely unused or a gap someone is about to stub around.
"""
import os, re, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
SHARED = os.path.join(ROOT, 'shared')
APPS = os.path.join(ROOT, 'apps')

def count(path, pattern):
    n = 0
    for root, dirs, files in os.walk(path):
        dirs[:] = [d for d in dirs if d != 'target']
        for f in files:
            if not f.endswith('.rs'): continue
            try: t = open(os.path.join(root, f), encoding='utf8').read()
            except Exception: continue
            n += len(re.findall(pattern, t))
    return n

def stub_sites(path):
    out = []
    for root, dirs, files in os.walk(path):
        dirs[:] = [d for d in dirs if d != 'target']
        for f in files:
            if not f.endswith('.rs'): continue
            p = os.path.join(root, f)
            try: lines = open(p, encoding='utf8').read().split('\n')
            except Exception: continue
            for i, l in enumerate(lines):
                if re.search(r'unimplemented!|todo!\(', l):
                    out.append((os.path.relpath(p, path), i + 1))
    return out

def consumers(name):
    """How many distinct APPS depend on `name`, from any Cargo.toml under apps/ — not just the
    fixed `apps/<app>/egui-frontend/Cargo.toml` path this used to hardcode.

    Found via logscope: its real egui frontend is `crates/logscope-egui/Cargo.toml`, not
    `egui-frontend/Cargo.toml` (that path does not exist for this app at all — logscope's own
    layout puts every crate under `crates/`). The hardcoded path silently missed EVERY shared
    crate logscope-egui depends on, for the single largest, most actively developed app in this
    tree — confirmed by hand for math-kit, noise-kit, and scene-kit, all three real dependencies
    of logscope-egui that this reported as zero-consumer "candidates for something a port is
    currently faking" when they are anything but.

    Counts distinct top-level apps (not crates) so an app with five internal crates depending on
    the same shared crate still reads as one consumer, matching what the original "ports" column
    name means — how many apps use this, not how many files mention it.
    """
    seen_apps = set()
    for root, dirs, files in os.walk(APPS):
        dirs[:] = [d for d in dirs if d != 'target']
        if 'Cargo.toml' not in files: continue
        app = os.path.relpath(root, APPS).split(os.sep)[0]
        if app in seen_apps: continue
        c = os.path.join(root, 'Cargo.toml')
        try:
            if name in open(c, encoding='utf8').read():
                seen_apps.add(app)
        except Exception:
            pass
    return len(seen_apps)

def main():
    rows = []
    for name in sorted(os.listdir(SHARED)):
        src = os.path.join(SHARED, name, 'src')
        if not os.path.isdir(src): continue
        rows.append((name, count(src, r'unimplemented!|todo!\('),
                     count(src, r'\bpub fn '), consumers(name)))
    w = max(len(r[0]) for r in rows)
    print(f"{'crate':<{w}}  stubs  pub_fns  ports")
    for name, stubs, fns, ports in rows:
        mark = '  <-- READ BEFORE USING' if stubs else ''
        print(f"{name:<{w}}  {stubs:5}  {fns:7}  {ports:5}{mark}")

    stubbed = [r for r in rows if r[1]]
    for name, _, _, _ in stubbed:
        print(f"\n{name}: stubbed at —")
        for rel, ln in stub_sites(os.path.join(SHARED, name, 'src'))[:20]:
            print(f"  {rel}:{ln}")
        print("  The REST of the crate may be fine; check whether the part you "
              "want is in this list before depending on it.")

    unused = [r[0] for r in rows if r[3] == 0 and r[1] == 0 and r[2] > 5]
    if unused:
        print(f"\n{len(unused)} substantial crates with NO port consumer:")
        print("  " + ", ".join(unused))
        print("  Each is a candidate for something a port is currently faking.")
    return 0

if __name__ == '__main__':
    sys.exit(main())
