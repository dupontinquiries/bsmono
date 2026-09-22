"""partnet -- can a network learn how a vehicle's parts evolve into a robot?

Data: harvested from the three viewer pages' own frames (tools/harvest.js):
orion, the Falcon GT and the flyer. Parts are described ONLY by geometry --
where they sit in their vehicle, how big and what shape -- never by name, so
whatever is learned could transfer to a vehicle with parts no one named.

Two models, to test where the division of labour should sit:

  end-to-end   part + animation + params + time  ->  where the part is
               (relative to the pelvis), and whether it shows / dissolves
  structured   part  ->  the DECISION only: keep or dissolve, which bone,
               where on it, how big, when it dissolves. The rig then
               animates it, for any motion.

Everything is scored LEAVE-ONE-VEHICLE-OUT (train on two, test on the third),
against baselines -- a number without a baseline means nothing, and with 90
parts from 3 vehicles, fitting the training set proves nothing at all.

Plain numpy: a small MLP with Adam, deterministic, the loop in view.
"""
import json, sys
import numpy as np

VEH = ['orion', 'falcon', 'flyer']
ANIMS = ['transform_ground', 'transform_air', 'idle', 'walk', 'run', 'jump', 'dash', 'punch']


# ── features: geometry only ──────────────────────────────────────────────
def side_of(c, half_w):
    """-1 left, +1 right, 0 on the centreline (within 8% of the half-width)."""
    return 0.0 if abs(c[2]) < 0.08 * half_w else float(np.sign(c[2]))


def part_features(s, lo, hi):
    lo, hi = np.array(lo), np.array(hi)
    ctr, half = (lo + hi) / 2, (hi - lo) / 2
    c, r = np.array(s['c']), np.array(s['r'])
    side = side_of(c, half[2])
    rel = (c - ctr) / half
    rel[2] = abs(rel[2])                     # mirrored parts share what they learn
    ext = np.sort(r)[::-1] / half[0]         # long, mid, thin -- orientation-free
    return np.concatenate([
        rel,                                 # where in the vehicle (3)
        r / half[0],                         # extents along the vehicle's axes (3)
        ext,                                 # sorted extents (3)
        [ext[2] / max(ext[0], 1e-6), ext[1] / max(ext[0], 1e-6)],   # flatness, elongation
        [np.log1p(s['nt']) / 10.0],          # detail
        [abs(side)],                         # sided or central
        # NOT the vehicle's own proportions: constant per vehicle, so with two
        # training vehicles they were a vehicle ID -- the network memorised it
        # and the flyer (wider than long) landed ~20 sd outside and blew up
    ]), side


def canon_bone(name):
    return name[:-2] if name.endswith(('_l', '_r')) else name


def load():
    D = {v: json.load(open(__import__('os').path.join(__import__('os').path.dirname(__file__), 'data', f'data_{v}.json'))) for v in VEH}
    bone_classes = sorted({canon_bone(b) for v in VEH for b in D[v]['bones']})
    return D, bone_classes


# ── the model: a small MLP, Adam, in plain numpy ─────────────────────────
class MLP:
    def __init__(self, sizes, seed=0):
        rng = np.random.default_rng(seed)
        self.W = [rng.normal(0, np.sqrt(2 / a), (a, b)) for a, b in zip(sizes[:-1], sizes[1:])]
        self.b = [np.zeros(b) for b in sizes[1:]]
        self.m = [np.zeros_like(p) for p in self.W + self.b]
        self.v = [np.zeros_like(p) for p in self.W + self.b]
        self.k = 0

    def forward(self, X):
        self.h = [X]
        for i, (W, b) in enumerate(zip(self.W, self.b)):
            z = self.h[-1] @ W + b
            self.h.append(np.maximum(z, 0) if i < len(self.W) - 1 else z)
        return self.h[-1]

    def step(self, grad_out, lr=3e-3, wd=3e-3):
        gW, gb, g = [], [], grad_out
        for i in reversed(range(len(self.W))):
            gW.insert(0, self.h[i].T @ g / len(g) + wd * self.W[i]); gb.insert(0, g.mean(0))
            if i:
                g = (g @ self.W[i].T) * (self.h[i] > 0)
        self.k += 1
        for j, (p, gr) in enumerate(zip(self.W + self.b, gW + gb)):
            self.m[j] = 0.9 * self.m[j] + 0.1 * gr
            self.v[j] = 0.999 * self.v[j] + 0.001 * gr * gr
            mh, vh = self.m[j] / (1 - 0.9 ** self.k), self.v[j] / (1 - 0.999 ** self.k)
            p -= lr * mh / (np.sqrt(vh) + 1e-8)


def train(model, X, lossgrad, epochs, bs=256, seed=0, log=None):
    rng = np.random.default_rng(seed)
    for ep in range(epochs):
        idx = rng.permutation(len(X))
        tot = 0.0
        for i in range(0, len(X), bs):
            j = idx[i:i + bs]
            # feature noise: with 2 training vehicles the net will memorise
            # anything it can; jitter makes it learn trends instead
            out = model.forward(X[j] + rng.normal(0, 0.15, X[j].shape))
            L, g = lossgrad(out, j)
            model.step(g); tot += L * len(j)
        if log and (ep % max(1, epochs // 4) == 0 or ep == epochs - 1):
            log(f'      epoch {ep:>4}  loss {tot / len(X):.4f}')
    return model


def standardise(Xtr, Xte):
    # CLIPPED: a new vehicle's shape must not push the net into extrapolation
    mu, sd = Xtr.mean(0), Xtr.std(0) + 1e-6
    return np.clip((Xtr - mu) / sd, -3, 3), np.clip((Xte - mu) / sd, -3, 3)


# ── structured: part -> decision ─────────────────────────────────────────
def structured_rows(D, bone_classes, vehs):
    X, Y = [], []
    for v in vehs:
        d = D[v]
        for s in d['stat']:
            f, side = part_features(s, d['lo'], d['hi'])
            seat = np.array(s['seat'], float)
            if side < 0:
                seat[2] = -seat[2]           # canonical side
            Y.append(dict(fate=s['fate'], bone=bone_classes.index(canon_bone(d['bones'][s['bone']])),
                          seat=seat / 100.0, scale=s['scale'], dis=np.array(s['dis']), side=side,
                          veh=v, g=s['g']))
            X.append(f)
    return np.array(X), Y


def fit_structured(Xtr, Ytr, nb, log):
    """One trunk, several heads: fate (1 logit), bone (nb logits), seat (3),
    scale (1), dissolve window (2). The seat and scale only matter for parts
    that are kept, so their loss is masked to armour."""
    fate = np.array([y['fate'] for y in Ytr], float)
    bone = np.array([y['bone'] for y in Ytr])
    seat = np.array([y['seat'] for y in Ytr]); scale = np.array([y['scale'] for y in Ytr])
    dis = np.array([y['dis'] for y in Ytr])
    net = MLP([Xtr.shape[1], 24, 1 + nb + 3 + 1 + 2])

    def lg(out, j):
        g = np.zeros_like(out)
        p = 1 / (1 + np.exp(-out[:, 0])); g[:, 0] = p - fate[j]
        lf = -np.mean(fate[j] * np.log(p + 1e-9) + (1 - fate[j]) * np.log(1 - p + 1e-9))
        z = out[:, 1:1 + nb]; z = z - z.max(1, keepdims=True); e = np.exp(z); sm = e / e.sum(1, keepdims=True)
        oh = np.eye(nb)[bone[j]]; g[:, 1:1 + nb] = sm - oh
        lb = -np.mean(np.log((sm * oh).sum(1) + 1e-9))
        a = fate[j][:, None]
        d_seat = (out[:, 1 + nb:4 + nb] - seat[j]) * a; g[:, 1 + nb:4 + nb] = 2 * d_seat
        d_sc = (out[:, 4 + nb] - scale[j]) * a[:, 0]; g[:, 4 + nb] = 2 * d_sc
        d_dis = out[:, 5 + nb:7 + nb] - dis[j]; g[:, 5 + nb:7 + nb] = 2 * d_dis
        return lf + lb + (d_seat ** 2).mean() + (d_sc ** 2).mean() + (d_dis ** 2).mean(), g
    return train(net, Xtr, lg, 600, bs=64, log=None)


# ── end-to-end: part + animation + params + time -> where it is ─────────
def time_code(anim, t):
    if anim.startswith('transform'):
        return [t, np.sin(np.pi * t), np.cos(np.pi * t), 0, 0]
    return [1.0, 0, 0, np.sin(2 * np.pi * t / 1.6), np.cos(2 * np.pi * t / 1.6)]


def e2e_rows(D, vehs):
    X, Y, M, meta = [], [], [], []
    for v in vehs:
        d = D[v]
        feats = [part_features(s, d['lo'], d['hi']) for s in d['stat']]
        for fr in d['frames']:
            a = np.eye(len(ANIMS))[ANIMS.index(fr['anim'])]
            tc = time_code(fr['anim'], fr['t'])
            for i, s in enumerate(d['stat']):
                f, side = feats[i]
                vis, vox, x, y, z = fr['rows'][i]
                if side < 0:
                    z = -z
                X.append(np.concatenate([f, a, tc, fr['params']]))
                Y.append([vis, vox, x / 100, y / 100, z / 100])
                M.append(s['fate'])                  # positions only for kept parts
                meta.append((v, fr['anim'], fr['t'], i, side))
    return np.array(X), np.array(Y), np.array(M, float), meta


def fit_e2e(Xtr, Ytr, Mtr, log):
    net = MLP([Xtr.shape[1], 48, 48, 5])

    def lg(out, j):
        g = np.zeros_like(out)
        p = 1 / (1 + np.exp(-out[:, 0])); g[:, 0] = p - Ytr[j, 0]
        lv = -np.mean(Ytr[j, 0] * np.log(p + 1e-9) + (1 - Ytr[j, 0]) * np.log(1 - p + 1e-9))
        d1 = out[:, 1] - Ytr[j, 1]; g[:, 1] = 2 * d1
        m = Mtr[j][:, None]; dp = (out[:, 2:5] - Ytr[j, 2:5]) * m; g[:, 2:5] = 2 * dp
        return lv + (d1 ** 2).mean() + (dp ** 2).mean(), g
    return train(net, Xtr, lg, 40, bs=256, log=None)


# ── the experiment ───────────────────────────────────────────────────────
def main():
    D, bone_classes = load()
    nb = len(bone_classes)
    log = print
    print(f'{sum(len(D[v]["stat"]) for v in VEH)} parts, {sum(s["fate"] for v in VEH for s in D[v]["stat"])} kept as armour; '
          f'{nb} bone classes (left/right folded)\n')
    S = {'fate_acc': [], 'bone_acc': [], 'seat_err': [], 'rig_pos_err': [],
         'b_fate': [], 'b_bone': [], 'b_seat': [], 'b_rig': [], 'nn_bone': [], 'nn_rig': [],
         'e2e_pos': [], 'e2e_vis': [], 'b_e2e_pos': [], 'nn_e2e_pos': []}
    for test in VEH:
        train_v = [v for v in VEH if v != test]
        print(f'── held out: {test}   (trained on {", ".join(train_v)})')
        # structured
        Xtr, Ytr = structured_rows(D, bone_classes, train_v)
        Xte, Yte = structured_rows(D, bone_classes, [test])
        Xtr_s, Xte_s = standardise(Xtr, Xte)
        net = fit_structured(Xtr_s, Ytr, nb, log)
        out = net.forward(Xte_s)
        fate_p = out[:, 0] > 0
        bone_p = out[:, 1:1 + nb].argmax(1)
        seat_p = out[:, 1 + nb:4 + nb]
        fate_t = np.array([y['fate'] for y in Yte]); bone_t = np.array([y['bone'] for y in Yte])
        arm = fate_t == 1
        S['fate_acc'].append((fate_p == fate_t).mean())
        S['bone_acc'].append((bone_p[arm] == bone_t[arm]).mean())
        seat_t = np.array([y['seat'] for y in Yte])
        S['seat_err'].append(np.linalg.norm((seat_p - seat_t)[arm], axis=1).mean() * 100)
        # baselines: the majority / the mean, and the NEAREST training part by features
        maj_f = round(np.mean([y['fate'] for y in Ytr]))
        S['b_fate'].append((fate_t == maj_f).mean())
        arm_tr = [y for y in Ytr if y['fate']]
        maj_b = np.bincount([y['bone'] for y in arm_tr]).argmax()
        S['b_bone'].append((bone_t[arm] == maj_b).mean())
        mean_seat = np.mean([y['seat'] for y in arm_tr], 0)
        S['b_seat'].append(np.linalg.norm(seat_t[arm] - mean_seat, axis=1).mean() * 100)
        near = np.argmin(((Xte_s[:, None, :] - Xtr_s[None, :, :]) ** 2).sum(2) +
                         1e9 * (np.array([y['fate'] for y in Ytr])[None, :] == 0), 1)
        S['nn_bone'].append((np.array([Ytr[k]['bone'] for k in near])[arm] == bone_t[arm]).mean())

        # THROUGH THE RIG: the predicted bone and seat, placed on the rig's
        # own bone frames in every robot-form frame, against where the page drew it
        d = D[test]
        def place(bone_cls, seat, side, fr):
            base = bone_classes[bone_cls]
            name = base + ('_l' if side < 0 else '_r') if base + '_l' in fr['bones'] else base
            if name not in fr['bones']:
                return None
            B = fr['bones'][name]; h, m = np.array(B[:3]), np.array(B[3:]).reshape(3, 3)
            o = seat.copy() * 100
            if side < 0:
                o[2] = -o[2]
            return h + m @ o
        errs, berrs, nerrs = [], [], []
        motion = [fr for fr in d['frames'] if not fr['anim'].startswith('transform')]
        for i, y in enumerate(Yte):
            if not y['fate']:
                continue
            for fr in motion:
                truth = np.array(fr['rows'][i][2:5])
                pp = place(bone_p[i], seat_p[i], y['side'], fr)
                bp = place(maj_b, mean_seat, y['side'], fr)
                npp = place(Ytr[near[i]]['bone'], Ytr[near[i]]['seat'], y['side'], fr)
                if pp is not None: errs.append(np.linalg.norm(pp - truth))
                if bp is not None: berrs.append(np.linalg.norm(bp - truth))
                if npp is not None: nerrs.append(np.linalg.norm(npp - truth))
        S['rig_pos_err'].append(np.mean(errs)); S['b_rig'].append(np.mean(berrs)); S['nn_rig'].append(np.mean(nerrs))

        # end-to-end
        Xtr2, Ytr2, Mtr2, _ = e2e_rows(D, train_v)
        Xte2, Yte2, Mte2, meta2 = e2e_rows(D, [test])
        Xtr2_s, Xte2_s = standardise(Xtr2, Xte2)
        net2 = fit_e2e(Xtr2_s, Ytr2, Mtr2, log)
        o2 = net2.forward(Xte2_s)
        mot = np.array([not m[1].startswith('transform') for m in meta2])
        sel = (Mte2 == 1) & mot
        S['e2e_pos'].append(np.linalg.norm((o2[sel, 2:5] - Yte2[sel, 2:5]), axis=1).mean() * 100)
        S['e2e_vis'].append(((o2[:, 0] > 0) == (Yte2[:, 0] > 0.5)).mean())
        # baseline: the average kept part's position at that animation and time
        key = lambda m: (m[1], round(m[2], 3))
        avg = {}
        for (mm, yy, kk) in zip(e2e_rows(D, train_v)[3], Ytr2, Mtr2):
            if kk:
                avg.setdefault(key(mm), []).append(yy[2:5])
        avg = {k: np.mean(v, 0) for k, v in avg.items()}
        S['b_e2e_pos'].append(np.mean([np.linalg.norm(avg[key(meta2[q])] - Yte2[q, 2:5]) * 100
                                       for q in np.where(sel)[0] if key(meta2[q]) in avg]))
        print(f'   structured: fate {S["fate_acc"][-1]:.0%} (baseline {S["b_fate"][-1]:.0%})   '
              f'bone {S["bone_acc"][-1]:.0%} (majority {S["b_bone"][-1]:.0%}, nearest part {S["nn_bone"][-1]:.0%})   '
              f'seat err {S["seat_err"][-1]:.0f} u (mean seat {S["b_seat"][-1]:.0f})')
        print(f'   through the rig, robot-form frames: position error {S["rig_pos_err"][-1]:.0f} u '
              f'(majority bone + mean seat {S["b_rig"][-1]:.0f}, nearest part {S["nn_rig"][-1]:.0f})')
        print(f'   end-to-end:  position error {S["e2e_pos"][-1]:.0f} u (average trajectory {S["b_e2e_pos"][-1]:.0f})   '
              f'shown/hidden {S["e2e_vis"][-1]:.0%}\n')
    print('── mean over the three held-out vehicles')
    m = {k: float(np.mean(v)) for k, v in S.items()}
    print(f'   keep-or-dissolve   {m["fate_acc"]:.0%}   vs majority {m["b_fate"]:.0%}')
    print(f'   which bone         {m["bone_acc"]:.0%}   vs majority {m["b_bone"]:.0%}, nearest part {m["nn_bone"]:.0%}')
    print(f'   position (robot form, units ~ cm; robot ~390 tall):')
    print(f'      structured + rig   {m["rig_pos_err"]:.0f}   vs majority+mean {m["b_rig"]:.0f}, nearest part {m["nn_rig"]:.0f}')
    print(f'      end-to-end         {m["e2e_pos"]:.0f}   vs average trajectory {m["b_e2e_pos"]:.0f}')
    json.dump(m, open(__import__('os').path.join(__import__('os').path.dirname(__file__), 'results.json'), 'w'), indent=1)


if __name__ == '__main__':
    main()
