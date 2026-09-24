"""decide.py -- decide a NEW vehicle's parts by nearest known part.

The generalisation experiment's winner: across held-out vehicles, copying the
decision of the most similar known part (geometry only) placed parts better
than any network trained on three vehicles. So a new vehicle's parts are
decided that way, and each decision carries WHICH part it came from, so it
can be read and overruled.
"""
import json, os, sys
import numpy as np
sys.path.insert(0, os.path.dirname(__file__))
from partnet import part_features, canon_bone, VEH

HERE = os.path.dirname(os.path.abspath(__file__))


def library():
    rows = []
    for v in VEH:
        d = json.load(open(os.path.join(HERE, 'data', f'data_{v}.json')))
        for s in d['stat']:
            f, side = part_features(s, d['lo'], d['hi'])
            rows.append(dict(f=f, side=side, veh=v, g=s['g'], fate=s['fate'], bone=canon_bone(d['bones'][s['bone']]),
                             seat=list(s['seat']), scale=s['scale'], dis=list(s['dis']),
                             mo={1: 'spike', 2: 'trim'}.get(s['morph'])))
    return rows


def decide(groups):
    """groups: {name: {'v': vertices, 'f': faces}} in car coordinates."""
    lib = library()
    F = np.array([r['f'] for r in lib]); mu, sd = F.mean(0), F.std(0) + 1e-6
    lo = np.min([np.min(np.array(g['v']), 0) for g in groups.values()], 0)
    hi = np.max([np.max(np.array(g['v']), 0) for g in groups.values()], 0)
    out = {}
    dl = np.clip((F - mu) / sd, -3, 3)
    # SEATS ARE EXCLUSIVE. Parts decided independently all copied the same
    # thigh-trim source, stacking two trims on one seat. The part with the
    # closest match claims its source first; a later part on the same side
    # falls to its next-nearest free one. Processed in order of match
    # distance, so the result does not depend on the order parts are named.
    cand = {}
    for name, g in groups.items():
        V = np.array(g['v']); c = (V.min(0) + V.max(0)) / 2; r = (V.max(0) - V.min(0)) / 2
        f, side = part_features({'c': c, 'r': r, 'nt': len(g['f'])}, lo, hi)
        dz = np.clip((f - mu) / sd, -3, 3); d = ((dl - dz) ** 2).sum(1)
        cand[name] = (side, np.argsort(d, kind='stable'), d)
    SIDED = ('clav', 'uparm', 'forearm', 'hand', 'hip', 'thigh', 'shin', 'foot')

    def resulting_seat(src, side):
        seat = list(src['seat'])
        if src['side'] * (side or 1) < 0:
            seat[2] = -seat[2]
        bone = src['bone'] + ('_l' if side < 0 else '_r') if src['bone'] in SIDED else src['bone']
        return bone, np.array(seat)
    # what must be exclusive is the RESULTING SEAT (bone + where on it), not
    # the source part: the flyer has two rear quarters, and after mirroring,
    # both landed on one thigh at one spot
    taken = []
    for name in sorted(cand, key=lambda n: (cand[n][2][cand[n][1][0]], n)):
        side, order, d = cand[name]
        for i in order:
            b_, st = resulting_seat(lib[int(i)], side)
            if lib[int(i)]['fate'] == 0 or not any(b_ == tb and np.linalg.norm(st - ts) < 25.0 for tb, ts in taken):
                k = int(i); break
        src = lib[k]
        if src['fate']:
            taken.append(resulting_seat(src, side))
        seat = list(src['seat'])
        # the source's seat is on ITS side; mirror it onto this part's side
        if src['side'] * (side or 1) < 0:
            seat[2] = -seat[2]
        bone = src['bone'] + ('_l' if side < 0 else '_r') if src['bone'] in ('clav', 'uparm', 'forearm', 'hand', 'hip', 'thigh', 'shin', 'foot') else src['bone']
        out[name] = dict(b=bone, o=[round(x, 2) for x in seat], s=round(src['scale'], 4),
                         f='armour' if src['fate'] else 'stow', dis=src['dis'], mo=src['mo'],
                         src=f"{src['veh']}:{src['g']}")
    return out


if __name__ == '__main__':
    sys.path.insert(0, os.path.join(HERE, '..', 'vehicles'))
    import importlib
    D = importlib.import_module(sys.argv[1]).groups()
    dec = decide(D)
    json.dump(dec, open(sys.argv[2], 'w'), indent=1)
    for g, d in sorted(dec.items()):
        print(f"  {g:<10} {d['f']:<7}{(' '+d['mo']) if d['mo'] else '':<7} -> {d['b']:<10} scale {d['s']:.2f}   (copied from {d['src']})")
