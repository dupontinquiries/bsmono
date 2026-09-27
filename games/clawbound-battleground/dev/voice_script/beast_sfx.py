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

Each sound is pitched down with its formants moving too (a bigger throat,
not a lower voice from the same throat), given a sub-octave and a
growl-carrier layer vocoded by its own envelope (beast.py), then a stone
room, and written to sfx/beast/<name>.opus. cues.rs plays them:

    wake   BossSpawn        the rattling snore, 5 semitones down
    fall   BossDown         a purring in-breath let out slowly, 8 down
    snort  TitanVulnerable  a waking snort, 4 down
"""
import os
import subprocess
import sys
import tempfile

import numpy as np
import soundfile as sf

import beast as B
import baby_clips as C

SR = 48000
HERE = os.path.dirname(os.path.abspath(__file__))

# name: (from_s, to_s, semitones, tempo, sub_db, growl_db, room, lowpass Hz)
# (wake ends at 68.95: at 69.0 the sleeper's bed creaks, a click)
SOUNDS = {
    'wake': (66.55, 68.95, -5, 1.0, -9, -12, 'hall', 3200),
    'fall': (54.35, 56.20, -8, 0.7, -8, -16, 'hall', 1600),
    'snort': (152.30, 152.95, -4, 1.0, -10, -14, 'room', 4000),
}
# a stretch between breaths: the room, the fridge, nothing of the sleeper
ROOM_TONE = (60.3, 61.9)


def ff(x, af):
    d = tempfile.mkdtemp(prefix='bsfx_')
    a, b = os.path.join(d, 'a.wav'), os.path.join(d, 'b.wav')
    sf.write(a, x, SR)
    subprocess.run(['ffmpeg', '-v', 'error', '-y', '-i', a, '-af', af, b], check=True)
    y, _ = sf.read(b, dtype='float32')
    return y


def denoise(x, tone):
    """Spectral subtraction of the room tone, 2x, floored at -24 dB."""
    X = C._stft(x)
    M = np.abs(X)
    g = np.maximum(1.0 - 2.0 * tone[None, :] / np.maximum(M, 1e-9), 0.063)
    g = np.pad(g, ((1, 1), (0, 0)), mode='edge')
    g = (g[:-2] + g[1:-1] + g[2:]) / 3.0
    return C._istft(X * g, len(x))


ROOMS = {
    # (delays ms, decays): a stone chamber for the bosses' lairs
    'hall': ('aecho=0.8:0.7:61|97|149|211:0.38|0.28|0.2|0.13'),
    'room': ('aecho=0.8:0.6:23|41|67:0.25|0.18|0.1'),
}


def make(full, tone, a, b, semis, tempo, sub_db, growl_db, room, lp):
    pre = 0.1
    x = full[int((a - pre) * SR):int((b + pre) * SR)].astype(np.float32)
    x = denoise(x, tone)[int(pre * SR):][:int((b - a) * SR)]
    x = B.bandpass(x, 40, 9000)
    ratio = 2 ** (semis / 12)
    # formants follow the pitch: the throat itself is bigger
    body = ff(x, f'rubberband=pitch={ratio:.5f}:tempo={tempo:.3f}:pitchq=quality:window=long')
    sub = B.lowpass(B.octave(body, 0.5), 400)
    growl = B.cross(body, B.growl_carrier(len(body), f0=48.0, seed=7))
    y = body + B.at(sub_db, sub, body) + B.at(growl_db, growl, body)
    # the breath hiss, moved down with the pitch into the 2-3 kHz 'presence'
    # band, is what makes a snore read as a man asleep: a beast is darker
    y = B.lowpass(y, lp)
    y = np.concatenate([y, np.zeros(int(0.6 * SR), np.float32)])
    y = ff(y, ROOMS[room])
    # trim the tail once the echo is 50 dB down
    e = B.envelope(y, 20)
    last = np.where(e > e.max() * 10 ** (-50 / 20))[0]
    y = y[:last[-1] + 1] if len(last) else y
    fi, fo = int(0.01 * SR), int(0.12 * SR)
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
