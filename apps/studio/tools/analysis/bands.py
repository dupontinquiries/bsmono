#!/usr/bin/env python3
"""Octave-band RMS (dB) of an audio file or a slice, numpy + one ffmpeg decode.
usage: bands.py FILE [start_s dur_s] [--raw]   (--raw: 48 kHz stereo f32le)"""
import sys, subprocess, numpy as np
a = [x for x in sys.argv[1:] if x != "--raw"]; raw = "--raw" in sys.argv
f = a[0]; ss = a[1] if len(a) > 1 else None; t = a[2] if len(a) > 2 else None
cmd = ["ffmpeg", "-nostdin", "-v", "error"]
if raw: cmd += ["-f", "f32le", "-ar", "48000", "-ac", "2"]
if ss: cmd += ["-ss", ss]
if t: cmd += ["-t", t]
cmd += ["-i", f, "-ac", "1", "-ar", "48000", "-f", "f32le", "-"]
x = np.frombuffer(subprocess.run(cmd, capture_output=True).stdout, dtype=np.float32).astype(np.float64)
X = np.abs(np.fft.rfft(x)) ** 2 / len(x) ** 2 * 2           # power per bin (Parseval, one-sided)
fr = np.fft.rfftfreq(len(x), 1 / 48000)
out = []
for c in [63, 125, 250, 500, 1000, 2000, 4000, 8000]:
    m = (fr >= c / 2 ** 0.5) & (fr < c * 2 ** 0.5)
    out.append(10 * np.log10(max(X[m].sum(), 1e-20)))
print(" ".join(f"{v:6.1f}" for v in out))
