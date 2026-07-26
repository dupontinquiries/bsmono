#!/usr/bin/env python3
"""shape_baseline.py -- is (blob -> detail) learnable? Answer before training.

A GAN can take hours on a GPU and will always produce SOMETHING, so it is a poor
instrument for deciding whether the task is well posed. Nearest-neighbour
retrieval needs no training and answers the prior question: do similar blobs have
similar details? If they do not, the encoding or the pairing is wrong and no
architecture will rescue it.

Reports, on a held-out split:
  IDENTITY   error if you just return the blob unchanged (the do-nothing baseline)
  MEAN       error if you return the average detail (the ignore-input baseline)
  RETRIEVAL  error using the nearest training blob's detail

RETRIEVAL must beat both. Beating MEAN shows the input carries usable signal;
beating IDENTITY shows the mapping adds something rather than copying. A GAN that
cannot beat RETRIEVAL is not earning its compute.

Usage:
  python3 tools/shape_baseline.py --pairs pairs/pairs.npz
  python3 tools/shape_baseline.py --pairs pairs/pairs.npz --k 5 --holdout 0.2
"""
import sys, os, json, argparse

def descriptor(X, blocks=8):
    """Coarse pooled signature of a depth map -- what 'similar blob' means here.

    Full 32k-pixel comparison is dominated by fine detail, which is exactly what
    the blob does not have; pooling to a coarse grid compares gross shape."""
    import numpy as np
    n, h, w = X.shape
    bh, bw = h // blocks, w // blocks
    D = X[:, :bh * blocks, :bw * blocks].reshape(n, blocks, bh, blocks, bw)
    return D.mean(axis=(2, 4)).reshape(n, -1)

def main():
    import numpy as np
    ap = argparse.ArgumentParser()
    ap.add_argument("--pairs", required=True)
    ap.add_argument("--k", type=int, default=1)
    ap.add_argument("--holdout", type=float, default=0.2)
    ap.add_argument("--seed", type=int, default=0)
    a = ap.parse_args()

    d = np.load(a.pairs, allow_pickle=True)
    B, D = d["blob"].astype("f4"), d["detail"].astype("f4")
    meta = json.loads(str(d["meta"])) if "meta" in d else [{}] * len(B)
    n = len(B)
    rng = np.random.default_rng(a.seed)
    idx = rng.permutation(n)
    n_te = max(1, int(n * a.holdout))
    te, tr = idx[:n_te], idx[n_te:]
    print(f"{n} pairs -> {len(tr)} train, {len(te)} test")

    # split by MODEL as well, so we can see whether it generalises across donors
    models = np.array([m.get("model", "?") for m in meta])
    uniq = sorted(set(models))
    print(f"donors: {len(uniq)}")

    def err(P, T):
        return float(np.abs(P - T).mean())

    e_ident = err(B[te], D[te])
    mean_detail = D[tr].mean(axis=0, keepdims=True)
    e_mean = err(np.repeat(mean_detail, len(te), 0), D[te])

    Ftr, Fte = descriptor(B[tr]), descriptor(B[te])
    Ftr /= (np.linalg.norm(Ftr, axis=1, keepdims=True) + 1e-6)
    Fte /= (np.linalg.norm(Fte, axis=1, keepdims=True) + 1e-6)
    sims = Fte @ Ftr.T
    order = np.argsort(-sims, axis=1)[:, :a.k]
    pred = D[tr][order].mean(axis=1)
    e_retr = err(pred, D[te])

    print("\n  %-12s %10s  %s" % ("baseline", "L1 error", "meaning"))
    print("  %-12s %10.4f  %s" % ("IDENTITY", e_ident, "return the blob unchanged"))
    print("  %-12s %10.4f  %s" % ("MEAN", e_mean, "ignore the input entirely"))
    print("  %-12s %10.4f  %s" % ("RETRIEVAL", e_retr, f"nearest {a.k} training blob(s)"))

    beats_mean = e_retr < e_mean
    beats_ident = e_retr < e_ident
    print("\n  retrieval vs mean     : %+.1f%%  %s"
          % (100 * (e_retr / e_mean - 1), "better" if beats_mean else "WORSE"))
    print("  retrieval vs identity : %+.1f%%  %s"
          % (100 * (e_retr / e_ident - 1), "better" if beats_ident else "WORSE"))

    # cross-donor: does a blob retrieve detail from its OWN model or another?
    same = 0
    for i, t in enumerate(te):
        src = models[tr][order[i, 0]]
        if src == models[t]: same += 1
    print("\n  nearest neighbour came from the SAME donor: %.0f%%" % (100 * same / len(te)))
    if same / len(te) > 0.9:
        print("    -> retrieval is mostly matching within a model. Cross-model")
        print("       transfer is unproven; more diverse donors would test it.")

    # LEAVE-ONE-DONOR-OUT: the question that actually matters is whether detail
    # transfers to a model the system has never seen. Random splits flatter it,
    # because a held-out component usually has a near-twin in the same model.
    print("\n  leave-one-donor-out (train on the others, test on this one):")
    print("    %-34s %9s %9s %9s" % ("held-out donor", "identity", "retrieval", "gain"))
    gains = []
    for u in uniq:
        te2 = np.where(models == u)[0]
        tr2 = np.where(models != u)[0]
        if len(te2) < 20 or len(tr2) < 50: continue
        Ftr2, Fte2 = descriptor(B[tr2]), descriptor(B[te2])
        Ftr2 /= (np.linalg.norm(Ftr2, axis=1, keepdims=True) + 1e-6)
        Fte2 /= (np.linalg.norm(Fte2, axis=1, keepdims=True) + 1e-6)
        o2 = np.argsort(-(Fte2 @ Ftr2.T), axis=1)[:, :a.k]
        e_i = err(B[te2], D[te2])
        e_r = err(D[tr2][o2].mean(axis=1), D[te2])
        g = 100 * (e_r / e_i - 1)
        gains.append(g)
        print("    %-34s %9.4f %9.4f %8.0f%%" % (u[:34], e_i, e_r, g))
    if gains:
        avg = sum(gains) / len(gains)
        print("    %-34s %9s %9s %8.0f%%  (mean)" % ("", "", "", avg))
        if avg < -20:
            print("    -> detail DOES transfer across models, not just within one.")
        elif avg < 0:
            print("    -> transfers weakly; more diverse donors would likely help.")
        else:
            print("    -> does NOT transfer to unseen models. Retrieval is memorising;")
            print("       a GAN on this data would do the same.")

    print()
    if beats_mean and beats_ident:
        print("  VERDICT: the mapping carries signal. Retrieval is the baseline a")
        print("           trained model has to beat -- and it needs no GPU.")
    elif beats_mean:
        print("  VERDICT: input carries signal, but retrieval does not beat simply")
        print("           returning the blob. The detail gap may be too fine for")
        print("           this encoding to represent.")
    else:
        print("  VERDICT: retrieval fails to beat ignoring the input. Something is")
        print("           wrong with the encoding or the pairing -- do NOT train on")
        print("           this data until that is understood.")

if __name__ == "__main__":
    main()
