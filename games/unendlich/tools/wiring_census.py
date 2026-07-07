#!/usr/bin/env python3
"""wiring_census.py — which engine modules are live in the fullgame bin?

Static reachability from unendlich_fullgame.rs over intra-crate path references,
PLUS linkme inversion: if the bin reaches a registry (sim::service_registry /
sim::kernel_registry), every module that REGISTERS into it (distributed_slice)
is live too — registration points at the registry, not the other way around,
which a naive census misses.
"""
import re, os, collections, sys
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SRC = os.path.join(ROOT, 'crates/unendlich-engine/src')
mods = {}
for dp, dn, fns in os.walk(SRC):
    if 'bin' in dp.split(os.sep):
        continue
    for f in fns:
        if not f.endswith('.rs'):
            continue
        p = os.path.join(dp, f)
        parts = os.path.relpath(p, SRC)[:-3].split(os.sep)
        if parts[-1] == 'mod':
            parts = parts[:-1]
        if parts and parts[-1] == 'lib':
            continue
        mods['::'.join(parts)] = p

def refs(text):
    out = set()
    for m in re.finditer(r'(?:crate|unendlich_engine)::([A-Za-z0-9_:]+)', text):
        out.add(m.group(1))
    for m in re.finditer(r'\buse (?:crate|super|self)::([A-Za-z0-9_:]+)', text):
        out.add(m.group(1))
    return out

def match_mods(refset):
    hit = set()
    for r in refset:
        segs = r.split('::')
        for i in range(len(segs), 0, -1):
            cand = '::'.join(segs[:i])
            if cand in mods:
                hit.add(cand)
                break
    return hit

text = {k: open(v, encoding='utf-8', errors='replace').read() for k, v in mods.items()}
bintext = open(os.path.join(SRC, 'bin/unendlich_fullgame.rs')).read()
reach = match_mods(refs(bintext))
frontier = list(reach)
registrars = [m for m, t in text.items()
              if 'distributed_slice(SERVICES)' in t or 'distributed_slice(KERNELS)' in t]
while frontier:
    m = frontier.pop()
    nxt = match_mods(refs(text.get(m, '')))
    # linkme inversion
    if m in ('sim::service_registry', 'sim::kernel_registry'):
        nxt |= set(registrars)
    for h in nxt:
        if h not in reach:
            reach.add(h)
            frontier.append(h)
orph = sorted(m for m in mods if m not in reach)
print(f'total modules: {len(mods)}  live in bin: {len(reach)}  orphans: {len(orph)}')
top = collections.Counter(m.split('::')[0] for m in orph)
print('orphans by tree:', top.most_common(8))
if '--list' in sys.argv:
    print('\n'.join(orph))
