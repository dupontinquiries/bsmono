"""baby_swarm -- a crowd of babies round Sproutkit when an ability fires.

    python3 baby_swarm.py [AUDIO_ROOT]     (default: ../../assets/audio)

Writes AUDIO_ROOT/sfx/swarm/sproutkit/<slot>/b<k>.<take>.opus: for each of
his four abilities, several babies (the clips in baby/, see
baby_clips.py), each one mono and one baby. The game (cues.rs) plays baby
k at its own spot on a ring round the caster, so the crowd is spread
around him by the ordinary panning, and the SFX bus sends it into the
space's reverb. What is baked in here is what the mixer cannot do:

* pitch -- every baby is moved (rubberband, formants kept) from its own
  F0 onto a note of the ability's chord, so the crowd sings in harmony,
  not in a smear. The chord's notes are dealt out in turn, each to the
  clip nearest it; no baby moves more than 7 semitones (see assign);
* time -- each starts late by its own small delay (or on an eighth note
  of 132 bpm for the dance party), so they tumble in one after another;
* width -- a light chorus, so each baby is a little blurred, more than
  one.

Two takes of each, with different clips on different notes and a
different order of entry, so two casts in a row are not the same crowd.
"""
import os
import subprocess
import sys
import tempfile

import numpy as np
import soundfile as sf

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import baby_clips as C  # noqa: E402

SR = 48000
A4 = 440.0


def note(semis_from_a4):
    return A4 * 2 ** (semis_from_a4 / 12)


# slot: (clips to draw from, chord tones as semitones from A4, babies,
#        entry: ('scatter', spread_s) or ('beat', bpm), level)
SWARM = {
    # Litter: the kittens burst out -- bright "ah!"s, A major
    1: (['ah.1', 'ah.2', 'ah.3', 'ah.4', 'ah.5', 'ah.6', 'ah.7'], [-5, 0, 4, 7, 12], 6, ('scatter', 0.35), 0.8),
    # Giggle Fit: babble tumbling over itself, A major pentatonic
    2: (['babble.1', 'babble.2', 'babble.3', 'ah.3', 'ah.7'], [0, 2, 4, 7, 9], 6, ('scatter', 0.5), 0.75),
    # First Spring: soft coos, a D major chord with the ninth
    3: (['coo.1', 'coo.2', 'coo.3', 'coo.5', 'fall.1'], [-7, -3, 0, 5, 7], 5, ('scatter', 0.6), 0.6),
    # Dance Party: squeals and "ah!"s on the beat, A major over two octaves
    4: (['squeal.1', 'squeal.2', 'ah.1', 'ah.2', 'ah.5', 'ah.6', 'coo.3', 'babble.1'], [-12, -5, 0, 4, 7, 12, 16, 19], 8, ('beat', 132), 0.85),
}
TAKES = 2
MAX_SHIFT = 7


def f0_of(x):
    f = C.pitch(x)
    f = f[f > 0]
    return float(np.median(f)) if len(f) else 0.0


def shift(x, ratio):
    d = tempfile.mkdtemp(prefix='swarm_')
    a, b = os.path.join(d, 'a.wav'), os.path.join(d, 'b.wav')
    sf.write(a, x, SR)
    subprocess.run(['ffmpeg', '-v', 'error', '-y', '-i', a, '-af',
                    f'rubberband=pitch={ratio:.5f}:formant=preserved,'
                    'chorus=0.7:0.8:22|31:0.3|0.25:0.5|0.8:1.2|1.6', b], check=True)
    y, _ = sf.read(b, dtype='float32')
    return y


def assign(f0s, tones, n, rng):
    """(clip, target Hz) pairs, `n` of them: the chord's tones are dealt
    out in a random order (round again past the end), and each tone goes
    to the clip nearest it (in whichever octave is nearest) not used yet --
    a used one only when no fresh clip is within MAX_SHIFT semitones. So
    the crowd covers the chord instead of piling onto the one note the
    clips happen to sit near."""
    order = [tones[i] for i in rng.permutation(len(tones))]
    out, used = [], set()
    for k in range(n):
        t = order[k % len(order)]
        best = None
        for fresh in (True, False):
            for i, f in enumerate(f0s):
                if f <= 0 or (fresh and i in used):
                    continue
                for o in (-24, -12, 0, 12, 24):
                    d = 12 * np.log2(note(t + o) / f)
                    if abs(d) <= MAX_SHIFT and (best is None or abs(d) < abs(best[1])):
                        best = (i, d, note(t + o))
            if best:
                break
        if best:
            used.add(best[0])
            out.append((best[0], best[2]))
    return out


def main():
    root = sys.argv[1] if len(sys.argv) > 1 else os.path.join(HERE, '..', '..', 'assets', 'audio')
    clips = {}
    for slot, (names, tones, n, entry, level) in SWARM.items():
        for n_ in names:
            if n_ not in clips:
                x, _ = sf.read(os.path.join(HERE, 'baby', n_ + '.opus'), dtype='float32')
                clips[n_] = (x, f0_of(x))
        out = os.path.join(root, 'sfx', 'swarm', 'sproutkit', str(slot))
        os.makedirs(out, exist_ok=True)
        for f in os.listdir(out):
            os.remove(os.path.join(out, f))
        for take in range(1, TAKES + 1):
            rng = np.random.default_rng(slot * 10 + take)
            # babies: the clips in a random order (assign reuses one only
            # when it must)
            pool = [names[i] for i in rng.permutation(len(names))]
            picks = assign([clips[c][1] for c in pool], tones, n, rng)
            assert len(picks) >= n - 1, (slot, take, len(picks))
            for k, (i, hz) in enumerate(picks):
                x, f0 = clips[pool[i]]
                y = shift(x, hz / f0)
                if entry[0] == 'beat':
                    delay = (60 / entry[1] / 2) * k
                else:
                    delay = float(rng.uniform(0, entry[1])) if k else 0.0
                y = np.concatenate([np.zeros(int(delay * SR), np.float32), y])
                y *= level * 0.9 / (np.abs(y).max() + 1e-9)
                fo = int(0.03 * SR)
                y[-fo:] *= np.linspace(1, 0, fo)
                dst = os.path.join(out, f'b{k}.{take}.opus')
                sf.write(dst, y, SR, format='OGG', subtype='OPUS')
                print(f'slot {slot} take {take} b{k}: {pool[i]:9} {f0:6.0f} Hz -> {hz:6.0f} Hz ({12 * np.log2(hz / f0):+.1f} st), in at {delay:.2f} s', flush=True)


if __name__ == '__main__':
    main()
