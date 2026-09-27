"""baby_clips -- a real baby's voice for Sproutkit, cut from home recordings.

    python3 baby_clips.py SRC_DIR

SRC_DIR holds the three source recordings (not in the repo: two of them
are mostly an adult reading and talking, and only the baby is wanted):

    coo.opus     IMG_1383   48 s of cooing and vocal play
    book.opus    IMG_3347   9 min of a picture book read aloud, the baby
                            joining in
    squeal.opus  pipo-screemo  4 s, one squeal and a coo

Each clip below is a window found by pitch (librosa pyin: a baby's voice
sits at 400-1000 Hz here, an adult's at 150-300), a spectrogram look
(long open vowels with steady, widely spaced harmonics, no consonants)
and speech recognition coming back empty or with a one-word guess. None of
the windows was listened to by ear when they were chosen, so a clip may
still hold a breath or a word of the adult under it -- replace a window
here, re-run, and re-master (master_take.py) to fix one.

The cuts are band-passed (220 Hz-11 kHz), cleaned (see clean()), faded at the ends and peak
normalised, and written to baby/<name>.opus, which master_take.py's
sproutkit layers mix into his lines (see sproutkit_post there).
"""
import os
import subprocess
import sys

import numpy as np
import soundfile as sf

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(HERE, 'baby')
SR = 48000

# name: (source, from_s, to_s) -- the role is the name's first word
CLIPS = {
    # the squeal of joy (F0 1.9-2.3 kHz)
    'squeal.1': ('squeal', 1.74, 2.54),
    'squeal.2': ('book', 447.45, 448.25),
    # short, bright, excited "ah!"s (F0 ~440)
    'ah.1': ('book', 336.60, 337.10),
    'ah.2': ('book', 334.40, 335.05),
    'ah.3': ('book', 50.58, 50.88),
    'ah.4': ('book', 52.30, 52.74),
    'ah.5': ('book', 202.00, 202.64),
    'ah.6': ('book', 423.33, 423.97),
    'ah.7': ('book', 229.66, 229.96),
    # longer vocal play, an open "aaah" rising and falling
    'coo.1': ('book', 469.50, 470.66),
    'coo.2': ('book', 546.20, 546.92),
    'coo.3': ('book', 379.28, 380.10),
    'coo.4': ('squeal', 2.66, 3.28),
    'coo.5': ('book', 388.00, 388.95),
    # a run of babble syllables (babble.1: quick even bursts, ~6 a second
    # -- the nearest thing to a laugh in the recordings)
    'babble.1': ('book', 111.25, 112.92),
    'babble.2': ('coo', 28.90, 30.22),
    'babble.3': ('coo', 4.68, 5.95),
    # soft, falling -- sleepy, or a pout
    'fall.1': ('book', 474.90, 475.90),
    'fall.2': ('book', 426.00, 426.52),
}


HOP = 480      # 10 ms: one pyin frame per STFT frame
NFFT = 2048


def _stft(x):
    w = np.hanning(NFFT).astype(np.float32)
    pad = np.concatenate([np.zeros(NFFT // 2, np.float32), x, np.zeros(NFFT, np.float32)])
    n = (len(pad) - NFFT) // HOP + 1
    idx = np.arange(NFFT)[None, :] + HOP * np.arange(n)[:, None]
    return np.fft.rfft(pad[idx] * w, axis=1)


def _istft(X, length):
    w = np.hanning(NFFT).astype(np.float32)
    y = np.fft.irfft(X, n=NFFT, axis=1).astype(np.float32)
    out = np.zeros(HOP * (len(X) - 1) + NFFT, np.float32)
    norm = np.zeros_like(out)
    for i in range(len(X)):
        out[i * HOP:i * HOP + NFFT] += y[i] * w
        norm[i * HOP:i * HOP + NFFT] += w * w
    return (out / np.maximum(norm, 1e-3))[NFFT // 2:NFFT // 2 + length]


def pitch(x):
    """The baby's F0 per 10 ms frame (0: unvoiced), pyin at 16 kHz."""
    import librosa
    y = librosa.resample(x, orig_sr=SR, target_sr=16000)
    f0, vf, _ = librosa.pyin(y, fmin=150, fmax=2600, sr=16000, frame_length=1024, hop_length=160, center=True)
    return np.where(vf, np.nan_to_num(f0), 0.0)


def room_tone(ctx):
    """The room's (and the far adult's) spectrum around a clip: the median
    of the quietest sixth of the frames in the seconds around it."""
    M = np.abs(_stft(ctx))
    e = M.sum(1)
    q = M[e <= np.percentile(e, 16)]
    return np.median(q, 0)


def clean(x, noise):
    """Two passes over the STFT:
    * spectral subtraction of the room tone (over-subtracted 2x, floored at
      -24 dB so nothing goes watery);
    * a harmonic mask where the baby is voiced -- pass a narrow band round
      every harmonic of its F0 up to 11 kHz, hold the rest 20 dB down --
      and 12 dB down where it is not voiced (the gaps are room, not baby).
    The mask is smoothed over three frames against musical noise."""
    X = _stft(x)
    M = np.abs(X)
    g = np.maximum(1.0 - 2.0 * noise[None, :] / np.maximum(M, 1e-9), 0.063)
    f0 = pitch(x)
    n = len(X)
    f0 = np.pad(f0, (0, max(0, n - len(f0))))[:n]
    # hold a voiced frame's pitch across 20 ms dropouts, so an onset or a
    # crack in the voice is not muted
    voiced = f0 > 0
    for k in range(2):
        prev = np.roll(f0, 1); prev[0] = 0
        nxt = np.roll(f0, -1); nxt[-1] = 0
        f0 = np.where(f0 > 0, f0, np.maximum(prev, nxt))
    freqs = np.fft.rfftfreq(NFFT, 1 / SR)
    H = np.full(M.shape, 0.25, np.float32)
    for i in np.where(f0 > 0)[0]:
        h = np.arange(1, int(11000 / f0[i]) + 1) * f0[i]
        sig = np.maximum(35.0, 0.035 * h)
        d = np.min(np.abs(freqs[:, None] - h[None, :]) / sig[None, :], axis=1)
        H[i] = 0.1 + 0.9 * np.exp(-0.5 * d * d)
    G = g * H
    G = np.pad(G, ((1, 1), (0, 0)), mode='edge')
    G = (G[:-2] + G[1:-1] + G[2:]) / 3.0
    return _istft(X * G, len(x)), float(voiced.mean())


def main():
    src = sys.argv[1]
    os.makedirs(OUT, exist_ok=True)
    cache = {}
    for name, (s, a, b) in CLIPS.items():
        if s not in cache:
            wav = subprocess.run(['ffmpeg', '-v', 'error', '-i', os.path.join(src, s + '.opus'), '-f', 'f32le', '-ac', '1', '-ar', str(SR), '-'],
                                 check=True, capture_output=True).stdout
            cache[s] = np.frombuffer(wav, np.float32)
        full = cache[s]
        # nothing of a baby's voice sits under 200 Hz
        from scipy.signal import butter, sosfilt
        hp = sosfilt(butter(4, [220, 11000], btype='band', fs=SR, output='sos'), full).astype(np.float32)
        ctx = hp[max(0, int((a - 3) * SR)):int((b + 3) * SR)]
        x = hp[int((a - 0.05) * SR):int((b + 0.05) * SR)]
        y, v = clean(x, room_tone(ctx))
        y = y[int(0.05 * SR):][:int((b - a) * SR)].copy()
        fi, fo = int(0.012 * SR), int(0.05 * SR)
        y[:fi] *= np.linspace(0, 1, fi)
        y[-fo:] *= np.linspace(1, 0, fo)
        y *= 0.9 / (np.abs(y).max() + 1e-9)
        sf.write(os.path.join(OUT, name + '.opus'), y, SR, format='OGG', subtype='OPUS')
        print(name, f'{len(y) / SR:.2f} s, voiced {v:.0%}', flush=True)


if __name__ == '__main__':
    main()
