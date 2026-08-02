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
    return 1 if total_dead else 0

if __name__ == '__main__':
    sys.exit(main())
