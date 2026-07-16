"""
kBM3 population-based coevolutionary training.
================================================
A single Generator vs. single Discriminator can get stuck: oscillating
equilibria, mode collapse, or (as found in gan-zai-v5.3) a genuinely dead
gradient region that neither side can escape. This trains a POPULATION of
generators and discriminators, plays every generator against every
discriminator each round (a cross-play score matrix, not just one pairing),
and uses that matrix to select and mutate -- standard practice in
coevolutionary GAN training (e.g. Lipizzaner, E-GAN).

Why this actually helps beyond "train several independently and pick the best":
  - A generator that only fools ONE discriminator looks fine 1-on-1 but is
    exposed instantly when scored against the whole population -- so fitness
    reflects how GENERALLY good it is, not how well it exploits one opponent's
    blind spot.
  - Symmetrically, a discriminator that's only sharp against one generator
    (or has collapsed into a degenerate always-reject state) scores badly
    against the rest of the population and gets replaced, instead of quietly
    being the only discriminator in the room and killing training (the
    single-pairing failure mode this conversation already hit once).
  - Weak population members are replaced each round with a mutated copy of a
    strong one (Gaussian weight perturbation + fresh optimizer state), so the
    population keeps exploring nearby weight space rather than all converging
    to one point and getting stuck together.

Reuses the verified Config/Generator/Discriminator/OpusMelDataset/
_InfiniteSampler/_instance_noise_sigma from gan-zai-v5.3.py so the model
architecture, data pipeline, and cache format are identical -- checkpoints
from this trainer are ordinary Generator state_dicts, loadable anywhere a
gan-zai-v5.3 gen_*.pt is (the explorer GUI, server.py, etc).

Usage:
    python population_trainer.py train --data ./music --pop 4 --rounds 500 --inner-iters 20
    python population_trainer.py test    # synthetic self-check, no audio needed
"""
import argparse
import copy
import importlib.util
import os
import time
from dataclasses import dataclass

import numpy as np
import torch
import torch.nn as nn
import torch.optim as optim
from torch.utils.data import DataLoader


def _load_base(path: str):
    spec = importlib.util.spec_from_file_location("kbm3_base", path)
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    return m


# --------------------------------------------------------------------------- #
# Population member + score bookkeeping
# --------------------------------------------------------------------------- #
@dataclass
class PopConfig:
    pop_size: int = 4          # generators AND discriminators (symmetric population)
    rounds: int = 500          # coevolutionary rounds
    inner_iters: int = 20      # adversarial steps per (G_i, D_j) pairing per round
    mutate_frac: float = 0.5   # fraction of the population replaced/mutated each round
    mutation_std: float = 0.02 # std of Gaussian weight noise applied on mutation
    all_pairs: bool = True     # True: full pop x pop cross-play; False: random subset
    pairs_per_round: int = 8   # used only if all_pairs=False
    lr: float = 2e-4
    seed: int = 0


class Member:
    """One population member: a model + its optimizer + a running fitness."""
    def __init__(self, model: nn.Module, lr: float, betas):
        self.model = model
        self.opt = optim.Adam(model.parameters(), lr=lr, betas=betas)
        self.fitness = 0.0

    def clone_mutated(self, lr: float, betas, std: float):
        """A copy of this member's weights with Gaussian noise added, and a
        FRESH optimizer (stale Adam momentum from the parent isn't meaningful
        for a perturbed set of weights)."""
        new_model = copy.deepcopy(self.model)
        with torch.no_grad():
            for p in new_model.parameters():
                p.add_(torch.randn_like(p) * std)
        return Member(new_model, lr, betas)


# --------------------------------------------------------------------------- #
# One coevolutionary round
# --------------------------------------------------------------------------- #
def play_pair(base, cfg, pcfg, g_mem: Member, d_mem: Member, batches, device, step_offset: int):
    """inner_iters of ordinary adversarial training between one G and one D.
    Returns final (D_real, D_fake) probabilities -- the raw material for
    fitness. Mirrors gan-zai-v5.3's train() inner loop exactly (same losses,
    same instance-noise schedule) so results are comparable to single-pair runs."""
    criterion = nn.BCEWithLogitsLoss()
    d_real = d_fake = 0.5
    for k in range(pcfg.inner_iters):
        real = next(batches).to(device)
        b = real.size(0)
        real_label = torch.full((b,), cfg.label_smooth, device=device)
        fake_label = torch.zeros(b, device=device)
        sigma = base._instance_noise_sigma(cfg, step_offset + k)

        def noised(x):
            return x + torch.randn_like(x) * sigma if sigma > 0 else x

        g_mem.model.zero_grad(); d_mem.model.zero_grad()
        d_real_logits = d_mem.model(noised(real))
        fake = g_mem.model(torch.randn(b, cfg.latent, device=device))
        d_fake_logits = d_mem.model(noised(fake.detach()))
        lossD = criterion(d_real_logits, real_label) + criterion(d_fake_logits, fake_label)
        lossD.backward(); d_mem.opt.step()

        g_mem.model.zero_grad()
        lossG = criterion(d_mem.model(noised(fake)), real_label)
        lossG.backward(); g_mem.opt.step()

        with torch.no_grad():
            d_real = torch.sigmoid(d_real_logits).mean().item()
            d_fake = torch.sigmoid(d_fake_logits).mean().item()
    return d_real, d_fake


def run_round(base, cfg, pcfg, gs: list[Member], ds: list[Member], batches, device, step_offset: int):
    """Cross-play every G against every D (or a random subset), fill the
    score matrices, and return per-member fitness."""
    ng, nd = len(gs), len(ds)
    S_fool = np.zeros((ng, nd))     # how much G_i fooled D_j: mean D_j(fake)
    S_sep = np.zeros((ng, nd))      # how well D_j separated real/fake vs G_i: D(real)-D(fake)

    if pcfg.all_pairs:
        pairs = [(i, j) for i in range(ng) for j in range(nd)]
    else:
        rng = np.random.default_rng(pcfg.seed + step_offset)
        pairs = [(int(rng.integers(ng)), int(rng.integers(nd))) for _ in range(pcfg.pairs_per_round)]

    for i, j in pairs:
        dr, df = play_pair(base, cfg, pcfg, gs[i], ds[j], batches, device, step_offset)
        S_fool[i, j] = df
        S_sep[i, j] = dr - df

    # Fitness = average performance against the WHOLE opposing population,
    # not just one opponent -- this is what makes exploiting a single weak
    # counterpart not pay off.
    covered_g = (S_fool.sum(axis=1) != 0) | (pcfg.all_pairs)
    covered_d = (S_sep.sum(axis=0) != 0) | (pcfg.all_pairs)
    fit_g = S_fool.mean(axis=1)     # higher = fools discriminators more broadly
    fit_d = S_sep.mean(axis=0)      # higher = separates real/fake more broadly
    return fit_g, fit_d, S_fool, S_sep


def select_and_mutate(members: list[Member], fitness: np.ndarray, pcfg: PopConfig, betas):
    """Keep the top (1-mutate_frac) by fitness; replace the rest with a
    mutated clone of a randomly-chosen SURVIVOR (not always the single best,
    so the population doesn't collapse onto one point)."""
    n = len(members)
    n_replace = max(1, int(round(n * pcfg.mutate_frac)))
    order = np.argsort(-fitness)  # best first
    survivors_idx = order[: n - n_replace]
    replace_idx = order[n - n_replace:]

    rng = np.random.default_rng()
    for ri in replace_idx:
        parent_idx = survivors_idx[rng.integers(len(survivors_idx))]
        members[ri] = members[parent_idx].clone_mutated(pcfg.lr, betas, pcfg.mutation_std)
    return members, survivors_idx, replace_idx


# --------------------------------------------------------------------------- #
# Training driver
# --------------------------------------------------------------------------- #
def train(base_path: str, data_dir: str, pcfg: PopConfig, cfg_overrides: dict,
          ckpt_dir: str = "pop_checkpoints", workers: int = 4, device=None):
    base = _load_base(base_path)
    cfg = base.Config(data_dir=data_dir, **cfg_overrides)
    device = device or torch.device("cuda" if torch.cuda.is_available() else "cpu")
    os.makedirs(ckpt_dir, exist_ok=True)

    torch.manual_seed(pcfg.seed)
    dataset = base.OpusMelDataset(cfg)
    if len(dataset) < 4:
        raise SystemExit(f"Only {len(dataset)} tracks; need a real cache to train on.")
    dataloader = DataLoader(dataset, batch_size=8,
                            sampler=base._InfiniteSampler(len(dataset)),
                            drop_last=True, num_workers=workers,
                            persistent_workers=workers > 0)
    batches = iter(dataloader)
    betas = (cfg.beta_1, 0.999)

    gs = [Member(base.Generator(cfg).to(device), pcfg.lr, betas) for _ in range(pcfg.pop_size)]
    ds = [Member(base.Discriminator(cfg).to(device), pcfg.lr, betas) for _ in range(pcfg.pop_size)]

    csv_path = os.path.join(ckpt_dir, "population_log.csv")
    if not os.path.exists(csv_path):
        with open(csv_path, "w") as f:
            f.write("round,best_g_fitness,mean_g_fitness,best_d_fitness,mean_d_fitness,ms\n")

    print(f"Population training on {device}: {pcfg.pop_size} G x {pcfg.pop_size} D, "
          f"{pcfg.rounds} rounds x {pcfg.inner_iters} inner iters "
          f"({'all-pairs' if pcfg.all_pairs else f'{pcfg.pairs_per_round} random pairs'}/round).")

    for r in range(pcfg.rounds):
        t0 = time.time()
        step_offset = r * pcfg.inner_iters
        fit_g, fit_d, S_fool, S_sep = run_round(base, cfg, pcfg, gs, ds, batches, device, step_offset)
        for m, f in zip(gs, fit_g): m.fitness = f
        for m, f in zip(ds, fit_d): m.fitness = f

        gs, g_survivors, g_replaced = select_and_mutate(gs, fit_g, pcfg, betas)
        ds, d_survivors, d_replaced = select_and_mutate(ds, fit_d, pcfg, betas)

        ms = (time.time() - t0) * 1000
        best_g, mean_g = fit_g.max(), fit_g.mean()
        best_d, mean_d = fit_d.max(), fit_d.mean()
        print(f"[round {r}/{pcfg.rounds}] G fitness best={best_g:.3f} mean={mean_g:.3f} | "
              f"D fitness best={best_d:.3f} mean={mean_d:.3f} | "
              f"replaced G{list(g_replaced)} D{list(d_replaced)} ({ms:.0f}ms)")
        with open(csv_path, "a") as f:
            f.write(f"{r},{best_g:.4f},{mean_g:.4f},{best_d:.4f},{mean_d:.4f},{ms:.1f}\n")

        if r % 20 == 0 or r == pcfg.rounds - 1:
            best_idx = int(np.argmax(fit_g))
            torch.save(gs[best_idx].model.state_dict(),
                       os.path.join(ckpt_dir, f"gen_best_round{r}.pt"))

    best_idx = int(np.argmax([m.fitness for m in gs]))
    final_path = os.path.join(ckpt_dir, "gen_best_final.pt")
    torch.save(gs[best_idx].model.state_dict(), final_path)
    print(f"Done. Best generator (fitness {gs[best_idx].fitness:.3f}) saved to {final_path}.")
    return gs, ds


# --------------------------------------------------------------------------- #
def _selftest(base_path: str = "gan-zai-v5.3.py"):
    """Synthetic self-check: a tiny cache + tiny population + a few rounds,
    verifying the score matrix, selection, mutation, and checkpointing all
    run correctly without needing real audio."""
    import json
    import sqlite3
    import tempfile
    import zlib

    base = _load_base(base_path)
    base.librosa.load = lambda path, sr=None, mono=True: (np.zeros(200000, np.float32), sr)
    base.librosa.feature.melspectrogram = lambda y, sr, n_fft, hop_length, n_mels, **k: \
        np.ones((n_mels, 700), np.float32)
    base.librosa.power_to_db = lambda mel, ref=1.0: mel * 0.0

    root = tempfile.mkdtemp()
    data, cache = os.path.join(root, "music"), os.path.join(root, "cache")
    os.makedirs(data)
    for i in range(24):
        open(os.path.join(data, f"{i}.opus"), "w").close()

    pcfg = PopConfig(pop_size=3, rounds=6, inner_iters=4, mutate_frac=0.34,
                     mutation_std=0.02, all_pairs=True, lr=2e-4, seed=0)
    overrides = dict(cache_dir=cache, n_mels=16, time_frames=16,
                     g_filters=16, d_filters=8, latent=8)

    gs, ds = train(base_path, data, pcfg, overrides,
                   ckpt_dir=os.path.join(root, "pop_ck"), workers=0,
                   device=torch.device("cpu"))

    assert len(gs) == pcfg.pop_size and len(ds) == pcfg.pop_size
    assert os.path.exists(os.path.join(root, "pop_ck", "gen_best_final.pt"))
    assert os.path.exists(os.path.join(root, "pop_ck", "population_log.csv"))

    # confirm a saved checkpoint is a plain Generator state_dict, loadable
    # exactly like a gan-zai-v5.3 gen_*.pt (drop-in with the explorer/server).
    cfg = base.Config(**overrides)
    g = base.Generator(cfg)
    g.load_state_dict(torch.load(os.path.join(root, "pop_ck", "gen_best_final.pt")))
    with torch.no_grad():
        out = g(torch.randn(2, cfg.latent))
    assert tuple(out.shape) == (2, 1, cfg.n_mels, cfg.time_frames)

    print("\nSELF-TEST PASSED: population trains, scores, selects, mutates, "
          "and checkpoints correctly; checkpoints load as plain Generators.")


def main():
    p = argparse.ArgumentParser()
    sub = p.add_subparsers(dest="cmd", required=True)

    pt = sub.add_parser("train")
    pt.add_argument("--data", required=True)
    pt.add_argument("--base", default="gan-zai-v5.3.py", help="path to the base gan-zai file to reuse")
    pt.add_argument("--pop", type=int, default=4)
    pt.add_argument("--rounds", type=int, default=500)
    pt.add_argument("--inner-iters", type=int, default=20)
    pt.add_argument("--mutate-frac", type=float, default=0.5)
    pt.add_argument("--mutation-std", type=float, default=0.02)
    pt.add_argument("--workers", type=int, default=4)
    pt.add_argument("--random-pairs", type=int, default=0,
                    help="if >0, sample this many random (G,D) pairs per round instead of all-pairs")
    pt.add_argument("--ckpt-dir", default="pop_checkpoints")
    pt.add_argument("--seed", type=int, default=0)

    sub.add_parser("test")
    args = p.parse_args()

    if args.cmd == "test":
        _selftest()
        return

    pcfg = PopConfig(
        pop_size=args.pop, rounds=args.rounds, inner_iters=args.inner_iters,
        mutate_frac=args.mutate_frac, mutation_std=args.mutation_std,
        all_pairs=(args.random_pairs == 0), pairs_per_round=args.random_pairs or 8,
        seed=args.seed,
    )
    train(args.base, args.data, pcfg, {}, ckpt_dir=args.ckpt_dir, workers=args.workers)


if __name__ == "__main__":
    main()
