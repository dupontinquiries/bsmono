#!/usr/bin/env python3
"""wiring_check.py — is a ported module actually CALLED by its app?

The parity audit counts modules that exist. This counts modules the app
reaches. The difference is the whole "dud demo" problem: snapclone had a
camera, an inbox, a composer, contacts, a conversation view and group voice —
six modules, 66 passing tests — and the binary called NONE of them. It drew a
placeholder rectangle labelled "live preview".

A module with tests and no caller is not a port. It is a library nobody uses,
and it looks exactly like an unfinished app because functionally it is one.
"""
import os, re, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))

def modules_of(src):
    """Modules declared in lib.rs."""
    try: t = open(os.path.join(src, 'lib.rs'), encoding='utf8').read()
    except Exception: return []
    return re.findall(r'^pub mod (\w+);', t, re.M)

def reached(src, mod):
    """Is `mod` referenced anywhere outside its own file?"""
    for root, dirs, files in os.walk(src):
        dirs[:] = [d for d in dirs if d != 'target']
        for f in files:
            if not f.endswith('.rs') or f == f'{mod}.rs': continue
            p = os.path.join(root, f)
            try: t = open(p, encoding='utf8').read()
            except Exception: continue
            # a use, a path call, or a field of that type
            if re.search(rf'\b{re.escape(mod)}::', t): return True
    return False

# Patterns that make a view LOOK wired while showing nothing. Every one of these
# passed the reachability check above, which is how 26 dead modules became 0 with
# no new functionality.
PLACEHOLDERS = [
    (r'Vec::new[(][)]\s*;\s*$', 'a view fed an empty Vec::new()'),
    (r'::new[(]&\[\],\s*&\[\][)]', 'a model built from empty slices'),
    (r'let\s+\w+\s*:\s*Vec<[^>]+>\s*=\s*Vec::new\(\)\s*;', 'a local empty list handed to a view'),
]

def placeholder_feeds(src):
    """Empty literals fed straight into a drawing call, in the same function."""
    hits = []
    for root, dirs, files in os.walk(src):
        dirs[:] = [d for d in dirs if d != 'target']
        for f in files:
            if not f.endswith('.rs'): continue
            p = os.path.join(root, f)
            try: lines = open(p, encoding='utf8').read().split('\n')
            except Exception: continue
            for i, line in enumerate(lines):
                for pat, why in PLACEHOLDERS:
                    if re.search(pat, line.strip()):
                        # only flag it when a draw call follows within a few lines
                        tail = '\n'.join(lines[i:i+6])
                        if re.search(r'_ui\(|render_|\.ui\(|show\(', tail):
                            hits.append((os.path.relpath(p, src), i + 1, why))
    return hits

def main():
    apps = os.path.join(ROOT, 'apps')
    rows = []
    for name in sorted(os.listdir(apps)):
        src = os.path.join(apps, name, 'egui-frontend', 'src')
        if not os.path.isdir(src): continue
        mods = [m for m in modules_of(src) if m not in ('util', 'config')]
        dead = [m for m in mods if not reached(src, m)]
        rows.append((name, len(mods), dead))
    w = max((len(r[0]) for r in rows), default=8)
    print(f"{'app':<{w}}  modules  wired  DEAD")
    total_dead = 0
    for name, n, dead in rows:
        total_dead += len(dead)
        print(f"{name:<{w}}  {n:7}  {n-len(dead):5}  {len(dead)}")
    for name, _, dead in rows:
        if dead: print(f"\n{name}: never called — {', '.join(dead)}")
    if total_dead:
        print(f"\n{total_dead} ported modules are unreachable from their app.")

    # Reachable is not the same as FED.
    total_stub = 0
    for name in sorted(os.listdir(apps)):
        src = os.path.join(apps, name, 'egui-frontend', 'src')
        if not os.path.isdir(src): continue
        hits = placeholder_feeds(src)
        if not hits: continue
        total_stub += len(hits)
        print(f"\n{name}: views fed placeholder data —")
        for rel, ln, why in hits:
            print(f"  {rel}:{ln}  {why}")
    if total_stub:
        print(f"\n{total_stub} views are wired to nothing. A view fed an empty "
              f"literal is a connected placeholder, and it is worse than an "
              f"unwired module because this checker would otherwise call it done.")
    return 1 if (total_dead or total_stub) else 0

if __name__ == '__main__':
    sys.exit(main())
