"""
Paperbound — Seasonal & Holiday Events.

A calendar of short special events that dress the game up and nudge the default
mode for a day or two. The whole thing is a pure function of the DATE, which is
the trick that makes it "just work" across devices: every player whose clock says
Oct 31 sees Halloween — no server, no coordination, no sync messages. Two people
on the same LAN on the same evening get the same theme automatically.

What an event may change:
  • palette      — a subtle tint applied to the board/paper (cosmetic)
  • particles    — an ambient overlay (snow, falling leaves, embers, petals…)
  • banner       — a little greeting shown on the menu
  • accent       — a UI accent colour for the menu
  • emoji        — a decoration for the title
  • default_mode — the mode the menu offers first during the window (a nudge, not
                   a lock — the player can still pick anything)
  • features     — optional flags a build may honour (all cosmetic-safe)

DETERMINISM NOTE. Everything here is COSMETIC or a menu-default. None of it feeds
the simulation, so it can never desync a net match — two peers can even be in
different time zones near midnight and, at worst, see different *decorations*; the
battle itself stays identical. If a future event ever wants to change real rules,
it must be gated behind the match seed / handshake, not the local date.

Public API:
    active_event(date=None)      -> event dict or None
    all_events()                 -> the full catalogue
    ambient_particles(ev, ...)   -> helper describing the overlay to draw
"""
import datetime
import math


# ── date helpers ─────────────────────────────────────────────────────────────
def _in_range(d, start, end):
    """True if (month, day) d falls within [start, end], inclusive, handling
    windows that wrap across the new year (e.g. Dec 28 → Jan 2)."""
    md = (d.month, d.day)
    if start <= end:
        return start <= md <= end
    return md >= start or md <= end          # wrapped around the year


def _easter(year):
    """Gregorian Easter Sunday (Anonymous/Meeus algorithm). Used to place the
    spring 'Bloom' event, which otherwise drifts week to week."""
    a = year % 19
    b = year // 100
    c = year % 100
    d = b // 4
    e = b % 4
    f = (b + 8) // 25
    g = (b - f + 1) // 3
    h = (19 * a + b - d - g + 15) % 30
    i = c // 4
    k = c % 4
    ll = (32 + 2 * e + 2 * i - h - k) % 7
    m = (a + 11 * h + 22 * ll) // 451
    month = (h + ll - 7 * m + 114) // 31
    day = ((h + ll - 7 * m + 114) % 31) + 1
    return datetime.date(year, month, day)


# Lunar New Year dates (fixed lookup — the astronomical calc isn't worth it here).
_LUNAR_NY = {
    2024: (2, 10), 2025: (1, 29), 2026: (2, 17), 2027: (2, 6),
    2028: (1, 26), 2029: (2, 13), 2030: (2, 3), 2031: (1, 23),
}


# ── the catalogue ────────────────────────────────────────────────────────────
# Each event: key, name, a matcher, and the cosmetic payload. `match` is a
# callable(date)->bool so movable feasts (Easter, Lunar NY) can compute a window.
def _fixed(start, end):
    return lambda d: _in_range(d, start, end)


def _easter_window(d):
    e = _easter(d.year)
    return (e - datetime.timedelta(days=3)) <= d <= (e + datetime.timedelta(days=1))


def _lunar_window(d):
    md = _LUNAR_NY.get(d.year)
    if not md:
        return False
    ny = datetime.date(d.year, md[0], md[1])
    return (ny - datetime.timedelta(days=1)) <= d <= (ny + datetime.timedelta(days=3))


EVENTS = [
    dict(key="newyear", name="New Year", emoji="🎆",
         match=_fixed((12, 31), (1, 2)),
         palette=(255, 226, 150), accent=(224, 190, 96), particles="fireworks",
         banner="A new page turns — Happy New Year!", default_mode="blitz"),

    dict(key="lunar", name="Lunar New Year", emoji="🧧",
         match=_lunar_window,
         palette=(230, 120, 90), accent=(220, 80, 70), particles="lanterns",
         banner="Fortune and folded paper — Lunar New Year!", default_mode="classic"),

    dict(key="valentines", name="Valentine's Day", emoji="💕",
         match=_fixed((2, 13), (2, 15)),
         palette=(240, 170, 195), accent=(214, 96, 130), particles="hearts",
         banner="Paper hearts on the page.", default_mode="sudden"),

    dict(key="stpatrick", name="St. Patrick's Day", emoji="🍀",
         match=_fixed((3, 16), (3, 18)),
         palette=(150, 210, 140), accent=(88, 168, 96), particles="clovers",
         banner="Luck of the fold to you.", default_mode="classic"),

    dict(key="aprilfools", name="April Fools", emoji="🃏",
         match=_fixed((4, 1), (4, 1)),
         palette=(210, 190, 240), accent=(150, 120, 210), particles="confetti",
         banner="Nothing is quite as it's folded today…", default_mode="living",
         features=dict(wildcard_boost=True)),

    dict(key="bloom", name="Spring Bloom", emoji="🌸",
         match=_easter_window,
         palette=(235, 200, 215), accent=(200, 130, 160), particles="petals",
         banner="Blossoms drift across the page.", default_mode="classic"),

    dict(key="earthday", name="Earth Day", emoji="🌱",
         match=_fixed((4, 22), (4, 22)),
         palette=(170, 205, 150), accent=(96, 156, 92), particles="leaves",
         banner="Grown from a single sheet.", default_mode="fortress"),

    dict(key="summer", name="Midsummer", emoji="☀️",
         match=_fixed((6, 20), (6, 22)),
         palette=(255, 224, 150), accent=(228, 176, 72), particles="sun",
         banner="The longest day — folded in gold.", default_mode="blitz"),

    dict(key="harvest", name="Harvest", emoji="🍂",
         match=_fixed((11, 20), (11, 30)),
         palette=(214, 158, 96), accent=(178, 110, 56), particles="leaves",
         banner="A harvest of folds.", default_mode="fortress"),

    dict(key="halloween", name="Halloween", emoji="🎃",
         match=_fixed((10, 25), (11, 1)),
         palette=(230, 130, 60), accent=(150, 90, 190), particles="bats",
         banner="Something stirs in the binder's shadow…", default_mode="veil",
         features=dict(spooky=True, night=True)),

    dict(key="winter", name="Winter Holidays", emoji="❄️",
         match=_fixed((12, 20), (12, 30)),
         palette=(200, 220, 240), accent=(196, 60, 60), particles="snow",
         banner="Snow settles on the open book. Season's greetings!",
         default_mode="classic", features=dict(snow=True)),

    dict(key="anniversary", name="Paperbound Anniversary", emoji="📖",
         match=_fixed((9, 12), (9, 14)),
         palette=(226, 200, 150), accent=(200, 168, 96), particles="confetti",
         banner="Another year of folded battles — thank you for playing!",
         default_mode="classic"),

    dict(key="risennight", name="Night of the Risen", emoji="🧟",
         match=_fixed((10, 1), (10, 3)),
         palette=(120, 150, 96), accent=(96, 150, 90), particles="bats",
         banner="The fallen do not rest tonight. Hold the page — together, if you must.",
         default_mode="risen", features=dict(risen=True, night=True)),
]


def all_events():
    return list(EVENTS)


# ── event-gated card variants ────────────────────────────────────────────────
# A holiday can offer a COSMETIC variant of existing cards — same stats & sigil,
# a themed recolour + name. These are available only while the event is active,
# and the deck builder lets you swap a card to/from its seasonal skin. Each entry
# is a variant-spec compatible with carddata.make_variant (tint/suffix/mods).
# Purely cosmetic by default (empty mods) so nothing about balance shifts.
SEASONAL_VARIANTS = {
    "halloween": dict(id="spooky", name="Spooky", suffix="Spooky",
                      tint=[150, 90, 190], mods={}, add_ability=None),
    "winter":    dict(id="frosted", name="Frosted", suffix="Frosted",
                      tint=[200, 220, 240], mods={}, add_ability=None),
    "valentines": dict(id="sweetheart", name="Sweetheart", suffix="Sweetheart",
                       tint=[240, 170, 195], mods={}, add_ability=None),
    "newyear":   dict(id="gilded_ny", name="Golden", suffix="Golden",
                      tint=[224, 190, 96], mods={}, add_ability=None),
    "harvest":   dict(id="autumnal", name="Autumnal", suffix="Autumnal",
                      tint=[214, 158, 96], mods={}, add_ability=None),
    "bloom":     dict(id="blooming", name="Blooming", suffix="Blooming",
                      tint=[240, 190, 210], mods={}, add_ability=None),
    "lunar":     dict(id="vermillion", name="Vermillion", suffix="Vermillion",
                      tint=[220, 80, 70], mods={}, add_ability=None),
    "stpatrick": dict(id="verdant", name="Verdant", suffix="Verdant",
                      tint=[120, 180, 110], mods={}, add_ability=None),
}


def seasonal_variant(date=None):
    """The variant-spec offered by the currently-active event, or None. Same for
    every device on the day, so a seasonal skin appears for everyone at once."""
    ev = active_event(date)
    if not ev:
        return None
    return SEASONAL_VARIANTS.get(ev["key"])


def active_event(date=None):
    """Return the event active on `date` (default: today), or None. If two windows
    overlap, the earlier one in the catalogue wins (order = priority)."""
    d = date or datetime.date.today()
    for ev in EVENTS:
        try:
            if ev["match"](d):
                return ev
        except Exception:
            continue
    return None


def days_until_next(date=None, horizon=120):
    """(event, days) for the next upcoming event within `horizon` days, or None.
    Handy for a menu teaser like 'Halloween in 6 days'."""
    d = date or datetime.date.today()
    for n in range(1, horizon + 1):
        cand = d + datetime.timedelta(days=n)
        ev = active_event(cand)
        if ev and not active_event(d):     # only if nothing is active now
            return (ev, n)
        if ev and active_event(d) and ev["key"] != active_event(d)["key"]:
            return (ev, n)
    return None


# ── ambient particle description (drawn by the game; pure data here) ──────────
def ambient_spec(ev):
    """Return a dict describing the ambient overlay for an event's particle type,
    or None. The game reads this to spawn/animate a display-only overlay; nothing
    here touches the sim."""
    if not ev:
        return None
    kind = ev.get("particles")
    specs = {
        "snow":      dict(count=70, color=(235, 242, 250), size=(2, 4), fall=(18, 40),
                          drift=14, shape="dot", sway=True),
        "leaves":    dict(count=40, color=(200, 130, 60), size=(4, 8), fall=(20, 46),
                          drift=26, shape="leaf", spin=True),
        "petals":    dict(count=44, color=(240, 190, 210), size=(3, 6), fall=(14, 34),
                          drift=30, shape="leaf", spin=True),
        "hearts":    dict(count=30, color=(224, 110, 140), size=(4, 8), fall=(12, 30),
                          drift=16, shape="heart", sway=True),
        "bats":      dict(count=18, color=(40, 30, 50), size=(6, 11), fall=(0, 0),
                          drift=60, shape="bat", flutter=True),
        "fireworks": dict(count=0, color=(255, 220, 140), size=(2, 3), fall=(0, 0),
                          drift=0, shape="burst", burst=True),
        "confetti":  dict(count=60, color=None, size=(3, 6), fall=(24, 60),
                          drift=40, shape="rect", spin=True, rainbow=True),
        "clovers":   dict(count=30, color=(96, 176, 96), size=(4, 7), fall=(16, 36),
                          drift=20, shape="clover", spin=True),
        "lanterns":  dict(count=14, color=(230, 90, 70), size=(8, 14), fall=(-14, -6),
                          drift=10, shape="lantern", rise=True, glow=True),
        "sun":       dict(count=20, color=(255, 226, 150), size=(3, 6), fall=(0, 0),
                          drift=8, shape="mote", glow=True, sway=True),
    }
    return specs.get(kind)


if __name__ == "__main__":
    import datetime as _dt
    print("Paperbound event calendar — sample dates:")
    samples = [
        ("Halloween", _dt.date(2026, 10, 31)),
        ("Christmas week", _dt.date(2026, 12, 25)),
        ("New Year", _dt.date(2027, 1, 1)),
        ("Valentine's", _dt.date(2026, 2, 14)),
        ("Lunar NY 2026", _dt.date(2026, 2, 17)),
        ("Easter window 2026", _easter(2026)),
        ("A plain Tuesday", _dt.date(2026, 5, 12)),
    ]
    for label, d in samples:
        ev = active_event(d)
        print(f"  {d}  {label:22} -> {ev['name']+' '+ev['emoji'] if ev else '(none)'}")
    # basic sanity
    assert active_event(_dt.date(2026, 10, 31))["key"] == "halloween"
    assert active_event(_dt.date(2026, 12, 25))["key"] == "winter"
    assert active_event(_dt.date(2027, 1, 1))["key"] == "newyear"
    assert active_event(_dt.date(2026, 5, 12)) is None
    assert ambient_spec(active_event(_dt.date(2026, 12, 25)))["shape"] == "dot"
    print("✅ events self-test passed")
