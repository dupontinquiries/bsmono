"""
kBM3 - AI Trap Music Generation (PyTorch / Asahi Linux)  -- v5.1
================================================================
Single-file successor to gan-zai-v4--r3_r2_fixed.py.

WHAT CHANGED vs v5 (training made feasible + observable on CPU)
--------------------------------------------------------------
  * ITERATION-BASED loop. v5 (like v4) defined an "epoch" as a full pass over
    all tracks -- on a 5,630-track set at CPU speed that is ~30 min between any
    log line, sample, or checkpoint. Training is now driven by global iteration
    count: --log-every / --sample-every / --ckpt-every are in ITERATIONS, so you
    get feedback every few minutes and can stop/resume anywhere. --max-iters
    replaces --epochs (an --epochs alias still works: it maps to iters).
  * HEALTH METRICS. Every log line and a rolling train_log.csv now report the
    two numbers that actually tell you if a GAN is working: mean D(real) and
    mean D(fake) as probabilities. ~0.5/0.5 = healthy game; D->1/0 = D winning
    (G starved); both drifting together = collapse. Computed from logits already
    in-hand, so zero extra forward passes.
  * INSTANCE NOISE. Small Gaussian noise added to the discriminator's inputs,
    decaying linearly to zero over --noise-iters. A standard, cheap stabilizer
    for spectrogram GANs that widens the support overlap early in training.
    Disable with --no-instance-noise.
  * CPU THREADS. --threads sets torch intra-op parallelism (defaults to all
    cores), which the v4/v5 loop never configured.
  * Checkpoints now carry the global step and are resumable by iteration.
    Generator-only gen_{step}.pt is still emitted for the explorer GUI.

WHAT CHANGED vs v4 (all fixes verified with torch shape/backprop checks)
-----------------------------------------------------------------------
CORRECTNESS
  * to_audio() called self._unit_to_db(), which was never defined -> hard crash
    at first sample. The exact inverse of _db_to_unit() is now implemented once,
    as a module-level function, and sampling no longer needs the __new__ hack.
  * The v4 Generator upsampled 1x1 -> 64x64, but real data is 128x256. The
    Discriminator's AdaptiveAvgPool2d(1) hid the mismatch, so it "trained" while
    G and the reals were literally different sizes and could never converge.
    The Generator now projects the latent to (B, f, n_mels/16, time_frames/16)
    and upsamples with four stride-2 blocks to (B, 1, 128, 256). Its state_dict
    is key-compatible with the v6 modular Generator, so gen_*.pt checkpoints load
    directly in the latent-space explorer GUI.
  * __getitem__ returned the WRONG track: the prefetch loop overwrote real_idx,
    so the item the DataLoader asked for was discarded. The hand-rolled prefetch
    buffer is gone; reads are now a single indexed SQLite lookup.
  * NameError: worker_finished_tally vs workers_finished_tally (fired on any
    fresh cache build).
  * `generate` crashed on args.data / args.no_opus (train-only args). Fixed, and
    it now accepts either a raw generator state_dict or a combined checkpoint.

PERFORMANCE / PRODUCT
  * Process-safe SQLite (connections keyed per-PID), so DataLoader num_workers>0
    is now safe -- data loading parallelises across cores (matters on CPU-only
    Asahi). --workers controls it.
  * Resumable training: combined checkpoints {epoch,G,D,optG,optD} + --resume.
    Generator-only gen_*.pt is still written for the GUI.
  * Depthwise-separable SparseBlock discriminator (~8-10x fewer MACs) -- the same
    block the v6 line uses -- for a meaningful CPU speedup.
  * --seed for reproducibility; optional PNG mel previews (if Pillow present);
    configurable --sample-every / --ckpt-every.

CACHE COMPATIBILITY
  * The on-disk format is unchanged (sharded SQLite, zlib-compressed float32 mel
    blobs). Your existing cache/ (5630 tracks) is reused as-is; no rebuild.

Usage
  python gan-zai-v5.py train --data ./music --epochs 20000 --batch 32 --workers 4
  python gan-zai-v5.py train --resume checkpoints/ckpt_3000.pt
  python gan-zai-v5.py generate --weights checkpoints/gen_3000.pt --n 4
"""

import argparse
import glob
import json
import os
import random
import shutil
import sqlite3
import subprocess
import time
import zlib
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import dataclass, field, asdict

import numpy as np
import librosa
import soundfile as sf

import torch
import torch.nn as nn
import torch.nn.functional as F
import torch.optim as optim
from torch.utils.data import Dataset, DataLoader


# --------------------------------------------------------------------------- #
# Config
# --------------------------------------------------------------------------- #
@dataclass
class Config:
    sr: int = 22050
    n_fft: int = 1024
    hop_length: int = 256
    n_mels: int = 128
    time_frames: int = 256
    db_floor: float = -80.0
    db_ceil: float = 40.0
    shard_size: int = 250              # Files per SQLite database

    latent: int = 128
    g_filters: int = 256
    d_filters: int = 64

    lr: float = 2e-4
    beta_1: float = 0.5
    label_smooth: float = 0.9
    griffinlim_iters: int = 64

    data_dir: str = "./music"
    ckpt_dir: str = "checkpoints"
    sample_dir: str = "samples"
    cache_dir: str = "cache"
    encode_opus: bool = True

    # --- iteration-based scheduling (all counts are in optimizer steps) ---
    max_iters: int = 100_000
    log_every: int = 50
    sample_every: int = 2_000
    ckpt_every: int = 2_000

    # --- CPU / stabilization ---
    threads: int = 0                   # 0 => use all cores
    instance_noise_sigma: float = 0.1  # std of decaying noise on D inputs (0=off)
    instance_noise_iters: int = 20_000 # iters over which the noise decays to 0

    audio_exts: tuple[str, ...] = field(
        default_factory=lambda: (".opus", ".ogg", ".mp3", ".wav", ".flac", ".m4a")
    )

    @property
    def min_h(self) -> int: return self.n_mels // 16
    @property
    def min_w(self) -> int: return self.time_frames // 16

    def __post_init__(self):
        if self.n_mels % 16 or self.time_frames % 16:
            raise ValueError("n_mels and time_frames must be divisible by 16.")


# --------------------------------------------------------------------------- #
# dB <-> [-1, 1] mapping (single source of truth, fully reversible)
# --------------------------------------------------------------------------- #
def db_to_unit(db: np.ndarray, cfg: Config) -> np.ndarray:
    db = np.clip(db, cfg.db_floor, cfg.db_ceil)
    return (db - cfg.db_floor) / (cfg.db_ceil - cfg.db_floor) * 2.0 - 1.0


def unit_to_db(unit: np.ndarray, cfg: Config) -> np.ndarray:
    unit = np.clip(unit, -1.0, 1.0)
    return unit * (cfg.db_ceil - cfg.db_floor) / 2.0 + (cfg.db_ceil + cfg.db_floor) / 2.0


def mel_unit_to_audio(unit_spec: np.ndarray, cfg: Config) -> np.ndarray:
    """Invert a [-1,1] mel image back to a waveform via Griffin-Lim."""
    db = unit_to_db(np.squeeze(unit_spec), cfg)
    power = librosa.db_to_power(db, ref=1.0)
    return librosa.feature.inverse.mel_to_audio(
        power, sr=cfg.sr, n_fft=cfg.n_fft,
        hop_length=cfg.hop_length, n_iter=cfg.griffinlim_iters,
    )


# --------------------------------------------------------------------------- #
# Data: Sharded SQLite + Threaded Builder + process-safe reads
# --------------------------------------------------------------------------- #
class OpusMelDataset(Dataset):
    def __init__(self, cfg: Config):
        self.cfg = cfg
        os.makedirs(cfg.cache_dir, exist_ok=True)
        self.manifest_path = os.path.join(cfg.cache_dir, "manifest.json")

        # Read connections are keyed per (pid, shard) so DataLoader workers
        # (separate processes) never share an sqlite handle. Created lazily
        # after fork, so this dict is empty at pickle time.
        self._pid_conns: dict = {}

        self.valid_indices: list[int] = []

        self._load_manifest()
        self._build_threaded()
        self._setup_valid_indices()

    # ---- shard helpers ---------------------------------------------------- #
    def _get_shard_name(self, shard_id: int) -> str:
        start = shard_id * self.cfg.shard_size
        end = start + self.cfg.shard_size - 1
        return f"db-{start:03d}-{end:03d}.sqlite"

    def _get_read_conn(self, shard_id: int) -> sqlite3.Connection:
        key = (os.getpid(), shard_id)
        conn = self._pid_conns.get(key)
        if conn is None:
            db_path = os.path.join(self.cfg.cache_dir, self._get_shard_name(shard_id))
            conn = sqlite3.connect(db_path, check_same_thread=False)
            conn.execute("PRAGMA journal_mode=WAL;")   # fast concurrent reads
            conn.execute("PRAGMA query_only=ON;")
            self._pid_conns[key] = conn
        return conn

    # ---- manifest --------------------------------------------------------- #
    def _load_manifest(self):
        if os.path.exists(self.manifest_path):
            with open(self.manifest_path, "r") as f:
                self.manifest = json.load(f)
        else:
            self.manifest = []

    def _save_manifest(self):
        with open(self.manifest_path, "w") as f:
            json.dump(self.manifest, f, indent=2)

    # ---- build ------------------------------------------------------------ #
    def _process_shard(self, shard_id: int, files_to_process: list[str]) -> int:
        """Thread target: build exactly one shard. Storage format is unchanged
        from v4 (zlib-compressed float32 mel blob), so old caches stay valid."""
        if not files_to_process:
            return 0

        db_path = os.path.join(self.cfg.cache_dir, self._get_shard_name(shard_id))
        conn = sqlite3.connect(db_path)
        cursor = conn.cursor()
        cursor.execute("""CREATE TABLE IF NOT EXISTS mels (
                            filepath TEXT PRIMARY KEY,
                            data BLOB NOT NULL)""")

        success_count = 0
        for fpath in files_to_process:
            try:
                y, _ = librosa.load(fpath, sr=self.cfg.sr, mono=True)
                if y.size < self.cfg.hop_length * self.cfg.time_frames:
                    continue

                mel = librosa.feature.melspectrogram(
                    y=y, sr=self.cfg.sr, n_fft=self.cfg.n_fft,
                    hop_length=self.cfg.hop_length, n_mels=self.cfg.n_mels)
                db = librosa.power_to_db(mel, ref=1.0)
                unit = db_to_unit(db, self.cfg).astype(np.float32)

                blob = zlib.compress(unit.tobytes(), level=4)
                cursor.execute(
                    "INSERT OR REPLACE INTO mels (filepath, data) VALUES (?, ?)",
                    (fpath, blob))
                success_count += 1
            except Exception:
                pass  # silently skip failed decodes

        conn.commit()
        conn.close()
        return success_count

    def _build_threaded(self):
        """Discover new files, assign manifest slots, build shards in parallel."""
        disk_files: set[str] = set()
        for ext in self.cfg.audio_exts:
            disk_files.update(
                glob.glob(os.path.join(self.cfg.data_dir, f"**/*{ext}"), recursive=True))
        disk_files = sorted(disk_files)

        manifest_set = set(f for f in self.manifest if f is not None)
        new_files = [f for f in disk_files if f not in manifest_set]

        if not new_files:
            print("All audio files already cached in sharded databases.")
            return

        print(f"Found {len(new_files)} new files. Assigning to shards...")

        # Assign new files to manifest slots so indices never shift later.
        for f in new_files:
            self.manifest.append(f)
        self._save_manifest()

        shards_to_build: dict[int, list[str]] = {}
        for f in new_files:
            idx = self.manifest.index(f)
            shard_id = idx // self.cfg.shard_size
            shards_to_build.setdefault(shard_id, []).append(f)

        max_workers = min(os.cpu_count() or 4, len(shards_to_build))
        print(f"Building {len(shards_to_build)} shard(s) across {max_workers} thread(s)...")

        total_processed = 0
        workers_finished = 0
        with ThreadPoolExecutor(max_workers=max_workers) as executor:
            futures = {
                executor.submit(self._process_shard, sid, files): sid
                for sid, files in shards_to_build.items()
            }
            for future in as_completed(futures):
                sid = futures[future]
                try:
                    count = future.result()
                    total_processed += count
                    workers_finished += 1
                    print(f"  [{workers_finished}] Shard {self._get_shard_name(sid)} "
                          f"complete ({count} files processed).")
                except Exception as e:
                    print(f"  [ERROR] Shard {self._get_shard_name(sid)} failed: {e}")

        print(f"Database build finished. {total_processed} new mel-spectrograms generated.")

    def _setup_valid_indices(self):
        """Determine which manifest indices actually contain valid DB entries."""
        print("Verifying database integrity (fast lookup)...")
        self.valid_indices = []

        for shard_id in set(i // self.cfg.shard_size for i in range(len(self.manifest))):
            start_idx = shard_id * self.cfg.shard_size
            end_idx = start_idx + self.cfg.shard_size

            db_path = os.path.join(self.cfg.cache_dir, self._get_shard_name(shard_id))
            if not os.path.exists(db_path):
                continue

            conn = sqlite3.connect(db_path)
            cursor = conn.cursor()
            cursor.execute("SELECT filepath FROM mels")
            db_files = set(row[0] for row in cursor.fetchall())
            conn.close()

            for i in range(start_idx, min(end_idx, len(self.manifest))):
                fpath = self.manifest[i]
                if fpath is not None and fpath in db_files:
                    self.valid_indices.append(i)

        print(f"Ready. {len(self.valid_indices)} valid tracks available for training.")

    # ---- Dataset API ------------------------------------------------------ #
    def __len__(self) -> int:
        return len(self.valid_indices)

    def __getitem__(self, idx: int) -> torch.Tensor:
        """Return a random time-crop of the requested track. Single indexed
        SQLite read -- correct, and safe under num_workers>0."""
        real_idx = self.valid_indices[idx]
        path = self.manifest[real_idx]
        shard_id = real_idx // self.cfg.shard_size

        conn = self._get_read_conn(shard_id)
        cursor = conn.cursor()
        cursor.execute("SELECT data FROM mels WHERE filepath = ?", (path,))
        row = cursor.fetchone()
        if row is None:
            # Should not happen after _setup_valid_indices; degrade gracefully.
            return torch.full((1, self.cfg.n_mels, self.cfg.time_frames), -1.0)

        arr = np.frombuffer(zlib.decompress(row[0]), dtype=np.float32).copy()
        arr = arr.reshape(self.cfg.n_mels, -1)

        tf = self.cfg.time_frames
        if arr.shape[1] < tf:                      # short track: pad with silence
            pad = np.full((self.cfg.n_mels, tf - arr.shape[1]), -1.0, np.float32)
            crop = np.concatenate([arr, pad], axis=1)
        else:
            t0 = np.random.randint(0, arr.shape[1] - tf + 1)
            crop = arr[:, t0:t0 + tf]

        return torch.from_numpy(crop).unsqueeze(0)


# --------------------------------------------------------------------------- #
# Models
# --------------------------------------------------------------------------- #
def weights_init(m):
    classname = m.__class__.__name__
    if classname.find("Conv") != -1:
        nn.init.normal_(m.weight.data, 0.0, 0.02)
    elif classname.find("BatchNorm") != -1:
        nn.init.normal_(m.weight.data, 1.0, 0.02)
        nn.init.constant_(m.bias.data, 0)


class SparseBlock(nn.Module):
    """Depthwise-separable conv with activation-sparsity masking.
    ~8-10x fewer multiply-adds than a dense Conv2d (matches the v6 line)."""
    def __init__(self, cin, cout, stride=1, threshold=0.05):
        super().__init__()
        self.depthwise = nn.Conv2d(cin, cin, 3, stride, padding=1, groups=cin, bias=False)
        self.pointwise = nn.Conv2d(cin, cout, 1, bias=False)
        self.bn = nn.BatchNorm2d(cout)
        self.threshold = threshold

    def forward(self, x):
        x = self.depthwise(x)
        x = x * (x.abs() > self.threshold).float()
        x = self.pointwise(x)
        return F.leaky_relu(self.bn(x), 0.2, inplace=True)


class Generator(nn.Module):
    """z (B, latent) -> mel (B, 1, n_mels, time_frames) in [-1, 1].

    Shape proof (n_mels=128, time_frames=256 -> min 8x16):
      fc: latent -> f*8*16, reshape (B, f, 8, 16)
      x4 ConvTranspose2d k4 s2 p1:  8->16->32->64->128  /  16->32->64->128->256
    state_dict keys match the v6 modular Generator (fc.*, main.*)."""
    def __init__(self, cfg: Config):
        super().__init__()
        f = cfg.g_filters
        self.f = f
        self.min_h, self.min_w = cfg.min_h, cfg.min_w
        self.fc = nn.Linear(cfg.latent, self.min_h * self.min_w * f)
        self.main = nn.Sequential(
            nn.ConvTranspose2d(f, f // 2, 4, 2, 1, bias=False),
            nn.BatchNorm2d(f // 2), nn.ReLU(True),
            nn.ConvTranspose2d(f // 2, f // 4, 4, 2, 1, bias=False),
            nn.BatchNorm2d(f // 4), nn.ReLU(True),
            nn.ConvTranspose2d(f // 4, f // 8, 4, 2, 1, bias=False),
            nn.BatchNorm2d(f // 8), nn.ReLU(True),
            nn.ConvTranspose2d(f // 8, 1, 4, 2, 1, bias=False),
            nn.Tanh(),
        )
        self.apply(weights_init)

    def forward(self, z):
        x = self.fc(z).view(-1, self.f, self.min_h, self.min_w)
        return self.main(x)


class Discriminator(nn.Module):
    """mel (B, 1, n_mels, time_frames) -> logit (B,). AdaptiveAvgPool makes it
    resolution-agnostic, but G now emits the exact target size regardless."""
    def __init__(self, cfg: Config):
        super().__init__()
        f = cfg.d_filters
        self.main = nn.Sequential(
            SparseBlock(1, f, stride=2),
            SparseBlock(f, f * 2, stride=2),
            SparseBlock(f * 2, f * 4, stride=2),
            SparseBlock(f * 4, f * 8, stride=2),
        )
        self.avgpool = nn.AdaptiveAvgPool2d(1)
        self.fc = nn.Linear(f * 8, 1)
        self.apply(weights_init)

    def forward(self, x):
        x = self.main(x)
        x = self.avgpool(x)
        x = torch.flatten(x, 1)
        return self.fc(x).squeeze(1)


# --------------------------------------------------------------------------- #
# Checkpointing
# --------------------------------------------------------------------------- #
def save_checkpoint(cfg, step, netG, netD, optG, optD):
    os.makedirs(cfg.ckpt_dir, exist_ok=True)
    torch.save(
        {"step": step, "G": netG.state_dict(), "D": netD.state_dict(),
         "optG": optG.state_dict(), "optD": optD.state_dict(), "cfg": asdict(cfg)},
        os.path.join(cfg.ckpt_dir, f"ckpt_{step}.pt"))
    # Generator-only weights: raw state_dict, loadable by the explorer GUI.
    torch.save(netG.state_dict(), os.path.join(cfg.ckpt_dir, f"gen_{step}.pt"))


def _infinite(dataloader):
    """Yield batches forever so the loop is driven by iteration count."""
    while True:
        for batch in dataloader:
            yield batch


# --------------------------------------------------------------------------- #
# Training
# --------------------------------------------------------------------------- #
def _instance_noise_sigma(cfg: Config, step: int) -> float:
    """Linearly-decaying std for instance noise; 0 once past noise_iters."""
    if cfg.instance_noise_sigma <= 0 or step >= cfg.instance_noise_iters:
        return 0.0
    return cfg.instance_noise_sigma * (1.0 - step / cfg.instance_noise_iters)


def train(cfg: Config, batch_size: int, device: torch.device,
          workers: int = 4, resume: str | None = None):
    os.makedirs(cfg.ckpt_dir, exist_ok=True)
    os.makedirs(cfg.sample_dir, exist_ok=True)

    if cfg.threads > 0:
        torch.set_num_threads(cfg.threads)
    print(f"torch intra-op threads: {torch.get_num_threads()}")

    dataset = OpusMelDataset(cfg)
    if len(dataset) < batch_size:
        raise SystemExit(f"Only {len(dataset)} tracks available; need >= batch size {batch_size}.")
    dataloader = DataLoader(dataset, batch_size=batch_size, shuffle=True,
                            drop_last=True, num_workers=workers,
                            persistent_workers=workers > 0)

    netG = Generator(cfg).to(device)
    netD = Discriminator(cfg).to(device)
    optG = optim.Adam(netG.parameters(), lr=cfg.lr, betas=(cfg.beta_1, 0.999))
    optD = optim.Adam(netD.parameters(), lr=cfg.lr, betas=(cfg.beta_1, 0.999))
    criterion = nn.BCEWithLogitsLoss()

    start_step = 0
    if resume:
        ck = torch.load(resume, map_location=device)
        netG.load_state_dict(ck["G"]); netD.load_state_dict(ck["D"])
        optG.load_state_dict(ck["optG"]); optD.load_state_dict(ck["optD"])
        start_step = ck.get("step", ck.get("epoch", 0)) + 1
        print(f"Resumed from {resume} at step {start_step}.")

    csv_path = os.path.join(cfg.ckpt_dir, "train_log.csv")
    if not os.path.exists(csv_path):
        with open(csv_path, "w") as f:
            f.write("step,lossD,lossG,D_real,D_fake,noise_sigma,ms_per_it\n")

    print(f"Training on {device} ({workers} loader workers). "
          f"Target {cfg.max_iters} iters. Ctrl-C is safe -- resume with --resume.")
    batches = _infinite(dataloader)
    t0 = time.time()

    for step in range(start_step, cfg.max_iters):
        real_imgs = next(batches).to(device)
        b_size = real_imgs.size(0)

        real_label = torch.full((b_size,), cfg.label_smooth,
                                dtype=torch.float32, device=device)
        fake_label = torch.zeros(b_size, dtype=torch.float32, device=device)

        sigma = _instance_noise_sigma(cfg, step)

        def noised(x):
            return x + torch.randn_like(x) * sigma if sigma > 0 else x

        # --- Discriminator: real-as-real, fake-as-fake ---
        netD.zero_grad()
        d_real_logits = netD(noised(real_imgs))
        lossD_real = criterion(d_real_logits, real_label)
        fake = netG(torch.randn(b_size, cfg.latent, device=device))
        d_fake_logits = netD(noised(fake.detach()))
        lossD_fake = criterion(d_fake_logits, fake_label)
        lossD = lossD_real + lossD_fake
        lossD.backward()
        optD.step()

        # --- Generator: fool D ---
        netG.zero_grad()
        lossG = criterion(netD(noised(fake)), real_label)
        lossG.backward()
        optG.step()

        if step % cfg.log_every == 0:
            with torch.no_grad():
                d_real = torch.sigmoid(d_real_logits).mean().item()
                d_fake = torch.sigmoid(d_fake_logits).mean().item()
            ms = (time.time() - t0) / (step - start_step + 1) * 1000
            print(f"[{step}/{cfg.max_iters}] D: {lossD.item():.4f} G: {lossG.item():.4f} "
                  f"| D(real): {d_real:.2f} D(fake): {d_fake:.2f} "
                  f"| noise: {sigma:.3f} ({ms:.0f}ms/it)")
            with open(csv_path, "a") as f:
                f.write(f"{step},{lossD.item():.4f},{lossG.item():.4f},"
                        f"{d_real:.4f},{d_fake:.4f},{sigma:.4f},{ms:.1f}\n")

        if step > 0 and step % cfg.sample_every == 0:
            sample(netG, cfg, device, step)
        if step > 0 and step % cfg.ckpt_every == 0:
            save_checkpoint(cfg, step, netG, netD, optG, optD)

    save_checkpoint(cfg, cfg.max_iters, netG, netD, optG, optD)
    print("Training complete.")


def _save_png(unit_spec: np.ndarray, path: str):
    """Optional grayscale mel preview (skipped if Pillow is absent)."""
    try:
        from PIL import Image
    except ImportError:
        return
    s = np.squeeze(unit_spec)
    img = ((s + 1.0) * 127.5).clip(0, 255).astype(np.uint8)[::-1]  # low freq at bottom
    Image.fromarray(img, mode="L").save(path)


def sample(netG: Generator, cfg: Config, device: torch.device, step: int, n: int = 2):
    netG.eval()
    with torch.no_grad():
        specs = netG(torch.randn(n, cfg.latent, device=device)).cpu().numpy()
    netG.train()

    for i in range(n):
        stem = os.path.join(cfg.sample_dir, f"it{step}_{i}")
        _save_png(specs[i], stem + ".png")
        sf.write(stem + ".wav", mel_unit_to_audio(specs[i], cfg), cfg.sr)
        if cfg.encode_opus:
            _encode_opus(stem + ".wav")


def _encode_opus(wav_path: str):
    if shutil.which("ffmpeg") is None:
        return
    opus_path = os.path.splitext(wav_path)[0] + ".opus"
    subprocess.run(
        ["ffmpeg", "-y", "-loglevel", "error", "-i", wav_path,
         "-c:a", "libopus", "-b:a", "96k", opus_path], check=True)


# --------------------------------------------------------------------------- #
# Main
# --------------------------------------------------------------------------- #
def set_seed(seed: int):
    random.seed(seed)
    np.random.seed(seed)
    torch.manual_seed(seed)


def main():
    p = argparse.ArgumentParser(description="kBM3 GAN music generator (v5)")
    sub = p.add_subparsers(dest="cmd", required=True)

    pt = sub.add_parser("train")
    pt.add_argument("--data", default="./music")
    pt.add_argument("--iters", "--epochs", dest="iters", type=int, default=100_000,
                    help="total optimizer steps (--epochs is a legacy alias)")
    pt.add_argument("--batch", type=int, default=32)
    pt.add_argument("--workers", type=int, default=4)
    pt.add_argument("--threads", type=int, default=0, help="torch CPU threads (0=all)")
    pt.add_argument("--resume", default=None)
    pt.add_argument("--log-every", type=int, default=50)
    pt.add_argument("--sample-every", type=int, default=2_000)
    pt.add_argument("--ckpt-every", type=int, default=2_000)
    pt.add_argument("--no-instance-noise", action="store_true")
    pt.add_argument("--no-opus", action="store_true")
    pt.add_argument("--seed", type=int, default=None)

    pg = sub.add_parser("generate")
    pg.add_argument("--weights", required=True)
    pg.add_argument("--n", type=int, default=4)
    pg.add_argument("--out", default="samples")
    pg.add_argument("--no-opus", action="store_true")
    pg.add_argument("--seed", type=int, default=None)

    args = p.parse_args()
    if args.seed is not None:
        set_seed(args.seed)

    device = torch.device("cuda" if torch.cuda.is_available() else "cpu")
    if device.type == "cpu":
        print("Warning: No GPU backend found. Running on CPU.")

    if args.cmd == "train":
        cfg = Config(
            data_dir=args.data, encode_opus=not args.no_opus,
            max_iters=args.iters, threads=args.threads,
            log_every=args.log_every, sample_every=args.sample_every,
            ckpt_every=args.ckpt_every,
            instance_noise_sigma=0.0 if args.no_instance_noise else Config.instance_noise_sigma,
        )
        train(cfg, args.batch, device, workers=args.workers, resume=args.resume)

    elif args.cmd == "generate":
        cfg = Config(sample_dir=args.out, encode_opus=not args.no_opus)
        os.makedirs(cfg.sample_dir, exist_ok=True)
        netG = Generator(cfg).to(device)
        state = torch.load(args.weights, map_location=device)
        if isinstance(state, dict) and "G" in state:   # combined checkpoint
            state = state["G"]
        netG.load_state_dict(state)
        sample(netG, cfg, device, step=0, n=args.n)


if __name__ == "__main__":
    main()
