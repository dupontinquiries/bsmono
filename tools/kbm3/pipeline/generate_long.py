#!/usr/bin/env python3
"""Generate long-form audio from a trained kBM3 generator by walking a smooth
path through latent space and crossfading consecutive fixed-size windows.

WHY THIS, NOT A BIGGER time_frames
-----------------------------------
The Generator's bottleneck FC layer scales as
    latent * g_filters * (n_mels/16) * (time_frames/16)
Making time_frames cover 30s directly (at sr=48000, hop=512) means a
~184M-parameter FC layer alone -- roughly 10x the rest of the model, and a
~23M-element feature map per batch. That's not a viable path, and it isn't
how real spectrogram-GAN systems produce long audio anyway: they train on a
short window (here, cfg.time_frames ~= 6s) and extend at inference time by
generating several overlapping windows along a smooth latent path and
crossfading the overlaps. That's what this script does.

Usage:
    python generate_long.py --weights checkpoints/gen_20000.pt --seconds 30 --out long.wav
    python generate_long.py --weights checkpoints/gen_20000.pt --seconds 30 \
        --vocoder ../vocoder/vocoder_ckpt/voc_100000.pt  # real vocoder instead of Griffin-Lim
"""
import argparse
import importlib.util

import numpy as np
import soundfile as sf
import torch


def _load_base(path: str):
    spec = importlib.util.spec_from_file_location("kbm3_base", path)
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    return m


def slerp(a: torch.Tensor, b: torch.Tensor, t: float) -> torch.Tensor:
    """Spherical linear interpolation between two latent vectors. Keeps
    interpolated points near the norm the Generator was actually trained on
    (a straight lerp cuts through the low-norm middle of a Gaussian latent
    space, a region the generator rarely saw during training)."""
    a_n, b_n = a / a.norm(), b / b.norm()
    omega = torch.acos(torch.clamp((a_n * b_n).sum(), -1.0, 1.0))
    if omega.abs() < 1e-6:
        return a * (1 - t) + b * t
    so = torch.sin(omega)
    return (torch.sin((1.0 - t) * omega) / so) * a + (torch.sin(t * omega) / so) * b


def latent_path(latent_dim: int, n_windows: int, seed: int | None = None,
                windows_per_keyframe: float = 2.5) -> list[torch.Tensor]:
    """A smooth walk through latent space: slerp between a small number of
    random keyframes, so consecutive windows are close together in latent
    space (and therefore related in character -- tempo, key, timbre) rather
    than independent random draws. Independent draws per window would only
    get smoothed AT the crossfade seam, while the underlying content either
    side of it would still be unrelated -- this makes the whole window, not
    just the seam, flow from its neighbors."""
    n_keyframes = max(2, int(np.ceil(n_windows / windows_per_keyframe)) + 1)
    g = torch.Generator().manual_seed(seed) if seed is not None else None
    keyframes = [torch.randn(latent_dim, generator=g) for _ in range(n_keyframes)]

    path = []
    for i in range(n_windows):
        pos = i / max(1, n_windows - 1) * (n_keyframes - 1)
        k0 = int(np.floor(pos))
        k1 = min(k0 + 1, n_keyframes - 1)
        frac = pos - k0
        path.append(slerp(keyframes[k0], keyframes[k1], frac) if k0 != k1 else keyframes[k0])
    return path


def crossfade(a: np.ndarray, b: np.ndarray, overlap: int) -> np.ndarray:
    """Equal-power (raised-cosine) crossfade: the tail `overlap` samples of a
    and the head `overlap` samples of b are blended; total output length is
    len(a) + len(b) - overlap."""
    if overlap <= 0:
        return np.concatenate([a, b])
    overlap = min(overlap, len(a), len(b))
    fade = np.linspace(0, np.pi / 2, overlap)
    fade_out, fade_in = np.cos(fade) ** 2, np.sin(fade) ** 2
    a_tail = a[-overlap:] * fade_out
    b_head = b[:overlap] * fade_in
    return np.concatenate([a[:-overlap], a_tail + b_head, b[overlap:]])


def plan_windows(total_seconds: float, window_seconds: float, crossfade_seconds: float) -> int:
    """How many fixed-size windows are needed to cover total_seconds once
    consecutive windows overlap by crossfade_seconds."""
    stride = window_seconds - crossfade_seconds
    if stride <= 0:
        raise ValueError(f"crossfade_seconds ({crossfade_seconds}) must be less than "
                         f"the window length ({window_seconds:.2f}s)")
    return max(1, int(np.ceil((total_seconds - crossfade_seconds) / stride)))


def generate_long(base, cfg, netG, device, seconds: float, crossfade_seconds: float,
                  seed: int | None = None, vocoder=None):
    window_seconds = cfg.time_frames * cfg.hop_length / cfg.sr
    n_windows = plan_windows(seconds, window_seconds, crossfade_seconds)
    overlap_samples = int(crossfade_seconds * cfg.sr)

    anchors = latent_path(cfg.latent, n_windows, seed=seed)
    audio_out = None
    netG.eval()
    with torch.no_grad():
        for z in anchors:
            mel = netG(z.unsqueeze(0).to(device))[0, 0].cpu().numpy()
            if vocoder is not None:
                netG_voc, vcfg = vocoder
                wav = __import__("vocoder").vocode(mel, vcfg, netG_voc, device)
            else:
                wav = base.mel_unit_to_audio(mel, cfg)
            audio_out = wav if audio_out is None else crossfade(audio_out, wav, overlap_samples)

    target_len = int(seconds * cfg.sr)
    return audio_out[:target_len] if len(audio_out) > target_len else audio_out


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--weights", required=True, help="path to gen_*.pt (or a combined checkpoint)")
    p.add_argument("--base", default="gan-zai-v5.3.py")
    p.add_argument("--seconds", type=float, default=30.0)
    p.add_argument("--crossfade", type=float, default=1.0, help="seconds of overlap between windows")
    p.add_argument("--vocoder", default=None, help="optional voc_*.pt for real audio instead of Griffin-Lim")
    p.add_argument("--out", default="long.wav")
    p.add_argument("--seed", type=int, default=None)
    args = p.parse_args()

    base = _load_base(args.base)
    cfg = base.Config()
    device = torch.device("cuda" if torch.cuda.is_available() else "cpu")
    netG = base.Generator(cfg).to(device)
    state = torch.load(args.weights, map_location=device)
    if isinstance(state, dict) and "G" in state:
        state = state["G"]
    netG.load_state_dict(state)

    vocoder = None
    if args.vocoder:
        import sys
        sys.path.insert(0, "../vocoder")
        from vocoder import load_vocoder
        vocoder = load_vocoder(args.vocoder, device)

    window_seconds = cfg.time_frames * cfg.hop_length / cfg.sr
    n_windows = plan_windows(args.seconds, window_seconds, args.crossfade)
    print(f"window={window_seconds:.2f}s, crossfade={args.crossfade}s -> "
          f"{n_windows} windows for ~{args.seconds}s "
          f"({'neural vocoder' if vocoder else 'Griffin-Lim'})")

    audio = generate_long(base, cfg, netG, device, args.seconds, args.crossfade,
                          seed=args.seed, vocoder=vocoder)
    sf.write(args.out, audio, cfg.sr)
    print(f"wrote {args.out}: {len(audio)/cfg.sr:.2f}s at {cfg.sr}Hz")


if __name__ == "__main__":
    main()
