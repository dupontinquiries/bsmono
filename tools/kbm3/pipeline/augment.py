"""
kBM3 augmentation — multiply the effective dataset by blending similar tracks.
==============================================================================
5-6k tracks is thin for a GAN. Naive mixup (blend any two random tracks) makes
garbage: a 140bpm track in C blended with a 90bpm track in F# isn't music, it's
mud. This instead only blends tracks that share tempo and key, on the theory
that two trap beats at the same BPM in the same key are harmonically/rhythmically
compatible, so the blend is more likely to land somewhere still "musical" -- a
plausible new "real" example for the GAN rather than an average of two unrelated
songs.

Two blend modes, both cheap (operate on already-computed unit-mels, no re-synthesis):
  - crossfade:  alpha * mel_a + (1-alpha) * mel_b        (timbre/energy blend)
  - bandswap:   low bins from A, high bins from B, soft-crossfaded seam
                (A's low end + B's high end -- since bass/hats are the parts
                most likely to differ between two same-key/tempo trap beats)

Usage:
    python augment.py features --cache cache             # one-time BPM/key pass
    python augment.py groups   --cache cache              # print group sizes

Integration: wrap your existing OpusMelDataset --
    from augment import AugmentedDataset
    ds = AugmentedDataset(OpusMelDataset(cfg), cache_dir=cfg.cache_dir, blend_prob=0.35)
    # __getitem__ unchanged signature: returns a torch.Tensor crop, same as before.
"""
import argparse
import json
import os
from collections import defaultdict
from dataclasses import dataclass

import numpy as np

try:
    import torch
    from torch.utils.data import Dataset
except ImportError:                      # augment.py's grouping logic doesn't need torch
    Dataset = object


# --------------------------------------------------------------------------- #
# Feature estimation (BPM + key), cached once per track
# --------------------------------------------------------------------------- #
@dataclass
class TrackFeatures:
    bpm: float
    key: int          # 0-11 pitch class of the dominant chroma bin


def estimate_features(y, sr) -> TrackFeatures:
    import librosa
    tempo, _ = librosa.beat.beat_track(y=y, sr=sr)
    bpm = float(tempo) if np.ndim(tempo) == 0 else float(tempo[0])
    if bpm <= 0:                          # beat tracker gave up (e.g. no clear pulse)
        bpm = 120.0
    chroma = librosa.feature.chroma_cqt(y=y, sr=sr)
    key = int(chroma.mean(axis=1).argmax())
    return TrackFeatures(bpm=bpm, key=key)


def build_features_cache(cache_dir: str, sr: int = 22050, exts=(".opus", ".ogg", ".mp3", ".wav", ".flac", ".m4a")):
    """One pass over the manifest's audio files, writing cache/features.json:
    {filepath: {bpm, key}}. Skips files already present so it's resumable."""
    import librosa
    manifest_path = os.path.join(cache_dir, "manifest.json")
    with open(manifest_path) as f:
        manifest = json.load(f)
    feat_path = os.path.join(cache_dir, "features.json")
    feats = {}
    if os.path.exists(feat_path):
        feats = json.load(open(feat_path))

    todo = [p for p in manifest if p and p not in feats]
    print(f"{len(feats)} cached, {len(todo)} to estimate...")
    for i, path in enumerate(todo):
        try:
            y, _ = librosa.load(path, sr=sr, mono=True)
            f = estimate_features(y, sr)
            feats[path] = {"bpm": f.bpm, "key": f.key}
        except Exception:
            continue
        if i % 100 == 0:
            print(f"  {i}/{len(todo)}")
            json.dump(feats, open(feat_path, "w"))
    json.dump(feats, open(feat_path, "w"))
    print(f"wrote {feat_path}: {len(feats)} tracks")
    return feats


# --------------------------------------------------------------------------- #
# Similarity grouping
# --------------------------------------------------------------------------- #
def bpm_bucket(bpm: float, tol: float = 6.0) -> int:
    """Coarse BPM bucket so near-tempo tracks (not just exact matches) group
    together; also folds half/double-time confusions (70 vs 140 read the same
    to a beat tracker on trap) by normalizing into a common octave band."""
    b = bpm
    while b < 70:
        b *= 2
    while b >= 140:
        b /= 2
    return round(b / tol)


def build_groups(features: dict, bpm_tol: float = 6.0) -> dict:
    """path -> (bpm_bucket, key) ; also returns the reverse index group -> [paths]."""
    groups = defaultdict(list)
    for path, f in features.items():
        key = (bpm_bucket(f["bpm"], bpm_tol), f["key"])
        groups[key].append(path)
    return dict(groups)


# --------------------------------------------------------------------------- #
# Mel-space blending
# --------------------------------------------------------------------------- #
def crossfade_blend(mel_a: np.ndarray, mel_b: np.ndarray, alpha: float) -> np.ndarray:
    """Straight alpha blend in unit-mel [-1,1] space. alpha in [0,1]."""
    return np.clip(alpha * mel_a + (1 - alpha) * mel_b, -1.0, 1.0).astype(np.float32)


def bandswap_blend(mel_a: np.ndarray, mel_b: np.ndarray, split: float = 0.5,
                    seam: int = 6) -> np.ndarray:
    """A's low mel bins + B's high mel bins, with a soft-crossfaded seam of
    `seam` bins so there's no hard seam artifact (which would just teach the
    discriminator to spot a straight line at a fixed frequency)."""
    n_mels = mel_a.shape[0]
    split_bin = int(n_mels * split)
    out = np.empty_like(mel_a)
    lo, hi = max(0, split_bin - seam), min(n_mels, split_bin + seam)
    out[:lo] = mel_a[:lo]
    out[hi:] = mel_b[hi:]
    if hi > lo:
        w = np.linspace(1, 0, hi - lo)[:, None]      # 1->0 across the seam: A fades out, B fades in
        out[lo:hi] = w * mel_a[lo:hi] + (1 - w) * mel_b[lo:hi]
    return out.astype(np.float32)


def blend(mel_a: np.ndarray, mel_b: np.ndarray, mode: str = "crossfade",
          rng: np.random.Generator | None = None) -> np.ndarray:
    rng = rng or np.random.default_rng()
    if mode == "crossfade":
        alpha = rng.uniform(0.3, 0.7)     # avoid near-0/1 (that's just "return the other track")
        return crossfade_blend(mel_a, mel_b, alpha)
    elif mode == "bandswap":
        split = rng.uniform(0.35, 0.65)
        return bandswap_blend(mel_a, mel_b, split)
    raise ValueError(f"unknown blend mode {mode!r}")


# --------------------------------------------------------------------------- #
# Dataset wrapper
# --------------------------------------------------------------------------- #
class AugmentedDataset(Dataset):
    """Wraps an existing mel Dataset (e.g. OpusMelDataset). With probability
    `blend_prob`, returns a blend of the requested track and a same-group
    (similar bpm/key) partner instead of the plain crop. Falls back to the
    plain crop if no group partner exists or features.json is absent, so this
    is always safe to enable even before running the features pass."""

    def __init__(self, base: Dataset, cache_dir: str, blend_prob: float = 0.35,
                 bpm_tol: float = 6.0, mode: str = "crossfade", seed: int | None = None):
        self.base = base
        self.blend_prob = blend_prob
        self.mode = mode
        self.rng = np.random.default_rng(seed)

        feat_path = os.path.join(cache_dir, "features.json")
        manifest_path = os.path.join(cache_dir, "manifest.json")
        self.enabled = os.path.exists(feat_path) and os.path.exists(manifest_path)
        self.groups_by_manifest_idx = {}
        if self.enabled:
            features = json.load(open(feat_path))
            manifest = json.load(open(manifest_path))
            groups = build_groups(features, bpm_tol)
            path_to_group = {}
            for members in groups.values():
                if len(members) < 2:
                    continue          # no partner available; skip singleton groups
                for m in members:
                    path_to_group[m] = members
            # map base dataset's manifest indices -> list of partner indices
            path_to_idx = {p: i for i, p in enumerate(manifest) if p is not None}
            for path, members in path_to_group.items():
                idx = path_to_idx.get(path)
                if idx is None:
                    continue
                partner_idxs = [path_to_idx[m] for m in members if m in path_to_idx and m != path]
                if partner_idxs:
                    self.groups_by_manifest_idx[idx] = partner_idxs

    def __len__(self):
        return len(self.base)

    def __getitem__(self, i):
        crop_a = self.base[i]
        if not self.enabled or self.rng.random() > self.blend_prob:
            return crop_a
        real_idx = self.base.valid_indices[i] if hasattr(self.base, "valid_indices") else i
        partners = self.groups_by_manifest_idx.get(real_idx)
        if not partners:
            return crop_a
        # pick a partner and find its position within base.valid_indices
        partner_real_idx = partners[self.rng.integers(len(partners))]
        if hasattr(self.base, "valid_indices"):
            try:
                j = self.base.valid_indices.index(partner_real_idx)
            except ValueError:
                return crop_a
        else:
            j = partner_real_idx
        crop_b = self.base[j]

        a_np = crop_a.squeeze(0).numpy() if hasattr(crop_a, "numpy") else np.asarray(crop_a)
        b_np = crop_b.squeeze(0).numpy() if hasattr(crop_b, "numpy") else np.asarray(crop_b)
        mode = self.mode if self.mode != "random" else self.rng.choice(["crossfade", "bandswap"])
        blended = blend(a_np, b_np, mode=mode, rng=self.rng)
        if hasattr(crop_a, "numpy"):
            return torch.from_numpy(blended).unsqueeze(0)
        return blended[None, ...]


# --------------------------------------------------------------------------- #
def main():
    p = argparse.ArgumentParser()
    sub = p.add_subparsers(dest="cmd", required=True)
    pf = sub.add_parser("features"); pf.add_argument("--cache", default="cache")
    pg = sub.add_parser("groups"); pg.add_argument("--cache", default="cache")
    pg.add_argument("--bpm-tol", type=float, default=6.0)
    args = p.parse_args()

    if args.cmd == "features":
        build_features_cache(args.cache)
    elif args.cmd == "groups":
        feats = json.load(open(os.path.join(args.cache, "features.json")))
        groups = build_groups(feats, args.bpm_tol)
        sizes = sorted((len(v) for v in groups.values()), reverse=True)
        multi = [s for s in sizes if s >= 2]
        print(f"{len(groups)} groups, {len(multi)} with >=2 tracks (blendable)")
        print("largest groups:", sizes[:15])
        blendable_tracks = sum(s for s in multi)
        print(f"{blendable_tracks}/{len(feats)} tracks have a blend partner")


if __name__ == "__main__":
    main()
