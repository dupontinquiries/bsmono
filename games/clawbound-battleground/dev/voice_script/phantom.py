"""phantom -- Hollowstep's voice, never quite one voice in one place.

Two halves:

* ``post(y, key)`` -- mixed into each of his mastered lines, after his
  chain (master_take.py's POST). A voice file is mono (the mixer places
  it), so what goes in here is what does not need a place:
    - a REVERSED ECHO: the line's own reverb, played backwards, swelling up
      into the first word -- the room answering before he speaks;
    - DETUNED DOUBLES a little behind him: one a sixth of a tone sharp, one
      a sixth flat (they beat against him and each other), and one a
      tritone below, faint -- the interval furthest from any harmony, the
      antiharmonic under the words.

* ``python3 phantom.py [AUDIO_ROOT]`` -- the phantoms that DO need a place:
  for each of his four abilities, five whispers of the cast line
  (sfx/swarm/hollowstep/<slot>/b<k>.<take>.opus, two takes each), which the
  cues stand round him on a ring turned to a new angle each cast
  (cues::swarm_spot) -- so they pan somewhere different every time. Each
  is the line made strange a different way: reversed; a tritone down,
  slowed; detuned up and smeared into echoes; only its reversed echo; a
  minor second up, thinned to a whisper. Takes differ in their detune and
  delay, drawn at random (seeded).

Reads the mastered lines from AUDIO_ROOT/voice/hollowstep/cast<N>.opus, so
run it after master_take.py.
"""
import os
import subprocess
import sys
import tempfile

import numpy as np
import soundfile as sf
from scipy.signal import butter, fftconvolve, sosfilt

SR = 48000
HERE = os.path.dirname(os.path.abspath(__file__))


def shift(x, semis, tempo=1.0):
    """Pitch-shifted by `semis` (formants kept), optionally slowed."""
    d = tempfile.mkdtemp(prefix='phantom_')
    a, b = os.path.join(d, 'a.wav'), os.path.join(d, 'b.wav')
    sf.write(a, x, SR)
    subprocess.run(['ffmpeg', '-v', 'error', '-y', '-i', a, '-af',
                    f'rubberband=pitch={2 ** (semis / 12):.6f}:tempo={tempo:.4f}:formant=preserved:pitchq=quality', b], check=True)
    y, _ = sf.read(b, dtype='float32')
    return y


def band(x, lo, hi):
    return sosfilt(butter(4, [lo, hi], btype='band', fs=SR, output='sos'), x).astype(np.float32)


def room(seconds, seed):
    """A dark decaying tail: noise under an exponential, low-passed."""
    rng = np.random.default_rng(seed)
    n = int(seconds * SR)
    t = np.arange(n) / SR
    ir = rng.normal(0, 1, n) * np.exp(-6.9 * t / seconds)
    ir = sosfilt(butter(2, 3500, btype='low', fs=SR, output='sos'), ir)
    return (ir / np.abs(ir).max()).astype(np.float32)


def reversed_echo(x, seconds=1.1, seed=1):
    """The line's reverb, backwards: it swells up and ends where the line
    begins. Returned with its length -- the lead-in it needs."""
    wet = fftconvolve(x[::-1], room(seconds, seed))[: len(x) + int(seconds * SR)]
    wet = band(wet[::-1].astype(np.float32), 250, 6000)
    return wet, int(seconds * SR)


def rms(x):
    return float(np.sqrt(np.mean(np.square(x))) + 1e-12)


def at_level(layer, ref, db):
    return layer * (rms(ref) / rms(layer)) * 10 ** (db / 20)


def mix(dst_len, parts):
    out = np.zeros(dst_len, np.float32)
    for off, sig in parts:
        n = min(len(sig), dst_len - off)
        if n > 0:
            out[off:off + n] += sig[:n]
    return out


def post(y, key):
    """His mastered line (mono), with the reversed echo leading in and the
    detuned doubles and the tritone under it (see the module doc)."""
    y = y.astype(np.float32)
    rev, lead = reversed_echo(y, 1.1, seed=sum(key.encode()))
    sharp = shift(y, +0.33)
    flat = shift(y, -0.33)
    tritone = band(shift(y, -6.0), 90, 4000)
    n = lead + len(y) + int(0.5 * SR)
    ms = lambda s: int(s * SR / 1000)
    out = mix(n, [
        (0, at_level(rev, y, -10.0)),
        (lead, y),
        (lead + ms(23), at_level(sharp, y, -13.0)),
        (lead + ms(37), at_level(flat, y, -13.0)),
        (lead + ms(11), at_level(tritone, y, -17.0)),
    ])
    # the reversed echo needs its lead-in; the first 60% of it is a whisper
    # of room that the master's trim would not keep anyway
    return out.astype(np.float32)


# per phantom k: how the cast line is made strange (see the module doc)
def phantom(x, k, rng):
    det = float(rng.uniform(-0.25, 0.25))
    if k == 0:
        y = x[::-1].copy()
        y = band(y, 300, 5000)
    elif k == 1:
        y = band(shift(x, -6.0 + det, tempo=0.8), 80, 3500)
    elif k == 2:
        y = shift(x, 0.5 + det)
        taps = [(0, 1.0)] + [(int(rng.uniform(0.08, 0.35) * SR), 0.55 ** (i + 1)) for i in range(4)]
        y = mix(len(y) + int(0.4 * SR), [(o, y * g) for o, g in taps])
    elif k == 3:
        y, _ = reversed_echo(x, float(rng.uniform(0.8, 1.4)), seed=int(rng.integers(1 << 20)))
    else:
        y = band(shift(x, 1.0 + det), 1500, 9000)
    delay = int(rng.uniform(0.0, 0.25) * SR)
    y = np.concatenate([np.zeros(delay, np.float32), y.astype(np.float32)])
    y *= 0.8 / (np.abs(y).max() + 1e-9)
    fi, fo = int(0.01 * SR), int(0.05 * SR)
    y[:fi] *= np.linspace(0, 1, fi)
    y[-fo:] *= np.linspace(1, 0, fo)
    return y


def main():
    root = sys.argv[1] if len(sys.argv) > 1 else os.path.join(HERE, '..', '..', 'assets', 'audio')
    for slot in range(1, 5):
        src = os.path.join(root, 'voice', 'hollowstep', f'cast{slot}.opus')
        x, _ = sf.read(src, dtype='float32')
        out = os.path.join(root, 'sfx', 'swarm', 'hollowstep', str(slot))
        os.makedirs(out, exist_ok=True)
        for f in os.listdir(out):
            os.remove(os.path.join(out, f))
        for take in (1, 2):
            rng = np.random.default_rng(slot * 10 + take)
            for k in range(5):
                y = phantom(x, k, rng)
                sf.write(os.path.join(out, f'b{k}.{take}.opus'), y, SR, format='OGG', subtype='OPUS')
        print(f'slot {slot}: 5 phantoms x 2 takes', flush=True)


if __name__ == '__main__':
    main()
