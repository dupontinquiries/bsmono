"""beast_sfx -- the jungle's big beasts, grown out of a real snore.

    python3 beast_sfx.py SRC.opus [AUDIO_ROOT]

SRC is snoring-beast-sounds-1 (156 s, a sleeper recorded at night; not in
the repo). Speech recognition with a voice-activity gate found no speech in
it anywhere, so every window below is breath and throat only. Three kinds
of sound are in it:

* 53-60 s and on through the file: slow in-breath snores, a clean buzz at
  ~180 Hz with formants at 500-700 -- a purr, really;
* 66.7-69.0 s: one heavy rattling snore, broadband under 300 Hz, the
  soft palate flapping -- this is the growl;
* 150-153 s: the sleeper stirring, short hard snorts.

How a person's snore becomes a big animal's (the first version pitched it
down with a phase vocoder, vocoded a synthetic pulse-train growl under it
and put it in a comb-filter echo; it read as a processed person):

* varispeed, not pitch-shifting: the sound is slowed like tape, pitch,
  formants and time together. Everything in it -- the flutter of the
  palate, the breath noise, the room -- scales as if the throat were that
  much longer, and there is no phase-vocoder smear, which on noisy
  material is what sounded synthetic.
* a second, slower copy a little behind the first, quieter: two resonant
  cavities (throat and chest) instead of one.
* soft saturation on the low band only: the rattle gets weight without
  fizz on top.
* a convolution reverb with a synthesised cave response (early
  reflections, then a dark exponential tail) in place of aecho's evenly
  spaced repeats, which are what a comb filter sounds like.
* nothing synthetic is layered in: every sound in the output is the
  sleeper's.

    wake   BossSpawn        the rattling snore, at 0.62 speed
    fall   BossDown         a purring in-breath, at 0.5, fading out
    snort  TitanVulnerable  a waking snort, at 0.7
"""
import os
import subprocess
import sys

import numpy as np
import soundfile as sf
from scipy.signal import butter, fftconvolve, resample_poly, sosfilt

import baby_clips as C

SR = 48000
HERE = os.path.dirname(os.path.abspath(__file__))

# name: (from_s, to_s, speed, chest_speed, chest_db, drive, lowpass Hz, cave seconds)
# (wake ends at 68.86: at 68.9 the sleeper's bed creaks, a click)
SOUNDS = {
    'wake': (66.55, 68.86, 0.62, 0.52, -7, 2.5, 3500, 1.6),
    'fall': (54.35, 56.20, 0.50, 0.43, -8, 1.8, 1300, 2.2),
    'snort': (152.30, 152.95, 0.70, 0.60, -6, 3.0, 4500, 1.2),
}
# a stretch between breaths: the room, the fridge, nothing of the sleeper
ROOM_TONE = (60.3, 61.9)


def sos(kind, f, order=4):
    return butter(order, f, btype=kind, fs=SR, output='sos')


def denoise(x, tone):
    """Spectral subtraction of the room tone, 2x, floored at -24 dB."""
    X = C._stft(x)
    M = np.abs(X)
    g = np.maximum(1.0 - 2.0 * tone[None, :] / np.maximum(M, 1e-9), 0.063)
    g = np.pad(g, ((1, 1), (0, 0)), mode='edge')
    g = (g[:-2] + g[1:-1] + g[2:]) / 3.0
    return C._istft(X * g, len(x))


def declick(x):
    """The bed and the sheets tick under the snore (67.05, 67.30 in the
    wake window): short broadband spikes. Above 1.5 kHz, hold each 5 ms
    to within 6 dB of the band's running median; below, a tick carries
    little and the snore everything, so leave it."""
    lo = sosfilt(sos('low', 1500), x)
    hi = x - lo
    h = 240
    e = np.sqrt(np.convolve(hi * hi, np.ones(h) / h, mode='same')) + 1e-9
    med = np.array([np.median(e[max(0, i - 4800):i + 4800:h]) for i in range(0, len(e), h)])
    med = np.repeat(med, h)[:len(e)]
    g = np.minimum(1.0, 2.0 * med / e)
    g = np.convolve(g, np.ones(h) / h, mode='same')
    return (lo + hi * g).astype(np.float32)


def varispeed(x, speed):
    """Slowed like tape: `speed` < 1 lowers pitch, formants and tempo
    together (resampled up, then played at the original rate)."""
    up = int(round(100 / speed))
    return resample_poly(x, up, 100).astype(np.float32)


def cave(seconds, seed=5):
    """A cave's impulse response: a few strong early reflections off near
    rock, then a dense tail decaying 60 dB over `seconds`, losing its
    highs as it goes (stone and air absorb the top first)."""
    rng = np.random.default_rng(seed)
    n = int(seconds * SR)
    t = np.arange(n) / SR
    tail = rng.normal(0, 1, n).astype(np.float32) * np.exp(-6.9 * t / seconds)
    # darken the tail over time: blend a low-passed copy in as it decays
    dark = sosfilt(sos('low', 900, 2), tail)
    mix = np.clip(t / (0.6 * seconds), 0, 1)
    tail = (1 - mix) * sosfilt(sos('low', 5000, 2), tail) + mix * dark
    tail *= np.clip(t / 0.02, 0, 1)  # the tail builds after the first bounces
    ir = 0.35 * tail
    for ms, g in ((0, 1.0), (11, 0.5), (19, -0.35), (29, 0.3), (43, -0.22), (61, 0.18)):
        ir[int(ms * SR / 1000)] += g
    return ir.astype(np.float32)


def make(full, tone, a, b, speed, chest_speed, chest_db, drive, lp, cave_s):
    pre = 0.1
    x = full[int((a - pre) * SR):int((b + pre) * SR)].astype(np.float32)
    x = declick(denoise(x, tone)[int(pre * SR):][:int((b - a) * SR)])
    x = sosfilt(sos('band', [35, 9000]), x).astype(np.float32)
    # fade the cut edges before slowing: an abrupt end is a click, and
    # each slowed copy would put its own click where it ends
    fi, fo = int(0.02 * SR), int(0.12 * SR)
    x[:fi] *= np.linspace(0, 1, fi)
    x[-fo:] *= np.linspace(1, 0, fo)
    throat = varispeed(x, speed)
    chest = varispeed(x, chest_speed)
    lag = int(0.018 * SR)
    n = max(len(throat), len(chest) + lag)
    y = np.zeros(n, np.float32)
    y[:len(throat)] += throat
    y[lag:lag + len(chest)] += chest * 10 ** (chest_db / 20) * (np.abs(throat).max() / (np.abs(chest).max() + 1e-9))
    # weight in the low band only: saturate under 400 Hz, keep the rest clean
    low = sosfilt(sos('low', 400), y)
    rest = y - low
    pk = np.abs(low).max() + 1e-9
    low = np.tanh(drive * low / pk) / np.tanh(drive) * pk
    y = sosfilt(sos('low', lp, 4), low + rest).astype(np.float32)
    # the cave: mostly the beast, the rock behind it
    wet = fftconvolve(y, cave(cave_s))[:len(y) + int(cave_s * SR)]
    dry = np.concatenate([y, np.zeros(len(wet) - len(y), np.float32)])
    y = dry + 0.28 * wet * (np.abs(dry).max() / (np.abs(wet).max() + 1e-9))
    # trim the tail once it is 50 dB down
    e = np.sqrt(np.convolve(y * y, np.ones(960) / 960, mode='same'))
    last = np.where(e > e.max() * 10 ** (-50 / 20))[0]
    y = y[:last[-1] + 1] if len(last) else y
    fi, fo = int(0.01 * SR), int(0.15 * SR)
    y[:fi] *= np.linspace(0, 1, fi)
    y[-fo:] *= np.linspace(1, 0, fo)
    return (y * 0.89 / (np.abs(y).max() + 1e-9)).astype(np.float32)


def main():
    src = sys.argv[1]
    root = sys.argv[2] if len(sys.argv) > 2 else os.path.join(HERE, '..', '..', 'assets', 'audio')
    wav = subprocess.run(['ffmpeg', '-v', 'error', '-i', src, '-f', 'f32le', '-ac', '1', '-ar', str(SR), '-'],
                         check=True, capture_output=True).stdout
    full = np.frombuffer(wav, np.float32)
    tone = C.room_tone(full[int(ROOM_TONE[0] * SR):int(ROOM_TONE[1] * SR)])
    out = os.path.join(root, 'sfx', 'beast')
    os.makedirs(out, exist_ok=True)
    for name, args in SOUNDS.items():
        y = make(full, tone, *args)
        sf.write(os.path.join(out, name + '.opus'), y, SR, format='OGG', subtype='OPUS')
        print(name, f'{len(y) / SR:.2f} s')


if __name__ == '__main__':
    main()
