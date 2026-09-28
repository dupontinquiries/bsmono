"""geartooth_voice -- speaks the Geartooth tongue (src/geartooth.rs).

    python3 geartooth_voice.py OUT_DIR [voice ...]        (default: oremantle)

Kokoro cannot say this tongue: it has two clicks and four vowels English
does not. So this is a small formant synthesizer, built for exactly it:

* a voiced source (a glottal pulse with a little jitter and shimmer, and a
  breath of aspiration), shaped by three formants that glide from sound to
  sound (the vowels: a e i o u, and ä ö ü y);
* the consonants as the source, noise, or silence through their own shapes
  (t k d stops with a burst, th s h v fricatives, l m n, a rolled r);
* the CLICKS as what they are -- no breath, the tongue alone: `!` a pop off
  the roof of the mouth (a resonant snap near 1.8 kHz with a low knock
  under it), `|` a sharp tsk behind the teeth (a burst of high noise);
* prosody from the spelling: a phrase falls; `...` trails off breathy; a
  word in CAPITALS is stressed, higher and louder; a closing `!` lifts.

It reads each voice's lines from recording_sheet.tsv (generated from
src/voicescript.rs), speaks them into OUT_DIR/session.wav with silence
between, and writes OUT_DIR/takes.json -- the manifest master_take.py takes
in place of a recording ('clean': false, so its declicker does not take the
clicks out):

    python3 master_take.py OUT_DIR/takes.json ../../assets/audio
"""
import csv
import json
import os
import sys

import numpy as np
import soundfile as sf
from scipy.signal import lfilter

HERE = os.path.dirname(os.path.abspath(__file__))
SR = 48000
BLOCK = 240  # 5 ms: formants are held per block, and glide block to block

# each voice: base pitch (Hz), how breathy, how creaky at a phrase's end
VOICES = {
    # the Sentinel: an old, huge voice given her by the engineers -- low for
    # a woman's (the oremantle chain takes it down further), a little breath,
    # creak where a phrase settles
    'oremantle': dict(f0=168.0, breath=0.05, creak=0.5),
}

# vowel formants (F1, F2, F3), a woman's voice
VOWELS = {
    'a': (820, 1450, 2800), 'e': (560, 1950, 2800), 'i': (320, 2650, 3300),
    'o': (480, 900, 2700), 'u': (340, 820, 2500),
    'ä': (720, 1850, 2750), 'ö': (460, 1600, 2500), 'ü': (330, 1950, 2600),
    'y': (380, 1450, 2500),
}
# voiced consonants: formants and loudness
VOICED = {'m': ((280, 1100, 2300), 0.35), 'n': ((280, 1650, 2600), 0.35),
          'l': ((360, 1300, 2800), 0.55), 'r': ((460, 1250, 1650), 0.6),
          'v': ((330, 1400, 2400), 0.3), 'd': ((300, 1700, 2600), 0.15)}


def parse(text):
    """Syllables of sound: a list of (phone, stressed, long) and pause marks."""
    out = []
    words = text.replace('...', ' … ').split()
    for w in words:
        if w == '…':
            out.append(('PAUSE', 0.42, 'trail'))
            continue
        stressed = any(c.isupper() for c in w) and len([c for c in w if c.isalpha()]) > 1 and w.upper() == w
        end = ''
        while w and w[-1] in '.,!?:':
            end = w[-1] + end
            w = w[:-1]
        w = w.lower()
        i = 0
        while i < len(w):
            c = w[i]
            if c in '!|' and i + 1 < len(w):
                out.append((c, stressed, False))
            elif c == '-':
                pass
            elif w[i:i + 2] == 'th':
                out.append(('th', stressed, False))
                i += 1
            elif c in VOWELS:
                long = i + 1 < len(w) and w[i + 1] == c
                out.append((c, stressed, long))
                if long:
                    i += 1
            elif c.isalpha():
                out.append((c, stressed, False))
            i += 1
        if '!' in end:
            out.append(('LIFT', 0, 0))
        if '.' in end:
            out.append(('PAUSE', 0.28, 'fall'))
        elif ',' in end:
            out.append(('PAUSE', 0.16, 'comma'))
        elif ':' in end:
            out.append(('PAUSE', 0.2, 'comma'))
        else:
            out.append(('PAUSE', 0.05, 'word'))
    return out


def resonator(x, f, bw, zi):
    r = np.exp(-np.pi * bw / SR)
    a1, a2 = -2 * r * np.cos(2 * np.pi * f / SR), r * r
    b0 = 1 - r  # roughly unit gain at the peak
    y, zf = lfilter([b0], [1, a1, a2], x, zi=zi)
    return y, zf


def bandnoise(n, lo, hi, rng):
    x = rng.standard_normal(n)
    X = np.fft.rfft(x)
    fr = np.fft.rfftfreq(n, 1 / SR)
    X[(fr < lo) | (fr > hi)] = 0
    y = np.fft.irfft(X, n)
    return y / (np.abs(y).max() + 1e-9)


def speak(text, v, seed):
    rng = np.random.default_rng(seed)
    seq = parse(text)
    # durations and targets, sound by sound
    plan = []  # (kind, dur, formants, gain, extra)
    for k, (ph, st, lg) in enumerate(seq):
        if ph == 'PAUSE':
            plan.append(('pause', st, None, 0.0, lg))
        elif ph == 'LIFT':
            plan.append(('lift', 0.0, None, 0.0, None))
        elif ph in VOWELS:
            d = (0.30 if lg else 0.17) * (1.35 if st else 1.0)
            plan.append(('vowel', d, VOWELS[ph], 1.0 * (1.4 if st else 1.0), None))
        elif ph in VOICED:
            d = 0.09 if ph == 'r' else 0.075
            plan.append(('voiced', d, VOICED[ph][0], VOICED[ph][1], ph))
        elif ph in ('!', '|'):
            plan.append(('click', 0.05, None, 1.0, ph))
        elif ph in ('t', 'k'):
            plan.append(('stop', 0.1, None, 0.5, ph))
        elif ph in ('s', 'th', 'h'):
            plan.append(('fric', 0.11 if ph == 's' else 0.09, None, 0.3, ph))
        else:
            plan.append(('voiced', 0.07, (400, 1500, 2600), 0.3, ph))
    # the pitch line: falls across the phrase, up on stress and a lift
    total = sum(p[1] for p in plan if p[0] != 'lift')
    n = int((total + 0.3) * SR)
    f0 = np.full(n, v['f0'])
    amp = np.zeros(n)
    voiced = np.zeros(n)
    noise_amp = np.zeros(n)
    noise = np.zeros(n)
    clicks = np.zeros(n)
    F = np.zeros((n, 3))
    t = int(0.05 * SR)
    last_f = VOWELS['a']
    lift = 0.0
    trail = 0.0
    for idx, (kind, d, fm, g, ex) in enumerate(plan):
        if kind == 'lift':
            lift = 0.18
            continue
        m = int(d * SR)
        seg = slice(t, min(n, t + m))
        L = seg.stop - seg.start
        if L <= 0:
            continue
        u = t / n
        base = v['f0'] * (1.08 - 0.22 * u)
        if kind == 'pause':
            if ex == 'fall':
                f0[seg] = base * 0.85
            if ex == 'trail':
                trail = 1.0
            F[seg] = last_f
        elif kind in ('vowel', 'voiced'):
            stress = g > 1.0
            ramp = np.linspace(0, 1, L)
            f0[seg] = base * (1.0 + (0.28 if stress else 0.0) + lift * ramp) * (1.0 - 0.12 * trail * ramp)
            fm = np.array(fm, dtype=float)
            # glide from the last sound over the first 40 ms
            gl = min(L, int(0.04 * SR))
            F[seg] = fm
            F[t:t + gl] = np.array(last_f) + (fm - np.array(last_f)) * np.linspace(0, 1, gl)[:, None]
            env = np.minimum(1.0, np.minimum(np.arange(L) / (0.02 * SR), (L - np.arange(L)) / (0.03 * SR) + 0.35))
            amp[seg] = g * env
            voiced[seg] = 1.0
            if ex == 'r':
                amp[seg] *= 0.55 + 0.45 * np.cos(2 * np.pi * 27 * np.arange(L) / SR)
            if ex == 'v':
                noise[seg] = bandnoise(L, 1500, 6000, rng) * 0.25
            if ex == 'd':
                noise[seg][:int(0.012 * SR)] = bandnoise(L, 2000, 4500, rng)[:int(0.012 * SR)] * 0.4
            last_f = tuple(fm)
            lift *= 0.5
            trail *= 0.9
        elif kind == 'stop':
            # closure (silence), a burst, a breath of aspiration
            c = int(0.05 * SR)
            b = int((0.012 if ex == 't' else 0.018) * SR)
            lo, hi = (3500, 8000) if ex == 't' else (1500, 3200)
            s0 = t + c
            burst = bandnoise(b + int(0.03 * SR), lo, hi, rng)
            e = np.exp(-np.arange(len(burst)) / (0.008 * SR))
            end = min(n, s0 + len(burst))
            noise[s0:end] += (burst * e)[:end - s0] * 0.6
            F[seg] = last_f
        elif kind == 'fric':
            lo, hi, a = {'s': (4500, 10000, 0.22), 'th': (2000, 7500, 0.07), 'h': (500, 4000, 0.09)}[ex]
            x = bandnoise(L, lo, hi, rng)
            env = np.minimum(1.0, np.minimum(np.arange(L) / (0.02 * SR), (L - np.arange(L)) / (0.02 * SR)))
            noise[seg] += x * env * a
            F[seg] = last_f
        elif kind == 'click':
            # silence round it; the snap itself
            c0 = t + int(0.012 * SR)
            if ex == '!':
                k = int(0.03 * SR)
                tt = np.arange(k) / SR
                snap = np.sin(2 * np.pi * 1850 * tt) * np.exp(-tt / 0.0035)
                knock = np.sin(2 * np.pi * 420 * tt) * np.exp(-tt / 0.006) * 0.5
                pop = (snap + knock)
                pop[:2] += 0.8
                clicks[c0:c0 + k] += pop[:max(0, min(k, n - c0))] * 0.95
            else:
                k = int(0.02 * SR)
                tsk = bandnoise(k, 3000, 7500, rng) * np.exp(-np.arange(k) / (0.004 * SR))
                clicks[c0:c0 + k] += tsk[:max(0, min(k, n - c0))] * 0.8
            F[seg] = last_f
        t += L
    # the source: a glottal pulse train (jittered), and aspiration
    jit = 1.0 + 0.012 * rng.standard_normal(n).cumsum() / np.sqrt(np.arange(1, n + 1))
    ph = np.cumsum(f0 * jit / SR) % 1.0
    # Rosenberg-ish: open 60%, a sharp close; differentiated
    op = 0.6
    pulse = np.where(ph < op, 0.5 * (1 - np.cos(np.pi * ph / op)), np.cos(np.pi * (ph - op) / (2 * (1 - op))).clip(0))
    src = np.diff(pulse, prepend=0.0) * 30.0
    # a real voice's source falls away ~12 dB an octave; the differentiated
    # pulse alone only 6, and the vowels came out buzzing to 8 kHz: a gentle
    # low-pass for the rest of the tilt
    k = np.exp(-2 * np.pi * 900 / SR)
    src = lfilter([1 - k], [1, -k], src) * 6.0
    shimmer = 1.0 + 0.04 * rng.standard_normal(n // BLOCK + 1).repeat(BLOCK)[:n]
    breath = bandnoise(n, 300, 6000, rng) * v['breath']
    # creak where a phrase settles: every other pulse weaker
    creak = 1.0 - v['creak'] * 0.4 * (np.floor(np.cumsum(f0 / SR)) % 2) * (f0 < v['f0'] * 0.9)
    x = (src * shimmer * creak + breath) * amp * voiced
    # the formants, block by block
    y = np.zeros(n)
    zis = [np.zeros(2), np.zeros(2), np.zeros(2)]
    bws = (90.0, 110.0, 160.0)
    for s in range(0, n, BLOCK):
        e = min(n, s + BLOCK)
        blk = x[s:e]
        acc = 0
        for j in range(3):
            out, zis[j] = resonator(blk, F[s, j], bws[j], zis[j])
            acc = acc + out * (1.0, 0.7, 0.35)[j]
        y[s:e] = acc
    y = y / (np.abs(y).max() + 1e-9) * 0.8
    y = y + noise * 0.5 + clicks * 0.9
    y = y / (np.abs(y).max() + 1e-9) * 0.85
    return y.astype(np.float32)


def main():
    out = sys.argv[1]
    who = sys.argv[2:] or ['oremantle']
    os.makedirs(out, exist_ok=True)
    rows = [r for r in csv.DictReader(open(os.path.join(HERE, 'recording_sheet.tsv')), delimiter='\t') if r['voice'] in who]
    gap = np.zeros(int(1.1 * SR), dtype=np.float32)
    session = [gap]
    takes = []
    t = len(gap) / SR
    for k, r in enumerate(rows):
        x = speak(r['line'], VOICES[r['voice']], seed=k + 7)
        key = r['file'].split('/')[-1][:-len('.opus')]
        takes.append([r['voice'], key, round(t - 0.2, 3), round(t + len(x) / SR + 0.2, 3)])
        session += [x, gap]
        t += (len(x) + len(gap)) / SR
        print(f"{r['voice']} {key}: '{r['line']}' {len(x) / SR:.2f} s")
    path = os.path.join(out, 'session.wav')
    sf.write(path, np.concatenate(session), SR)
    json.dump({'session': os.path.abspath(path), 'clean': False, 'source': 'geartooth_voice.py (formant synthesis)', 'takes': takes}, open(os.path.join(out, 'takes.json'), 'w'), indent=1)


if __name__ == '__main__':
    main()
