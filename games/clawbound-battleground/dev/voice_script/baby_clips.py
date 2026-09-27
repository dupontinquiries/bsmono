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

The cuts are high-passed, lightly denoised, faded at the ends and peak
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


def main():
    src = sys.argv[1]
    os.makedirs(OUT, exist_ok=True)
    cache = {}
    for name, (s, a, b) in CLIPS.items():
        if s not in cache:
            wav = subprocess.run(['ffmpeg', '-v', 'error', '-i', os.path.join(src, s + '.opus'), '-f', 'f32le', '-ac', '1', '-ar', str(SR), '-'],
                                 check=True, capture_output=True).stdout
            cache[s] = np.frombuffer(wav, np.float32)
        x = cache[s][int((a - 0.05) * SR):int((b + 0.05) * SR)]
        # the room and the adult under it: high-pass (nothing of a baby's
        # voice sits under 200 Hz), a gentle broadband denoise
        p = subprocess.run(['ffmpeg', '-v', 'error', '-f', 'f32le', '-ar', str(SR), '-ac', '1', '-i', '-',
                            '-af', 'highpass=f=220:poles=2,lowpass=f=11000,afftdn=nr=10:nf=-50', '-f', 'f32le', '-'],
                           input=x.tobytes(), check=True, capture_output=True)
        y = np.frombuffer(p.stdout, np.float32).copy()[int(0.05 * SR):]
        y = y[:int((b - a) * SR)]
        fi, fo = int(0.012 * SR), int(0.05 * SR)
        y[:fi] *= np.linspace(0, 1, fi)
        y[-fo:] *= np.linspace(1, 0, fo)
        y *= 0.9 / (np.abs(y).max() + 1e-9)
        sf.write(os.path.join(OUT, name + '.opus'), y, SR, format='OGG', subtype='OPUS')
        print(name, f'{len(y) / SR:.2f} s', flush=True)


if __name__ == '__main__':
    main()
