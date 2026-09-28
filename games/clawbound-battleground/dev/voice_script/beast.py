"""beast -- the animal and the god under each voice (used by master_take.py).

Everything here is synthesised from the performer's own line, so no
sample library is needed and every layer follows the words:

* cross(voice, carrier): a channel vocoder -- the voice's spectral
  envelope (its vowels and consonants) imposed on another sound. With a
  growl carrier the words come out of a throat; with noise, as breath.
* growl_carrier: an irregular glottal pulse train, the way a big cat's
  growl is -- low (30-90 Hz), jittering in period and alternating in
  strength (period doubling), which is what makes it rough, not a buzz.
* loud_gate: how far into its shout the voice is, 0..1, so a layer can
  swell only on the loud words (a roar on "ROAR", a snarl on "bite").
* octave(voice, ratio): the voice shifted with formants kept, for a
  second, bigger (or smaller) self speaking alongside.
* chirp: a caracal's chirrup -- a short rising two-note call.
"""
import os
import subprocess
import tempfile

import numpy as np
import soundfile as sf

SR = 48000
N = 1024
HOP = 256
WIN = np.hanning(N).astype(np.float32)


def stft(x):
    pad = np.concatenate([np.zeros(N, np.float32), x, np.zeros(N, np.float32)])
    frames = (len(pad) - N) // HOP + 1
    idx = np.arange(N)[None, :] + HOP * np.arange(frames)[:, None]
    return np.fft.rfft(pad[idx] * WIN, axis=1)


def istft(X, length):
    frames = X.shape[0]
    out = np.zeros(HOP * (frames - 1) + N, np.float32)
    norm = np.zeros_like(out)
    y = np.fft.irfft(X, n=N, axis=1).astype(np.float32)
    for i in range(frames):
        out[i * HOP:i * HOP + N] += y[i] * WIN
        norm[i * HOP:i * HOP + N] += WIN * WIN
    out /= np.maximum(norm, 1e-6)
    return out[N:N + length]


def _smooth_bins(M, width):
    k = np.ones(width, np.float32) / width
    return np.apply_along_axis(lambda r: np.convolve(r, k, mode='same'), 1, M)


def cross(voice, carrier, width=12):
    """The voice's spectral envelope on the carrier (same length)."""
    V, C = stft(voice), stft(carrier[:len(voice)])
    ev = _smooth_bins(np.abs(V), width)
    ec = _smooth_bins(np.abs(C), width)
    return istft(C * (ev / np.maximum(ec, 1e-6)), len(voice))


def envelope(x, ms=15):
    h = max(1, int(SR * ms / 1000))
    return np.sqrt(np.convolve(x * x, np.ones(h) / h, mode='same'))


def loud_gate(x, below_peak_db=12.0, ms=40):
    """0 where the voice is more than `below_peak_db` under its peak, rising
    to 1 at the peak."""
    e = envelope(x, ms)
    db = 20 * np.log10(e / (e.max() + 1e-9) + 1e-9)
    return np.clip((db + below_peak_db) / below_peak_db, 0.0, 1.0).astype(np.float32)


def growl_carrier(n, f0=55.0, jitter=0.18, double=0.35, seed=1):
    """An irregular pulse train (see the module doc)."""
    rng = np.random.default_rng(seed)
    out = np.zeros(n, np.float32)
    t, k = 0.0, 0
    # a slow wander of the base pitch, and per-period jitter on top
    wander = np.cumsum(rng.normal(0, 0.02, n // 480 + 2))
    while t < n:
        f = f0 * np.exp(0.15 * np.tanh(wander[min(int(t) // 480, len(wander) - 1)]))
        period = SR / f * (1.0 + jitter * rng.normal())
        amp = 1.0 - double * (k % 2)
        i = int(t)
        # a glottal-ish pulse: a sharp rise, an exponential fall
        L = min(int(period), n - i)
        if L > 0:
            u = np.arange(L, dtype=np.float32)
            out[i:i + L] += amp * np.exp(-u / (0.18 * period)) * (1 - np.exp(-u / 6.0))
        t += max(period, 20)
        k += 1
    out -= np.convolve(out, np.ones(480) / 480, mode='same')
    return out / (np.abs(out).max() + 1e-9)


def noise(n, seed=2):
    return np.random.default_rng(seed).normal(0, 0.3, n).astype(np.float32)


def bandpass(x, lo, hi, order=4):
    from scipy.signal import butter, sosfilt
    sos = butter(order, [lo, hi], btype='band', fs=SR, output='sos')
    return sosfilt(sos, x).astype(np.float32)


def lowpass(x, f, order=4):
    from scipy.signal import butter, sosfilt
    return sosfilt(butter(order, f, btype='low', fs=SR, output='sos'), x).astype(np.float32)


def octave(x, ratio):
    """The voice shifted by `ratio` with its formants kept (rubberband)."""
    d = tempfile.mkdtemp(prefix='beast_')
    a, b = os.path.join(d, 'a.wav'), os.path.join(d, 'b.wav')
    sf.write(a, x, SR)
    subprocess.run(['ffmpeg', '-v', 'error', '-y', '-i', a, '-af', f'rubberband=pitch={ratio}:formant=preserved', b], check=True)
    y, _ = sf.read(b, dtype='float32')
    y = y[:len(x)] if len(y) >= len(x) else np.pad(y, (0, len(x) - len(y)))
    return y


def rms(x):
    return float(np.sqrt(np.mean(x * x)) + 1e-9)


def at(level_db, layer, ref):
    """`layer` scaled to sit `level_db` relative to `ref`'s RMS."""
    return layer * (rms(ref) / rms(layer)) * 10 ** (level_db / 20)


def giggle(n_hee=4, seed=7):
    """A kitten's giggle: short bright "hee"s, each a little lower and
    shaken fast, with breath in them."""
    rng = np.random.default_rng(seed)
    out = []
    for k in range(n_hee):
        d = 0.09
        t = np.arange(int(SR * d)) / SR
        f = 1100.0 * (0.93 ** k) * (1 + 0.05 * np.sin(2 * np.pi * 36 * t))
        ph = 2 * np.pi * np.cumsum(f) / SR
        tone = np.sin(ph) + 0.35 * np.sin(2 * ph) + 0.15 * rng.normal(0, 1, len(t))
        env = np.sin(np.pi * t / d) ** 1.2
        out += [tone * env, np.zeros(int(SR * 0.045))]
    y = bandpass(np.concatenate(out).astype(np.float32), 600, 6000, 2)
    return y / (np.abs(y).max() + 1e-9)


def chirp(dur=0.16, f_from=520.0, f_to=980.0, seed=3):
    """A caracal's chirrup: a harmonic tone sweeping up with a quick trill
    in its amplitude, through a bright formant."""
    n = int(SR * dur)
    t = np.arange(n) / SR
    f = f_from * (f_to / f_from) ** (t / dur)
    ph = 2 * np.pi * np.cumsum(f) / SR
    tone = sum((0.6 ** h) * np.sin((h + 1) * ph) for h in range(6))
    trill = 0.6 + 0.4 * np.sin(2 * np.pi * 32 * t)
    env = np.sin(np.pi * np.clip(t / dur, 0, 1)) ** 1.5
    y = bandpass((tone * trill * env).astype(np.float32), 700, 5000, 2)
    return y / (np.abs(y).max() + 1e-9)
