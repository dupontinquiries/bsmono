#!/usr/bin/env python3
"""Convert beatspace-gpu mel dumps to wav via Griffin-Lim.

The Rust GPU trainer writes generated mels as raw little-endian f32, row-major
[n_mels, time_frames], values in [-1, 1] (the generator's tanh output). This
reuses the SAME inverse mapping as gan-zai-v5.2.py so audio stays consistent
across the Python and Rust halves.

    python tools/mel2audio.py samples_rs/*.bin
"""
import sys, os, re, glob
import numpy as np, librosa, soundfile as sf

SR, N_FFT, HOP = 22050, 1024, 256
DB_FLOOR, DB_CEIL, GL_ITERS = -80.0, 40.0, 64

def mel2wav(path):
    m = re.search(r"_(\d+)x(\d+)\.bin$", path)
    if not m:
        print("skip (no _HxW in name):", path); return
    h, w = int(m.group(1)), int(m.group(2))
    unit = np.fromfile(path, dtype="<f4").reshape(h, w)
    db = np.clip(unit, -1, 1) * (DB_CEIL - DB_FLOOR) / 2 + (DB_CEIL + DB_FLOOR) / 2
    power = librosa.db_to_power(db, ref=1.0)
    wav = librosa.feature.inverse.mel_to_audio(
        power, sr=SR, n_fft=N_FFT, hop_length=HOP, n_iter=GL_ITERS)
    out = os.path.splitext(path)[0] + ".wav"
    sf.write(out, wav, SR); print("wrote", out)

paths = sys.argv[1:] or glob.glob("samples_rs/*.bin")
if not paths:
    print("no .bin files given or found in samples_rs/")
for p in paths:
    mel2wav(p)
