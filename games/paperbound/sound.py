"""
PAPERBOUND — sound engine.

Fully procedural: every UI blip, SFX and ambient bed is synthesised at runtime
with numpy, so the game ships with zero audio asset files and stays a single
self-contained package. If no audio device is available (headless / CI / a
locked-down machine) the whole module degrades to a silent no-op — nothing here
ever raises into the game loop.

Public surface used by game.py:
    sound.init()                         # call once at startup (safe if it fails)
    sound.play(name, gain=1.0)           # fire a one-shot SFX by name
    sound.set_ambient(key)               # cross-fade to an arena's ambient bed
    sound.stop_ambient()
    sound.set_master(v) / get_master()   # 0..1 master volume (slider)
    sound.set_muted(b)  / is_muted()
    sound.available                      # bool: is real audio playing?
    SFX                                  # tuple of known one-shot names
"""
import math

try:
    import numpy as np
    _NUMPY = True
except Exception:
    _NUMPY = False

try:
    import pygame
    _PYGAME = True
except Exception:
    _PYGAME = False

RATE = 44100
available = False           # flipped True by init() only if the mixer comes up
_master = 0.7               # 0..1 master volume
_muted = False
_cache = {}                 # name -> pygame.mixer.Sound
_ambient_chan = None        # dedicated channel for the looping ambient bed
_ambient_key = None         # currently-playing ambient key
_ui_chan = None             # dedicated channel for UI blips (never starved)

# one-shot SFX the game may request
SFX = ("click", "select", "deploy", "error", "hover", "back", "toggle",
       "coin", "victory", "defeat", "forfeit", "tower_hit", "card_draw",
       "whoosh", "thunder", "freeze", "fire")


# ─────────────────────────────── synthesis ────────────────────────────────
def _env(n, attack=0.01, release=0.2):
    """A simple attack/release amplitude envelope of length n samples."""
    e = np.ones(n)
    a = max(1, int(RATE * attack))
    r = max(1, int(RATE * release))
    if a < n:
        e[:a] = np.linspace(0, 1, a)
    if r < n:
        e[-r:] = np.linspace(1, 0, r)
    return e


def _tone(freq, dur, kind="sine", vol=0.5, attack=0.005, release=0.12, detune=0.0):
    n = int(RATE * dur)
    t = np.linspace(0, dur, n, endpoint=False)
    f = freq * (1.0 + detune * np.linspace(0, 1, n))
    ph = 2 * np.pi * np.cumsum(f) / RATE
    if kind == "sine":
        w = np.sin(ph)
    elif kind == "tri":
        w = 2 * np.abs(2 * (ph / (2 * np.pi) % 1) - 1) - 1
    elif kind == "square":
        w = np.sign(np.sin(ph))
    elif kind == "saw":
        w = 2 * (ph / (2 * np.pi) % 1) - 1
    else:
        w = np.sin(ph)
    return (w * _env(n, attack, release) * vol).astype(np.float32)


def _boxblur(x, radius):
    """A fast vectorised moving-average (approximates a low-pass) via cumsum.
    Replaces a per-sample IIR loop that was O(n) in *Python* — on a slow mobile
    CPU that loop over ~176k samples could stall the main thread for a noticeable
    fraction of a second when an ambient bed is first built."""
    radius = max(1, int(radius))
    k = radius * 2 + 1
    c = np.cumsum(np.concatenate(([0.0], x)))
    # padded moving sum → mean
    pad = np.concatenate((np.full(radius, x[0]), x, np.full(radius, x[-1])))
    cpad = np.cumsum(np.concatenate(([0.0], pad)))
    out = (cpad[k:] - cpad[:-k]) / k
    return out[:len(x)].astype(np.float32)


def _noise(dur, vol=0.4, attack=0.002, release=0.15, lp=0.0):
    n = int(RATE * dur)
    x = np.random.uniform(-1, 1, n).astype(np.float32)
    if lp > 0:  # soft papery low-pass — vectorised, no Python sample loop
        radius = max(1, int(RATE * lp * 0.5))
        x = _boxblur(x, radius) * 3.0
    return (x * _env(n, attack, release) * vol).astype(np.float32)


def _mix(*layers):
    n = max(len(l) for l in layers)
    out = np.zeros(n, dtype=np.float32)
    for l in layers:
        out[:len(l)] += l
    peak = np.max(np.abs(out)) or 1.0
    if peak > 1.0:
        out /= peak
    return out


def _to_sound(mono):
    """float32 mono [-1,1] → a stereo pygame Sound."""
    clipped = np.clip(mono, -1, 1)
    stereo = np.repeat((clipped * 32767).astype(np.int16)[:, None], 2, axis=1)
    return pygame.sndarray.make_sound(np.ascontiguousarray(stereo))


def _build(name):
    """Synthesize a one-shot SFX by name (aged-paper / ink-and-candle palette)."""
    if name == "click":
        return _mix(_tone(660, 0.05, "tri", 0.4, release=0.05),
                    _noise(0.03, 0.15, lp=0.0008))
    if name == "select":
        return _mix(_tone(520, 0.09, "sine", 0.4, detune=0.4),
                    _tone(780, 0.06, "sine", 0.2))
    if name == "hover":
        return _tone(880, 0.035, "sine", 0.18, release=0.03)
    if name == "back":
        return _mix(_tone(440, 0.10, "tri", 0.35, detune=-0.35))
    if name == "toggle":
        return _mix(_tone(600, 0.05, "square", 0.22),
                    _tone(900, 0.05, "square", 0.14))
    if name == "deploy":       # a soft paper "thup" as a unit unfolds onto the page
        return _mix(_noise(0.10, 0.35, lp=0.0016),
                    _tone(200, 0.12, "tri", 0.3, detune=0.6, release=0.1))
    if name == "card_draw":    # a paper slide
        return _noise(0.16, 0.3, attack=0.02, release=0.12, lp=0.0022)
    if name == "error":        # the "not enough ink" bounce
        return _mix(_tone(300, 0.10, "square", 0.28, release=0.08),
                    _tone(230, 0.12, "square", 0.24, release=0.1))
    if name == "coin":
        return _mix(_tone(880, 0.08, "sine", 0.3),
                    _tone(1320, 0.12, "sine", 0.24))
    if name == "tower_hit":
        return _mix(_noise(0.09, 0.4, lp=0.0009),
                    _tone(140, 0.10, "tri", 0.3, detune=-0.4))
    if name == "whoosh":
        return _noise(0.24, 0.4, attack=0.05, release=0.16, lp=0.003)
    if name == "thunder":
        return _mix(_noise(0.6, 0.55, attack=0.01, release=0.5, lp=0.004),
                    _tone(70, 0.5, "sine", 0.4, release=0.4))
    if name == "freeze":
        return _mix(_tone(1200, 0.22, "sine", 0.22, detune=-0.5),
                    _tone(1800, 0.18, "sine", 0.14, detune=-0.4))
    if name == "fire":
        return _mix(_noise(0.3, 0.4, lp=0.0016),
                    _tone(320, 0.2, "saw", 0.2, detune=-0.6, release=0.16))
    if name == "forfeit":
        return _mix(_tone(400, 0.5, "tri", 0.3, detune=-0.5, release=0.4))
    if name == "victory":      # a small warm rising triad
        return _mix(_tone(523, 0.5, "sine", 0.34, release=0.4),
                    _tone(659, 0.5, "sine", 0.30, release=0.4, attack=0.08),
                    _tone(784, 0.6, "sine", 0.30, release=0.5, attack=0.16))
    if name == "defeat":       # a slow falling minor drop
        return _mix(_tone(392, 0.6, "tri", 0.32, release=0.5),
                    _tone(311, 0.7, "tri", 0.30, release=0.55, attack=0.1),
                    _tone(233, 0.8, "sine", 0.28, release=0.6, attack=0.2))
    # default: a neutral blip
    return _tone(600, 0.05, "sine", 0.3)


def _build_ambient(key):
    """A ~4s seamless-ish ambient bed themed to an arena's climate. Loops."""
    dur = 4.0
    n = int(RATE * dur)
    t = np.linspace(0, dur, n, endpoint=False)
    # a low room drone everyone shares
    base = 0.12 * np.sin(2 * np.pi * 55 * t) + 0.08 * np.sin(2 * np.pi * 82.5 * t)
    base *= 0.5 + 0.5 * np.sin(2 * np.pi * 0.15 * t)     # slow swell
    layers = [base.astype(np.float32)]
    # soft filtered hiss bed (paper / air)
    hiss = _noise(dur, 0.05, attack=0.5, release=0.5, lp=0.02)[:n]
    layers.append(hiss)
    if key in ("study", "quake", "clockwork", "observatory", "forge"):
        # candle room: occasional soft crackle
        cr = np.zeros(n, dtype=np.float32)
        for _ in range(6):
            i = np.random.randint(0, n - 400)
            cr[i:i + 300] += np.random.uniform(-1, 1, 300).astype(np.float32) * \
                np.linspace(1, 0, 300) * 0.10
        layers.append(cr)
    if key in ("frost", "aurora"):
        layers.append((0.06 * np.sin(2 * np.pi * 220 * t) *
                       (0.5 + 0.5 * np.sin(2 * np.pi * 0.3 * t))).astype(np.float32))
    if key in ("storm",):
        layers.append(_noise(dur, 0.09, attack=0.4, release=0.4, lp=0.05)[:n])
    if key in ("volcano", "forge"):
        layers.append((0.08 * np.sin(2 * np.pi * 48 * t)).astype(np.float32) +
                      _noise(dur, 0.05, lp=0.03)[:n])
    if key in ("sunken", "sandbar", "outcrop", "inktubs", "greenhouse"):
        layers.append((0.05 * np.sin(2 * np.pi * 0.5 * t) *
                       np.sin(2 * np.pi * 160 * t)).astype(np.float32))
    if key in ("dunes", "sandstorm"):
        layers.append(_noise(dur, 0.10, attack=0.6, release=0.6, lp=0.08)[:n])
    out = _mix(*layers)
    # gentle fade at the loop seam to hide the discontinuity
    seam = int(RATE * 0.2)
    out[:seam] *= np.linspace(0, 1, seam)
    out[-seam:] *= np.linspace(1, 0, seam)
    return out * 0.6


# ─────────────────────────────── public API ───────────────────────────────
def init():
    """Bring up the mixer and pre-bake the SFX. Safe to call when audio or
    numpy are missing — it just leaves `available` False and every call no-ops."""
    global available, _ambient_chan, _ui_chan
    if not (_NUMPY and _PYGAME):
        return False
    try:
        pygame.mixer.pre_init(RATE, -16, 2, 512)
        pygame.mixer.init(RATE, -16, 2, 512)
        pygame.sndarray.array  # attribute probe
    except Exception:
        available = False
        return False
    try:
        pygame.mixer.set_num_channels(24)
        _ambient_chan = pygame.mixer.Channel(0)
        _ui_chan = pygame.mixer.Channel(1)
        # pre-bake one-shots so first use never hitches mid-frame
        for nm in SFX:
            try:
                _cache[nm] = _to_sound(_build(nm))
            except Exception:
                pass
        # pre-bake the menu ambient bed too (the one shown at startup)
        try:
            _cache[("amb", "study")] = _to_sound(_build_ambient("study"))
        except Exception:
            pass
        available = True
        _apply_volumes()
        return True
    except Exception:
        available = False
        return False


def _apply_volumes():
    if not available:
        return
    v = 0.0 if _muted else _master
    for s in _cache.values():
        try: s.set_volume(v)
        except Exception: pass
    if _ambient_chan is not None:
        try: _ambient_chan.set_volume(v * 0.55)   # ambience sits under SFX
        except Exception: pass


def play(name, gain=1.0):
    """Fire a one-shot SFX. Unknown names fall back to a neutral blip."""
    if not available or _muted or _master <= 0:
        return
    snd = _cache.get(name)
    if snd is None:
        try:
            snd = _cache[name] = _to_sound(_build(name))
        except Exception:
            return
    try:
        vol = max(0.0, min(1.0, _master * gain))
        if name in ("click", "hover", "select", "toggle", "back") and _ui_chan is not None:
            snd.set_volume(vol)
            _ui_chan.play(snd)
        else:
            snd.set_volume(vol)
            snd.play()
    except Exception:
        pass


def set_ambient(key):
    """Cross-fade the looping ambient bed to a new arena key (or None to stop)."""
    global _ambient_key
    if not available:
        _ambient_key = key
        return
    if key == _ambient_key:
        return
    _ambient_key = key
    if _ambient_chan is None:
        return
    try:
        if key is None:
            _ambient_chan.fadeout(600)
            return
        bed = _cache.get(("amb", key))
        if bed is not None:
            _ambient_chan.play(bed, loops=-1, fade_ms=800)
            _ambient_chan.set_volume((0.0 if _muted else _master) * 0.55)
            return
        # not cached yet — synthesise on a background thread so the main game
        # loop never blocks on ~tens of ms of DSP when changing scenes/arenas.
        import threading

        def _bake(k=key):
            try:
                snd = _to_sound(_build_ambient(k))
                _cache[("amb", k)] = snd
                # only start it if we're still meant to be on this bed
                if _ambient_key == k and _ambient_chan is not None:
                    snd.set_volume((0.0 if _muted else _master) * 0.55)
                    _ambient_chan.play(snd, loops=-1, fade_ms=800)
            except Exception:
                pass
        threading.Thread(target=_bake, daemon=True).start()
    except Exception:
        pass


def stop_ambient():
    set_ambient(None)


def set_master(v):
    global _master
    _master = max(0.0, min(1.0, float(v)))
    _apply_volumes()


def get_master():
    return _master


def set_muted(b):
    global _muted
    _muted = bool(b)
    _apply_volumes()


def is_muted():
    return _muted


if __name__ == "__main__":
    # headless self-test: synthesis must work even without an audio device
    assert _NUMPY, "numpy required for synthesis"
    ok = 0
    for nm in SFX:
        buf = _build(nm)
        assert buf is not None and len(buf) > 0
        ok += 1
    for k in ("study", "frost", "volcano", "storm", "dunes", "observatory"):
        b = _build_ambient(k)
        assert len(b) > 0
    print(f"✅ sound self-test: synthesised {ok} SFX + ambient beds "
          f"(audio device available: {_PYGAME})")
