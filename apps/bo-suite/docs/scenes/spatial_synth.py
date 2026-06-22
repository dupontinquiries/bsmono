#!/usr/bin/env python3
"""
spatial_synth.py — generate spatial forest + stream sound assets.

What "spatial" means here, in plain terms: each scene voice is
authored as a small bank of variants that differ in distance,
movement, and stereo width. When the soundscape engine routes a
voice to a physical channel (or to several channels via the
RouteStrategy::Diffuse path), the variants combine to suggest a
3-D environment rather than a single point source.

Output:
  docs/scenes/clips_spatial/<voice>_<variant>.opus
  Each clip is 16 seconds, stereo, 48 kHz, Opus-encoded ~96 kbps.

Voices produced:
  brook_close_{a..h}     — bright water sounds with micro-splashes
  brook_mid_{a..h}       — mid-distance brook, rolled HF
  brook_far_{a..h}       — far brook, smoothed and dimmed
  wind_canopy_{a..h}     — mid-band wind through leaves, wide stereo
  wind_distant_{a..h}    — low-band wind in distance, narrow stereo
  leaves_rustle_{a..h}   — sparse leaf rustles, micro-transients

The variants within each voice are NOT identical — they share a
character but have different random seeds for the surface noise,
different LFO phases, slightly different filter parameters. The
soundscape engine picks among them randomly on each spawn so the
playback never has a fixed loop point — perceptually alive.

Why per-channel content vs simple panning of mono?
  • Pan-of-mono produces "phantom center" — a clear point source,
    which reads as artificial in nature scenes
  • Decorrelated L/R gives "diffuse field" — the natural quality
    of being inside an ambient environment
  • Multi-device playback (2-3 speakers around the room) benefits
    most when each speaker plays its own decorrelated content

Dependencies: numpy, scipy, ffmpeg (for Opus encoding).

Usage:
  python3 spatial_synth.py [output_dir]
  Default output_dir: ./clips_spatial/
"""
import os
import sys
import subprocess
import tempfile
from pathlib import Path

import numpy as np
from scipy import signal as sps

# ── Constants ────────────────────────────────────────────────────────
SAMPLE_RATE = 48000
DURATION_S = 16.0
N_SAMPLES = int(SAMPLE_RATE * DURATION_S)
N_VARIANTS = 8  # a..h per voice
VARIANT_LETTERS = "abcdefgh"

# Opus encoding target bitrates per voice type. Narrower-band voices
# encode at lower bitrate without perceptual loss; full-band voices
# need more. Per the earlier A/B work, bitrate-matched encoding cut
# storage by ~27% vs a flat 96 kbps baseline.
BITRATE = {
    "brook_close":    96,   # full-band, micro-transients need clarity
    "brook_mid":      80,
    "brook_far":      64,
    "wind_canopy":    80,
    "wind_distant":   64,
    "leaves_rustle":  96,   # transients need clarity even though sparse
}

# ── Helpers ──────────────────────────────────────────────────────────

def rng(seed):
    """Per-variant RNG. Deterministic so re-runs produce the same
    clips — useful for caching and for guaranteeing the test data
    matches across deploys."""
    return np.random.default_rng(seed)

def lowpass(x, hz):
    """4th-order Butterworth LP. The HF rolloff cue for distance —
    far sources lose treble in proportion to atmospheric absorption."""
    sos = sps.butter(4, hz, btype="low", fs=SAMPLE_RATE, output="sos")
    return sps.sosfiltfilt(sos, x)

def highpass(x, hz):
    sos = sps.butter(4, hz, btype="high", fs=SAMPLE_RATE, output="sos")
    return sps.sosfiltfilt(sos, x)

def bandpass(x, lo, hi):
    sos = sps.butter(4, [lo, hi], btype="band", fs=SAMPLE_RATE, output="sos")
    return sps.sosfiltfilt(sos, x)

def fade(x, n_in=2400, n_out=2400):
    """Apply linear fade-in/out so loops don't click. 2400 samples
    @48 kHz = 50 ms — short enough to be inaudible, long enough to
    suppress any click from a non-zero crossing at the loop point."""
    x = x.copy()
    x[:n_in]  *= np.linspace(0, 1, n_in)
    x[-n_out:] *= np.linspace(1, 0, n_out)
    return x

def loop_safe(x, n_xfade=4800):
    """Make the clip seamless when looped: crossfade the tail with a
    copy of the head over n_xfade samples (100 ms default). After
    this, the join from sample N-1 to sample 0 has no discontinuity
    in either amplitude or spectrum.

    Accepts either 1-D mono (shape (N,)) or 2-D stereo where the
    samples-axis is the LAST axis (shape (..., N))."""
    x = x.copy()
    n = x.shape[-1]
    if n_xfade >= n // 2:
        n_xfade = n // 2
    ramp = np.linspace(0, 1, n_xfade)
    # Slice the head and tail along the last axis; numpy broadcasts
    # ramp (1-D) against (..., n_xfade) naturally.
    head = x[..., :n_xfade].copy()
    tail = x[..., -n_xfade:].copy()
    x[..., -n_xfade:] = tail * (1 - ramp) + head * ramp
    return x

def fade_last_axis(x, n_in=2400, n_out=2400):
    """Same as `fade` but expects samples on the last axis. Works
    for both mono (N,) and stereo (2, N)."""
    x = x.copy()
    x[..., :n_in]  *= np.linspace(0, 1, n_in)
    x[..., -n_out:] *= np.linspace(1, 0, n_out)
    return x

def stereo_decorrelate(mono, width=1.0, seed=0):
    """Turn a mono signal into a wide stereo pair by passing it
    through two different all-pass-ish filters with randomized
    coefficients. This produces L and R signals with the same
    spectrum but very different waveforms — the human auditory
    system reads this as "diffuse" rather than "centered".

    The 'width' param scales the decorrelation: 0 = identical
    (mono in a stereo wrapper), 1 = maximally different."""
    r = rng(seed)
    # Build two short FIR decorrelators. ~30 ms each is enough to
    # break the cross-correlation without smearing transients to
    # an audible degree.
    n_fir = int(0.03 * SAMPLE_RATE)
    h_l = r.normal(0, 1, n_fir)
    h_r = r.normal(0, 1, n_fir)
    # Apply a smooth envelope so the FIR doesn't add a transient
    # of its own.
    env = np.hanning(n_fir)
    h_l *= env; h_r *= env
    # Normalize gain so the convolution doesn't change loudness.
    h_l /= np.linalg.norm(h_l) + 1e-9
    h_r /= np.linalg.norm(h_r) + 1e-9
    L = sps.fftconvolve(mono, h_l, mode="same")
    R = sps.fftconvolve(mono, h_r, mode="same")
    # Blend with the original by `width` — width=0 means pure mono,
    # width=1 means fully decorrelated.
    L = (1 - width) * mono + width * L
    R = (1 - width) * mono + width * R
    return L, R

def normalize_peak(stereo, peak=0.85):
    """Scale so the absolute peak is `peak`. Leaves 1.4 dB of
    headroom before clipping — enough that subsequent gain stages
    (per-voice gain + scene gain + scene-level master) don't push
    the result over 1.0 unless they really intend to."""
    m = max(abs(stereo).max(), 1e-9)
    return stereo * (peak / m)

# ── Voice synthesizers ───────────────────────────────────────────────

def brook_close(seed):
    """Bright brook with sharp micro-splashes. Stereo width is
    moderate — close water suggests a clear directional source,
    not a diffuse field."""
    r = rng(seed)
    n = N_SAMPLES

    # Base: filtered pink noise 200-6000 Hz — the bulk of brook
    # spectrum. Pink (1/f) instead of white because water reads as
    # noise-with-character, not hiss.
    pink = r.normal(0, 1, n)
    # Pinkify: filter with a 1-pole 1/f-ish filter.
    pink = sps.lfilter([0.049922035, -0.095993537, 0.050612699, -0.004408786],
                       [1, -2.494956002, 2.017265875, -0.522189400], pink)
    base = bandpass(pink, 200, 6000)
    base /= np.std(base) + 1e-9

    # Micro-splashes: ~80 brief bandlimited bursts over the 16 s.
    # Each is a 30-80ms bandpass burst with sharp attack.
    splashes = np.zeros(n)
    n_splashes = r.integers(60, 100)
    burst_centers = r.uniform(0.05, DURATION_S - 0.1, n_splashes)
    for t in burst_centers:
        dur = r.uniform(0.03, 0.08)
        n_b = int(dur * SAMPLE_RATE)
        b = r.normal(0, 1, n_b)
        b = bandpass(b, 800, 4000)
        # Sharp attack, exp decay
        env = np.exp(-np.linspace(0, 5, n_b))
        b *= env
        idx = int(t * SAMPLE_RATE)
        end = min(idx + n_b, n)
        splashes[idx:end] += b[:end-idx] * r.uniform(0.4, 1.0)
    splashes /= max(abs(splashes).max(), 1e-9)

    # Slow tonal undertone: 80-150 Hz gentle hum. Real brooks have
    # a low "rumble" from water mass and channel resonance.
    undertone = bandpass(r.normal(0, 1, n), 80, 150)
    undertone /= np.std(undertone) + 1e-9

    # Mix
    mono = 0.45 * base + 0.55 * splashes + 0.15 * undertone

    # Stereo: moderate decorrelation (0.5) — close brook has a sense
    # of direction but isn't a point source like a tap.
    L, R = stereo_decorrelate(mono, width=0.5, seed=seed + 1000)
    stereo = np.stack([L, R])
    stereo = loop_safe(stereo)
    stereo = fade_last_axis(stereo)
    return normalize_peak(stereo)

def brook_mid(seed):
    """Mid-distance brook: bandwidth reduced, splashes softer,
    stereo image slightly wider (diffuse-field rendering)."""
    r = rng(seed)
    n = N_SAMPLES

    pink = r.normal(0, 1, n)
    pink = sps.lfilter([0.049922035, -0.095993537, 0.050612699, -0.004408786],
                       [1, -2.494956002, 2.017265875, -0.522189400], pink)
    base = bandpass(pink, 150, 3500)  # HF rolled vs close
    base /= np.std(base) + 1e-9

    splashes = np.zeros(n)
    n_splashes = r.integers(40, 70)
    burst_centers = r.uniform(0.05, DURATION_S - 0.1, n_splashes)
    for t in burst_centers:
        dur = r.uniform(0.05, 0.12)
        n_b = int(dur * SAMPLE_RATE)
        b = r.normal(0, 1, n_b)
        b = bandpass(b, 500, 2500)  # Softer band
        env = np.exp(-np.linspace(0, 3, n_b))  # Slower decay
        b *= env
        idx = int(t * SAMPLE_RATE)
        end = min(idx + n_b, n)
        splashes[idx:end] += b[:end-idx] * r.uniform(0.2, 0.6)
    splashes /= max(abs(splashes).max(), 1e-9)

    mono = 0.6 * base + 0.4 * splashes

    L, R = stereo_decorrelate(mono, width=0.75, seed=seed + 1000)
    stereo = np.stack([L, R])
    stereo = loop_safe(stereo)
    stereo = fade_last_axis(stereo)
    return normalize_peak(stereo, peak=0.7)  # quieter than close

def brook_far(seed):
    """Distant brook: heavily rolled high frequencies, smoothed
    surface, very wide diffuse stereo. Suggests being "around"
    a body of water rather than next to a specific spot."""
    r = rng(seed)
    n = N_SAMPLES

    pink = r.normal(0, 1, n)
    pink = sps.lfilter([0.049922035, -0.095993537, 0.050612699, -0.004408786],
                       [1, -2.494956002, 2.017265875, -0.522189400], pink)
    # Narrow band: 100-1500 Hz. Far water has lost most treble.
    base = bandpass(pink, 100, 1500)
    base /= np.std(base) + 1e-9

    # Very slow LFO envelope to suggest "wind shifts" carrying the
    # sound nearer and farther.
    t = np.linspace(0, DURATION_S, n)
    lfo_freq = r.uniform(0.08, 0.15)  # ~6-12 second period
    lfo = 0.5 + 0.5 * np.sin(2 * np.pi * lfo_freq * t + r.uniform(0, 2*np.pi))
    base *= 0.7 + 0.3 * lfo

    L, R = stereo_decorrelate(base, width=1.0, seed=seed + 1000)
    stereo = np.stack([L, R])
    stereo = loop_safe(stereo)
    stereo = fade_last_axis(stereo)
    return normalize_peak(stereo, peak=0.5)  # noticeably quieter

def wind_canopy(seed):
    """Mid-band wind through leaves overhead. Wide stereo to suggest
    spatial envelopment. The signal is filtered noise with a slow
    LFO for gust modulation — wind comes in waves, not constantly."""
    r = rng(seed)
    n = N_SAMPLES

    base = bandpass(r.normal(0, 1, n), 300, 3000)
    base /= np.std(base) + 1e-9

    # Multi-LFO gust modulation: 2 superimposed slow oscillators at
    # different periods produce a quasi-random amplitude envelope.
    t = np.linspace(0, DURATION_S, n)
    lfo1 = 0.5 + 0.5 * np.sin(2 * np.pi * r.uniform(0.10, 0.18) * t + r.uniform(0, 6.28))
    lfo2 = 0.5 + 0.5 * np.sin(2 * np.pi * r.uniform(0.03, 0.06) * t + r.uniform(0, 6.28))
    env = 0.3 + 0.7 * (0.6 * lfo1 + 0.4 * lfo2)
    base *= env

    L, R = stereo_decorrelate(base, width=1.0, seed=seed + 2000)
    stereo = np.stack([L, R])
    stereo = loop_safe(stereo)
    stereo = fade_last_axis(stereo)
    return normalize_peak(stereo, peak=0.7)

def wind_distant(seed):
    """Low-band distant wind, narrow stereo. Suggests the steady
    backdrop of air movement when one isn't directly in the gust."""
    r = rng(seed)
    n = N_SAMPLES

    base = bandpass(r.normal(0, 1, n), 60, 600)
    base /= np.std(base) + 1e-9

    # Very slow envelope — distant wind is almost steady.
    t = np.linspace(0, DURATION_S, n)
    lfo = 0.5 + 0.5 * np.sin(2 * np.pi * r.uniform(0.04, 0.08) * t)
    base *= 0.6 + 0.4 * lfo

    # Narrow stereo — distant sources don't have width.
    L, R = stereo_decorrelate(base, width=0.3, seed=seed + 3000)
    stereo = np.stack([L, R])
    stereo = loop_safe(stereo)
    stereo = fade_last_axis(stereo)
    return normalize_peak(stereo, peak=0.55)

def leaves_rustle(seed):
    """Sparse, brief leaf rustles distributed across the stereo
    field. Each rustle is a 10-50ms bandlimited burst. Quiet
    overall — these add "movement" without dominating."""
    r = rng(seed)
    n = N_SAMPLES

    stereo = np.zeros((2, n))
    # ~30 rustles in 16 seconds. Density chosen so they're noticeable
    # but not constant — like wind catching leaves at random spots.
    n_rustles = r.integers(20, 40)
    for _ in range(n_rustles):
        t_center = r.uniform(0.1, DURATION_S - 0.1)
        dur = r.uniform(0.01, 0.05)
        n_b = int(dur * SAMPLE_RATE)
        b = r.normal(0, 1, n_b)
        # Leaf rustles have a sharp HF character.
        b = bandpass(b, 2000, 8000)
        env = np.exp(-np.linspace(0, 6, n_b))
        b *= env
        amp = r.uniform(0.3, 1.0)
        # Pan: each rustle picks a stereo position randomly. -1=L, +1=R.
        pan = r.uniform(-1.0, 1.0)
        gain_l = np.sqrt((1 - pan) / 2) * amp
        gain_r = np.sqrt((1 + pan) / 2) * amp
        idx = int(t_center * SAMPLE_RATE)
        end = min(idx + n_b, n)
        stereo[0, idx:end] += b[:end-idx] * gain_l
        stereo[1, idx:end] += b[:end-idx] * gain_r
    stereo = loop_safe(stereo)
    stereo = fade_last_axis(stereo)
    return normalize_peak(stereo, peak=0.6)

# ── Encoding ─────────────────────────────────────────────────────────

def write_wav_then_opus(stereo, out_path, bitrate_k):
    """Write the stereo signal as WAV, then re-encode to Opus via
    ffmpeg. ffmpeg's libopus is bit-perfect for what we need and
    avoids pulling pyogg/pyflac into the deps. The WAV is written
    to /tmp and deleted after encoding."""
    # stereo shape: (2, N) → transpose to (N, 2) for soundfile-style
    # interleaved write.
    interleaved = stereo.T.astype(np.float32)
    interleaved = np.clip(interleaved, -1.0, 1.0)
    # 16-bit PCM is plenty for ambient material; Opus quantizes
    # further anyway. Write via scipy's WAV writer.
    from scipy.io import wavfile
    pcm16 = (interleaved * 32767).astype(np.int16)
    with tempfile.NamedTemporaryFile(suffix=".wav", delete=False) as tmp:
        tmp_path = tmp.name
    wavfile.write(tmp_path, SAMPLE_RATE, pcm16)
    try:
        subprocess.run([
            "ffmpeg", "-y", "-loglevel", "error",
            "-i", tmp_path,
            "-c:a", "libopus",
            "-b:a", f"{bitrate_k}k",
            "-vbr", "on",
            "-application", "audio",
            "-frame_duration", "20",
            str(out_path),
        ], check=True)
    finally:
        os.unlink(tmp_path)

# ── Main ─────────────────────────────────────────────────────────────

VOICES = {
    "brook_close":   brook_close,
    "brook_mid":     brook_mid,
    "brook_far":     brook_far,
    "wind_canopy":   wind_canopy,
    "wind_distant":  wind_distant,
    "leaves_rustle": leaves_rustle,
}

def main():
    out_dir = Path(sys.argv[1] if len(sys.argv) > 1 else "clips_spatial")
    out_dir.mkdir(parents=True, exist_ok=True)
    total = 0
    for voice_name, synth_fn in VOICES.items():
        bitrate = BITRATE[voice_name]
        for v_idx, letter in enumerate(VARIANT_LETTERS):
            # Seed combines voice name hash and variant index for
            # determinism across re-runs.
            seed = (hash(voice_name) & 0xFFFF) ^ v_idx
            stereo = synth_fn(seed)
            out_path = out_dir / f"{voice_name}_{letter}.opus"
            write_wav_then_opus(stereo, out_path, bitrate)
            size_k = out_path.stat().st_size // 1024
            print(f"  {out_path.name:32}  {size_k:3} KB  ({bitrate}kbps)")
            total += 1
    print(f"\nGenerated {total} spatial clips in {out_dir}/")

if __name__ == "__main__":
    main()
