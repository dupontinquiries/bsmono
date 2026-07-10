"""
kBM3 neural vocoder — mel -> waveform, to replace Griffin-Lim.
=============================================================
A HiFi-GAN-style generator trained to invert the *exact* mel representation the
rest of the pipeline uses: melspectrogram -> power_to_db(ref=1.0) -> clip to
[db_floor, db_ceil] -> [-1, 1]. Because it learns that specific representation,
the GAN's generated unit-mels feed straight in, and inference is a single fast
forward pass (unlike Griffin-Lim's dozens of iterations).

Losses: multi-resolution STFT + L1 log-mel reconstruction + LS-GAN adversarial
(multi-period discriminator) + feature matching.

    python vocoder.py train --data ./music --iters 200000 --batch 16
    python vocoder.py test          # shape/step self-check on synthetic data

Integration: `vocode(unit_mel, cfg, netG)` returns a waveform for a [-1,1] mel
of shape [n_mels, T]; drop it in place of mel_unit_to_audio() in gan-zai-v5.2.py
and server.py.
"""
import argparse
import glob
import os
import time
from dataclasses import dataclass, field

import numpy as np
import librosa

import torch
import torch.nn as nn
import torch.nn.functional as F
from torch.utils.data import Dataset, DataLoader


# --------------------------------------------------------------------------- #
# Config — must match the mel front-end used everywhere else in the pipeline
# --------------------------------------------------------------------------- #
@dataclass
class VocConfig:
    sr: int = 22050
    n_fft: int = 1024
    hop: int = 256
    n_mels: int = 128
    fmin: float = 0.0
    fmax: float | None = None
    db_floor: float = -80.0
    db_ceil: float = 40.0

    segment_frames: int = 32          # mel frames per training segment
    upsample_rates: tuple = (8, 8, 2, 2)          # product == hop (256)
    upsample_kernels: tuple = (16, 16, 4, 4)
    upsample_init_ch: int = 256
    resblock_kernels: tuple = (3, 7, 11)
    resblock_dilations: tuple = ((1, 3, 5), (1, 3, 5), (1, 3, 5))

    lr: float = 2e-4
    lambda_stft: float = 2.5
    lambda_mel: float = 45.0
    lambda_fm: float = 2.0

    @property
    def segment_len(self) -> int:
        return self.segment_frames * self.hop


# --------------------------------------------------------------------------- #
# Shared mel transforms (mirror the pipeline's cache builder exactly)
# --------------------------------------------------------------------------- #
def db_to_unit(db, cfg):
    db = np.clip(db, cfg.db_floor, cfg.db_ceil)
    return (db - cfg.db_floor) / (cfg.db_ceil - cfg.db_floor) * 2.0 - 1.0


def wav_to_unit_mel(y, cfg):
    """Exactly the pipeline's representation: the vocoder learns to invert THIS."""
    mel = librosa.feature.melspectrogram(
        y=y, sr=cfg.sr, n_fft=cfg.n_fft, hop_length=cfg.hop,
        n_mels=cfg.n_mels, fmin=cfg.fmin, fmax=cfg.fmax)
    db = librosa.power_to_db(mel, ref=1.0)
    return db_to_unit(db, cfg).astype(np.float32)


class MelSpec(nn.Module):
    """Differentiable log-mel for the reconstruction loss (torch-only)."""
    def __init__(self, cfg):
        super().__init__()
        basis = librosa.filters.mel(sr=cfg.sr, n_fft=cfg.n_fft, n_mels=cfg.n_mels,
                                    fmin=cfg.fmin, fmax=cfg.fmax)
        self.register_buffer("basis", torch.from_numpy(basis).float())
        self.register_buffer("window", torch.hann_window(cfg.n_fft))
        self.n_fft, self.hop = cfg.n_fft, cfg.hop

    def forward(self, wav):                    # wav: [B, 1, L] or [B, L]
        if wav.dim() == 3:
            wav = wav.squeeze(1)
        spec = torch.stft(wav, self.n_fft, self.hop, window=self.window,
                          return_complex=True)
        mag = spec.abs()                        # [B, n_fft/2+1, T]
        mel = torch.matmul(self.basis, mag)     # [B, n_mels, T]
        return torch.log(torch.clamp(mel, min=1e-5))


# --------------------------------------------------------------------------- #
# HiFi-GAN generator
# --------------------------------------------------------------------------- #
class ResBlock(nn.Module):
    def __init__(self, ch, k, dilations):
        super().__init__()
        self.convs1 = nn.ModuleList([
            nn.Conv1d(ch, ch, k, 1, dilation=d, padding=d * (k - 1) // 2) for d in dilations])
        self.convs2 = nn.ModuleList([
            nn.Conv1d(ch, ch, k, 1, dilation=1, padding=(k - 1) // 2) for _ in dilations])

    def forward(self, x):
        for c1, c2 in zip(self.convs1, self.convs2):
            xt = c2(F.leaky_relu(c1(F.leaky_relu(x, 0.1)), 0.1))
            x = x + xt
        return x


class Generator(nn.Module):
    def __init__(self, cfg: VocConfig):
        super().__init__()
        self.num_up = len(cfg.upsample_rates)
        self.num_res = len(cfg.resblock_kernels)
        ch = cfg.upsample_init_ch
        self.conv_pre = nn.Conv1d(cfg.n_mels, ch, 7, 1, padding=3)
        self.ups = nn.ModuleList()
        self.resblocks = nn.ModuleList()
        for i, (u, k) in enumerate(zip(cfg.upsample_rates, cfg.upsample_kernels)):
            cin, cout = ch // (2 ** i), ch // (2 ** (i + 1))
            self.ups.append(nn.ConvTranspose1d(cin, cout, k, u, padding=(k - u) // 2))
            for rk, rd in zip(cfg.resblock_kernels, cfg.resblock_dilations):
                self.resblocks.append(ResBlock(cout, rk, rd))
        self.conv_post = nn.Conv1d(cout, 1, 7, 1, padding=3)

    def forward(self, mel):                     # mel: [B, n_mels, T]
        x = self.conv_pre(mel)
        for i in range(self.num_up):
            x = self.ups[i](F.leaky_relu(x, 0.1))
            xs = 0
            for j in range(self.num_res):
                xs = xs + self.resblocks[i * self.num_res + j](x)
            x = xs / self.num_res
        x = torch.tanh(self.conv_post(F.leaky_relu(x, 0.1)))
        return x                                # [B, 1, T*hop]


# --------------------------------------------------------------------------- #
# Multi-Period Discriminator (+ feature maps for feature matching)
# --------------------------------------------------------------------------- #
class PeriodDisc(nn.Module):
    def __init__(self, period):
        super().__init__()
        self.period = period
        self.convs = nn.ModuleList([
            nn.Conv2d(1, 32, (5, 1), (3, 1), padding=(2, 0)),
            nn.Conv2d(32, 128, (5, 1), (3, 1), padding=(2, 0)),
            nn.Conv2d(128, 256, (5, 1), (3, 1), padding=(2, 0)),
            nn.Conv2d(256, 512, (5, 1), (3, 1), padding=(2, 0)),
            nn.Conv2d(512, 512, (5, 1), 1, padding=(2, 0)),
        ])
        self.post = nn.Conv2d(512, 1, (3, 1), 1, padding=(1, 0))

    def forward(self, x):                       # x: [B, 1, L]
        b, c, t = x.shape
        if t % self.period:
            x = F.pad(x, (0, self.period - (t % self.period)), "reflect")
            t = x.shape[2]
        x = x.view(b, c, t // self.period, self.period)
        feats = []
        for conv in self.convs:
            x = F.leaky_relu(conv(x), 0.1)
            feats.append(x)
        x = self.post(x)
        feats.append(x)
        return x.flatten(1), feats


class MultiPeriodDisc(nn.Module):
    def __init__(self, periods=(2, 3, 5, 7, 11)):
        super().__init__()
        self.discs = nn.ModuleList([PeriodDisc(p) for p in periods])

    def forward(self, x):
        outs, feats = [], []
        for d in self.discs:
            o, f = d(x)
            outs.append(o)
            feats.append(f)
        return outs, feats


# --------------------------------------------------------------------------- #
# Multi-resolution STFT loss (Parallel WaveGAN style)
# --------------------------------------------------------------------------- #
class MRSTFTLoss(nn.Module):
    def __init__(self, ffts=(512, 1024, 2048), hops=(128, 256, 512), wins=(400, 1024, 2048)):
        super().__init__()
        self.params = list(zip(ffts, hops, wins))
        for w in wins:
            self.register_buffer(f"win_{w}", torch.hann_window(w))

    def _stft_mag(self, x, n_fft, hop, win):
        window = getattr(self, f"win_{win}")
        spec = torch.stft(x, n_fft, hop, win_length=win, window=window, return_complex=True)
        return spec.abs().clamp(min=1e-7)

    def forward(self, pred, target):            # [B, 1, L] or [B, L]
        if pred.dim() == 3:
            pred, target = pred.squeeze(1), target.squeeze(1)
        total = 0.0
        for n_fft, hop, win in self.params:
            p = self._stft_mag(pred, n_fft, hop, win)
            t = self._stft_mag(target, n_fft, hop, win)
            sc = torch.norm(t - p, p="fro") / (torch.norm(t, p="fro") + 1e-7)   # spectral convergence
            mag = F.l1_loss(torch.log(p), torch.log(t))                          # log-magnitude
            total = total + sc + mag
        return total / len(self.params)


# --------------------------------------------------------------------------- #
# Data: audio -> (unit-mel, waveform) aligned segments
# --------------------------------------------------------------------------- #
class VocoderDataset(Dataset):
    def __init__(self, cfg: VocConfig, data_dir: str,
                 exts=(".opus", ".ogg", ".mp3", ".wav", ".flac", ".m4a")):
        self.cfg = cfg
        self.files = []
        for e in exts:
            self.files += glob.glob(os.path.join(data_dir, f"**/*{e}"), recursive=True)
        if not self.files:
            raise SystemExit(f"no audio found under {data_dir}")

    def __len__(self):
        return len(self.files)

    def __getitem__(self, idx):
        cfg = self.cfg
        seg = cfg.segment_len
        for _ in range(4):                       # retry on too-short/bad files
            try:
                y, _ = librosa.load(self.files[np.random.randint(len(self.files))],
                                    sr=cfg.sr, mono=True)
            except Exception:
                continue
            if y.size >= seg + cfg.hop:
                start = np.random.randint(0, y.size - seg - cfg.hop)
                wav = y[start:start + seg].astype(np.float32)
                # mel spans the segment; pad a hop so mel width == segment_frames
                mel = wav_to_unit_mel(y[start:start + seg + cfg.hop], cfg)[:, :cfg.segment_frames]
                return torch.from_numpy(mel), torch.from_numpy(wav).unsqueeze(0)
        # fallback: silence
        return (torch.full((cfg.n_mels, cfg.segment_frames), -1.0),
                torch.zeros(1, seg))


# --------------------------------------------------------------------------- #
# Losses (generator side)
# --------------------------------------------------------------------------- #
def feature_matching(feats_real, feats_fake):
    loss = 0.0
    for fr_list, ff_list in zip(feats_real, feats_fake):
        for fr, ff in zip(fr_list, ff_list):
            loss = loss + F.l1_loss(ff, fr.detach())
    return loss


# --------------------------------------------------------------------------- #
# Training
# --------------------------------------------------------------------------- #
def train(cfg: VocConfig, data_dir, iters, batch, device, workers=4,
          ckpt_dir="vocoder_ckpt", log_every=50, ckpt_every=5000):
    os.makedirs(ckpt_dir, exist_ok=True)
    ds = VocoderDataset(cfg, data_dir)
    dl = DataLoader(ds, batch_size=batch, shuffle=True, drop_last=True,
                    num_workers=workers, persistent_workers=workers > 0)

    netG = Generator(cfg).to(device)
    netD = MultiPeriodDisc().to(device)
    optG = torch.optim.AdamW(netG.parameters(), lr=cfg.lr, betas=(0.8, 0.99))
    optD = torch.optim.AdamW(netD.parameters(), lr=cfg.lr, betas=(0.8, 0.99))
    mrstft = MRSTFTLoss().to(device)
    melspec = MelSpec(cfg).to(device)

    print(f"Vocoder training on {device}. {len(ds)} files.")
    it = iter(dl)
    t0 = time.time()
    for step in range(iters):
        try:
            mel, wav = next(it)
        except StopIteration:
            it = iter(dl); mel, wav = next(it)
        mel, wav = mel.to(device), wav.to(device)

        fake = netG(mel)
        fake = fake[..., :wav.shape[-1]]         # match length exactly

        # ---- Discriminator ----
        d_real, _ = netD(wav)
        d_fake, _ = netD(fake.detach())
        loss_d = sum(((dr - 1) ** 2).mean() + (df ** 2).mean() for dr, df in zip(d_real, d_fake))
        optD.zero_grad(); loss_d.backward(); optD.step()

        # ---- Generator ----
        d_fake2, f_fake = netD(fake)
        _, f_real = netD(wav)
        loss_adv = sum(((df - 1) ** 2).mean() for df in d_fake2)
        loss_fm = feature_matching(f_real, f_fake)
        loss_stft = mrstft(fake, wav)
        loss_mel = F.l1_loss(melspec(fake), melspec(wav))
        loss_g = (loss_adv + cfg.lambda_fm * loss_fm
                  + cfg.lambda_stft * loss_stft + cfg.lambda_mel * loss_mel)
        optG.zero_grad(); loss_g.backward(); optG.step()

        if step % log_every == 0:
            ms = (time.time() - t0) / (step + 1) * 1000
            print(f"[{step}/{iters}] D:{loss_d.item():.3f} G:{loss_g.item():.3f} "
                  f"stft:{loss_stft.item():.3f} mel:{loss_mel.item():.3f} "
                  f"adv:{loss_adv.item():.3f} fm:{loss_fm.item():.3f} ({ms:.0f}ms/it)")
        if step > 0 and step % ckpt_every == 0:
            torch.save({"step": step, "G": netG.state_dict(), "cfg": cfg.__dict__},
                       os.path.join(ckpt_dir, f"voc_{step}.pt"))
    torch.save({"step": iters, "G": netG.state_dict(), "cfg": cfg.__dict__},
               os.path.join(ckpt_dir, f"voc_{iters}.pt"))
    print("done.")


# --------------------------------------------------------------------------- #
# Inference — drop-in replacement for mel_unit_to_audio()
# --------------------------------------------------------------------------- #
@torch.no_grad()
def vocode(unit_mel, cfg: VocConfig, netG: Generator, device="cpu"):
    """unit_mel: np.ndarray [n_mels, T] in [-1,1] -> waveform np.float32."""
    x = torch.as_tensor(unit_mel, dtype=torch.float32, device=device)
    if x.dim() == 2:
        x = x.unsqueeze(0)
    wav = netG(x)[0, 0].cpu().numpy()
    return wav


def load_vocoder(path, device="cpu"):
    ck = torch.load(path, map_location=device)
    cfg = VocConfig(**{k: v for k, v in ck["cfg"].items()
                       if k in VocConfig.__dataclass_fields__})
    netG = Generator(cfg).to(device).eval()
    netG.load_state_dict(ck["G"])
    return netG, cfg


# --------------------------------------------------------------------------- #
def main():
    p = argparse.ArgumentParser()
    sub = p.add_subparsers(dest="cmd", required=True)
    pt = sub.add_parser("train")
    pt.add_argument("--data", required=True)
    pt.add_argument("--iters", type=int, default=200000)
    pt.add_argument("--batch", type=int, default=16)
    pt.add_argument("--workers", type=int, default=4)
    sub.add_parser("test")
    args = p.parse_args()

    device = torch.device("cuda" if torch.cuda.is_available() else "cpu")
    cfg = VocConfig()
    if args.cmd == "train":
        train(cfg, args.data, args.iters, args.batch, device, workers=args.workers)
    else:
        _selftest(cfg, device)


def _selftest(cfg, device):
    """Synthetic shape/step check — no audio files required."""
    print("running vocoder self-test...")
    netG = Generator(cfg).to(device)
    netD = MultiPeriodDisc().to(device)
    mrstft = MRSTFTLoss().to(device)
    melspec = MelSpec(cfg).to(device)

    mel = torch.rand(2, cfg.n_mels, cfg.segment_frames, device=device) * 2 - 1
    wav = torch.rand(2, 1, cfg.segment_len, device=device) * 2 - 1
    fake = netG(mel)
    print(f"  G: mel{list(mel.shape)} -> wav{list(fake.shape)} "
          f"(expected [...,{cfg.segment_len}])")
    assert fake.shape[-1] == cfg.segment_len

    d_real, f_real = netD(wav)
    d_fake, f_fake = netD(fake.detach())
    loss_d = sum(((dr - 1) ** 2).mean() + (df ** 2).mean() for dr, df in zip(d_real, d_fake))
    d_fake2, f_fake2 = netD(fake)
    loss_g = (sum(((df - 1) ** 2).mean() for df in d_fake2)
              + cfg.lambda_fm * feature_matching(f_real, f_fake2)
              + cfg.lambda_stft * mrstft(fake, wav)
              + cfg.lambda_mel * F.l1_loss(melspec(fake), melspec(wav)))
    loss_d.backward(); loss_g.backward()
    n_params = sum(p.numel() for p in netG.parameters())
    print(f"  D loss {loss_d.item():.3f}, G loss {loss_g.item():.3f}, grads OK")
    print(f"  generator params: {n_params/1e6:.2f}M")

    y = vocode((np.random.rand(cfg.n_mels, cfg.segment_frames) * 2 - 1).astype(np.float32),
               cfg, netG.eval(), device)
    print(f"  vocode(): unit-mel -> waveform {y.shape} (expected ({cfg.segment_len},))")
    assert y.shape == (cfg.segment_len,)
    print("SELF-TEST PASSED")


if __name__ == "__main__":
    main()
