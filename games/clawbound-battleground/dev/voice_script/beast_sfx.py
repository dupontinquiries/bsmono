"""beast_sfx -- the jungle's big beasts, grown out of a real snore.

    python3 beast_sfx.py SRC [AUDIO_ROOT] [--cry=take48.wav]

SRC is snoring_teeth_grinding.m4a (241 s, a sleeper snoring and grinding
their teeth; not in the repo). It replaced an earlier, quieter snore
recording, whose snores were thin in-breaths that could not be made to
sound like anything but a person. Every ~4 s there is a snore of about
1.5 s with a clear harmonic rattle at 100-700 Hz. There are also:

* two voiced grunts. Speech recognition hears both as "Uh"; there are no
  words anywhere in the file.
* the loudest event, at 5.5 s: a hard snort that runs straight into a
  snore.
* teeth grinding at 221-238 s. It is unused for now, because no event
  wants a chewing sound yet.

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

The sounds:

    wake   BossSpawn        the snort and the snore after it, at 0.62 speed
    fall   BossDown         a long rattling snore, at 0.52, fading out
    snort  TitanVulnerable  the second grunt, cut off hard, at 0.66
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
# (cave seconds here are unused by `make`; DRESS carries the cave)
SOUNDS = {
    'wake': (5.45, 7.10, 0.62, 0.52, -7, 2.2, 3800, 1.6),
    'fall': (76.15, 77.95, 0.52, 0.44, -8, 1.8, 2400, 2.2),
    'snort': (178.85, 179.62, 0.66, 0.56, -6, 2.6, 4200, 1.2),
}
# a stretch between snores: the room and nothing of the sleeper
ROOM_TONE = (21.0, 23.5)

# The war cry: the word "ROAR" from Goldenmane's "Hear me ROAR!" in the
# voice session (take48.wav, 53.95-55.65 s -- the performer roaring the
# vowel, no consonant to speak of once it is slowed). Optional: without the
# session the beasts are built without it.
CRY = (53.95, 55.65)
CRY_TONE = (131.0, 132.5)  # the session's room between lines

# per sound: air (dB of fizz under the beast), shriek (tape speed, dB),
# host (cry voices' speeds, spread s, start s, dB), keen (a lone cry
# slowing from speed to speed, start s, dB), cave seconds
DRESS = {
    # the boss rises: a screech on the snort, then its war band roars
    'wake': {'air': -9, 'shriek': (2.0, -10), 'host': ([0.66, 0.74, 0.83, 0.93, 1.05], 0.16, 0.45, -3), 'cave': 1.6},
    # the boss falls: a last cry winding down under the dying breath
    'fall': {'air': -13, 'keen': (0.8, 0.5, 0.15, -5), 'cave': 2.2},
    # a titan's guard is down: a snort and a short, ragged shout
    'snort': {'air': -8, 'shriek': (1.8, -8), 'host': ([0.8, 0.9, 1.0], 0.1, 0.2, -5), 'cave': 1.2},
}


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
    """The beast itself, dry (the cave goes on in `finish`)."""
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
    return y


def excite(y, level_db):
    """Air on top: the 1.2-4 kHz band driven hard (its harmonics land at
    3-12 kHz), kept only above 3 kHz, mixed in at `level_db` under the
    beast. A slowed sound has lost its top octave; this puts a rasp back
    without pulling the body up with it."""
    mid = sosfilt(sos('band', [1200, 4000]), y)
    pk = np.abs(mid).max() + 1e-9
    fizz = sosfilt(sos('high', 3000), np.tanh(4.0 * mid / pk) * pk)
    fizz = sosfilt(sos('low', 12000, 2), fizz)
    return (y + fizz * rms(y) / rms(fizz) * 10 ** (level_db / 20)).astype(np.float32)


def rms(x):
    return float(np.sqrt(np.mean(np.square(x))) + 1e-12)


def ramp_speed(x, s0, s1):
    """Tape slowing from `s0` to `s1` across the clip: a cry winding down."""
    n_out = int(len(x) / ((s0 + s1) / 2))
    sp = np.linspace(s0, s1, n_out)
    pos = np.cumsum(sp)
    pos = pos[pos < len(x) - 1]
    return np.interp(pos, np.arange(len(x)), x).astype(np.float32)


def host(cry, voices, spread, drive=2.0, seed=11):
    """A war band behind the beast: the cry at several tape speeds (a big
    throat and smaller ones), each a little late, each a little rough."""
    rng = np.random.default_rng(seed)
    parts = []
    for k, sp in enumerate(voices):
        v = varispeed(cry, sp)
        pk = np.abs(v).max() + 1e-9
        v = np.tanh(drive * v / pk) / np.tanh(drive)
        d = 0 if k == 0 else int(rng.uniform(0.02, spread) * SR)
        parts.append((d, v * rng.uniform(0.7, 1.0)))
    n = max(d + len(v) for d, v in parts)
    out = np.zeros(n, np.float32)
    for d, v in parts:
        out[d:d + len(v)] += v
    return out


def at_(base, layer, start_s, level_db):
    """`layer` mixed into `base` from `start_s`, `level_db` against its RMS."""
    i = int(start_s * SR)
    n = max(len(base), i + len(layer))
    out = np.zeros(n, np.float32)
    out[:len(base)] += base
    out[i:i + len(layer)] += layer * rms(base) / rms(layer) * 10 ** (level_db / 20)
    return out


def finish(y, cave_s):
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


def raw(full, a, b):
    x = full[int(a * SR):int(b * SR)].astype(np.float32).copy()
    fi, fo = int(0.02 * SR), int(0.1 * SR)
    x[:fi] *= np.linspace(0, 1, fi)
    x[-fo:] *= np.linspace(1, 0, fo)
    return x


def dress(name, beast, snore_src, cry):
    """Each sound's top end and war cry (see DRESS)."""
    d = DRESS[name]
    y = excite(beast, d['air'])
    if d.get('shriek'):
        # the same snort at double speed and above 1.8 kHz: a screech
        # riding the front of it
        sp, lvl = d['shriek']
        sh = sosfilt(sos('high', 1800), varispeed(snore_src, sp)).astype(np.float32)
        y = at_(y, sh, 0.0, lvl)
    if cry is not None and d.get('host'):
        voices, spread, start, lvl = d['host']
        y = at_(y, excite(host(cry, voices, spread), -8), start, lvl)
    if cry is not None and d.get('keen'):
        s0, s1, start, lvl = d['keen']
        k = excite(ramp_speed(cry, s0, s1), -10)
        y = at_(y, k, start, lvl)
    return finish(y, d['cave'])


def load(path):
    wav = subprocess.run(['ffmpeg', '-v', 'error', '-i', path, '-f', 'f32le', '-ac', '1', '-ar', str(SR), '-'],
                         check=True, capture_output=True).stdout
    return np.frombuffer(wav, np.float32)


def main():
    args = [a for a in sys.argv[1:] if not a.startswith('--cry=')]
    cries = [a[len('--cry='):] for a in sys.argv[1:] if a.startswith('--cry=')]
    src = args[0]
    root = args[1] if len(args) > 1 else os.path.join(HERE, '..', '..', 'assets', 'audio')
    full = load(src)
    tone = C.room_tone(full[int(ROOM_TONE[0] * SR):int(ROOM_TONE[1] * SR)])
    cry = None
    if cries:
        take = load(cries[0])
        ct = C.room_tone(take[int(CRY_TONE[0] * SR):int(CRY_TONE[1] * SR)])
        cry = denoise(raw(take, CRY[0] - 0.1, CRY[1] + 0.1), ct)
        cry = sosfilt(sos('band', [90, 10000]), cry).astype(np.float32)
    out = os.path.join(root, 'sfx', 'beast')
    os.makedirs(out, exist_ok=True)
    for name, spec in SOUNDS.items():
        beast = make(full, tone, *spec)
        y = dress(name, beast, raw(full, spec[0], spec[1]), cry)
        sf.write(os.path.join(out, name + '.opus'), y, SR, format='OGG', subtype='OPUS')
        print(name, f'{len(y) / SR:.2f} s', 'with the war cry' if cry is not None else '')


if __name__ == '__main__':
    main()
