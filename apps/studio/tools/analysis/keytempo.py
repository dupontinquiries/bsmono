#!/usr/bin/env python3
"""Tempo and key of an audio file (numpy + ffmpeg).

usage: keytempo.py FILE [FILE ...] [--start S] [--dur D]

TEMPO: autocorrelation of the onset envelope (log-energy flux, 11.6 ms
frames) over 60..190 BPM; the best lag and its half/double are reported
(a tempo detector's octave is always a guess).
KEY: a chroma vector (STFT bins folded to pitch classes, 55 Hz..5 kHz,
weighted by magnitude) correlated with the Krumhansl-Kessler major and minor
profiles in all 12 transpositions; the best three are printed with their
correlation."""
import sys, subprocess, argparse, numpy as np
SR = 22_050
NAMES = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"]
MAJ = np.array([6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88])
MIN = np.array([6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17])

def decode(path, start=None, dur=None):
    cmd = ["ffmpeg", "-nostdin", "-v", "error"] + (["-ss", str(start)] if start else []) + (["-t", str(dur)] if dur else []) + ["-i", path, "-ac", "1", "-ar", str(SR), "-f", "f32le", "-"]
    return np.frombuffer(subprocess.run(cmd, capture_output=True, check=True).stdout, np.float32).astype(np.float64)

def tempo(x):
    hop = 256
    n = len(x) // hop
    e = np.log(np.array([np.sum(x[i * hop:(i + 1) * hop] ** 2) for i in range(n)]) + 1e-10)
    o = np.maximum(np.diff(e, prepend=e[0]), 0); o -= o.mean()
    fps = SR / hop
    ac = np.correlate(o, o, "full")[len(o) - 1:]
    lags = np.arange(len(ac))
    bpm = 60 * fps / np.maximum(lags, 1)
    ok = (bpm >= 60) & (bpm <= 190)
    best = lags[ok][np.argmax(ac[ok])]
    # refine by parabola
    y0, y1, y2 = ac[best - 1], ac[best], ac[best + 1]
    d = 0.5 * (y0 - y2) / (y0 - 2 * y1 + y2) if (y0 - 2 * y1 + y2) != 0 else 0
    return 60 * fps / (best + d)

def key(x):
    nfft, hop = 8192, 4096
    w = np.hanning(nfft)
    f = np.fft.rfftfreq(nfft, 1 / SR)
    sel = (f >= 55) & (f <= 5000)
    pc = (np.round(12 * np.log2(f[sel] / 440.0)) + 9) % 12
    chroma = np.zeros(12)
    for i in range(0, len(x) - nfft, hop):
        m = np.abs(np.fft.rfft(x[i:i + nfft] * w))[sel]
        np.add.at(chroma, pc.astype(int), m)
    chroma /= chroma.sum() + 1e-12
    res = []
    for k in range(12):
        for prof, mode in [(MAJ, "major"), (MIN, "minor")]:
            res.append((np.corrcoef(chroma, np.roll(prof, k))[0, 1], f"{NAMES[k]} {mode}"))
    res.sort(reverse=True)
    return res[:3], chroma

ap = argparse.ArgumentParser(); ap.add_argument("files", nargs="+"); ap.add_argument("--start", type=float); ap.add_argument("--dur", type=float)
a = ap.parse_args()
for fpath in a.files:
    x = decode(fpath, a.start, a.dur)
    t = tempo(x)
    (k, chroma) = key(x)
    print(f"{fpath}\n  tempo {t:6.1f} BPM (or {t / 2:.1f} / {t * 2:.1f})   key: " + ", ".join(f"{n} ({c:.2f})" for c, n in k))
    print("  chroma: " + " ".join(f"{NAMES[i]}:{chroma[i]:.2f}" for i in np.argsort(-chroma)[:7]))
