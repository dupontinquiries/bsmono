"""
PAPERBOUND — v3: worn paper, directional shading, extruded 3D units
Run interactively:   python game.py
Render screenshot:   python game.py --shot
Requires: pygame-ce
"""
import os, sys, math, random

HEADLESS = "--shot" in sys.argv
if HEADLESS:
    os.environ["SDL_VIDEODRIVER"] = "dummy"

# ── on-screen / IME keyboard support (Wayland text-input v1/v2/v3, mobile OSK) ──
# These hints must be set BEFORE pygame.init(). They ask SDL to draw the IME UI
# and to prefer the Wayland text-input protocol so a virtual keyboard (phosh,
# squeekboard, maliit, etc. on postmarketOS/Alpine) can feed the game. They're
# harmless on desktops. Individual hints are ignored by SDL builds that don't
# know them, so we set the whole set for maximum compatibility.
os.environ.setdefault("SDL_IME_SHOW_UI", "1")
os.environ.setdefault("SDL_HINT_IME_SHOW_UI", "1")
os.environ.setdefault("SDL_VIDEO_WAYLAND_PREFER_LIBDECOR", "1")
# allow the compositor's text-input to drive us even without a hardware keyboard
os.environ.setdefault("SDL_HINT_ENABLE_SCREEN_KEYBOARD", "1")

import pygame

# ── pygame / SDL_ttf compatibility guard ──────────────────────────────────
# On some interpreters (notably Python 3.14 with upstream `pygame` 2.6.1) the
# font module fails to load — a circular import leaves `pygame.font.Font`
# missing, which otherwise surfaces later as a cryptic crash the first time we
# render text. Detect it up front and print the actual fix instead. The
# community fork `pygame-ce` ships wheels for new Pythons and is what this game
# targets, so it's the recommended resolution.
def _check_pygame_font():
    try:
        pygame.font.init()
        _ = pygame.font.Font(None, 16)          # the call that fails when broken
        return True
    except Exception as e:
        pyver = "%d.%d.%d" % sys.version_info[:3]
        kind = "pygame-ce" if getattr(pygame, "IS_CE", 0) else "pygame"
        msg = (
            "\n"
            "════════════════════════════════════════════════════════════════\n"
            " Paperbound can't initialise pygame's font module.\n"
            f"   {kind} {getattr(pygame, '__version__', '?')} on Python {pyver}\n"
            f"   ({type(e).__name__}: {e})\n\n"
            " This is an upstream SDL_ttf / Python incompatibility (seen with\n"
            " Python 3.14 + the original `pygame` package), not a bug in the game.\n\n"
            " Fix — either:\n"
            "   • pip install -U pygame-ce      (community fork; has 3.14 wheels)\n"
            "     (uninstall the old one first: pip uninstall -y pygame)\n"
            "   • or run the game on Python 3.11 / 3.12.\n"
            "════════════════════════════════════════════════════════════════\n")
        if not HEADLESS:
            print(msg, file=sys.stderr)
        return False

_FONT_OK = _check_pygame_font()

try:
    import netplay
    from netplay import INPUT_DELAY, DEFAULT_PORT
    NET_OK = True
except Exception:
    NET_OK = False
    INPUT_DELAY, DEFAULT_PORT = 4, 7777

try:
    import inputs
    INPUTS_OK = True
except Exception:
    INPUTS_OK = False

try:
    import sound
    SOUND_OK = True
except Exception:
    SOUND_OK = False
    sound = None

try:
    import battlefsm
    from battlefsm import BattleFSM
    FSM_OK = True
except Exception:
    FSM_OK = False

def seat_team_local(seat):
    """Seats 0,1 → team 0 (bottom); 2,3 → team 1 (top). Local mirror of
    netplay.seat_team so it works even when networking is unavailable."""
    return 0 if seat < 2 else 1

W, H = 1280, 800
TPS = 20
DT = 1.0 / TPS
# Global pace multiplier: how fast real time maps to simulation time. 0.85 runs
# the whole battle 15% slower (more reaction time, less spammy) WITHOUT touching
# any per-tick math — the tick sequence is identical on every peer, so net
# lockstep determinism is unaffected. Applied only to the sim accumulator.
GAME_SPEED = 0.82875            # 0.85 slowed 2.5% for a slightly calmer pace

DESK      = (32, 22, 18)
DESK_GRAIN= (24, 16, 13)
PAGE      = (204, 186, 150)
PAGE_LINE = (184, 166, 130)
COVER     = (58, 22, 20)
RIVER     = (16, 18, 30)
RIVER_HI  = (52, 66, 104)
RIBBON    = (128, 30, 36)
import theme  # shared palette (mirrors snapclone-theme; see theme.py)
INK       = theme.color("text_primary")   # shared 'paper' palette
P_ACCENT  = theme.color("accent")
E_ACCENT  = (118, 52, 134)
GOLD      = (188, 146, 66)
WAX       = (150, 36, 42)
HAND_COL  = (38, 29, 25)
HAND_RIM  = (172, 118, 74)

BOOK = pygame.Rect(330, 36, 620, 640)
RIVER_Y = BOOK.centery
RIVER_H = 58
BRIDGE_XS = (BOOK.left + 155, BOOK.right - 155)
BRIDGE_HALF = 34
HAND_Y = 690

CX = W / 2
TOP_Y, BOT_Y = 86, 648
S_FAR, S_NEAR = 0.60, 1.12
UNIT_SCALE = 0.70      # visual only; sim sizes unchanged
TOWER_SCALE = 0.85

# accessibility: a global visual punch-up for units + towers so pieces read
# better on small screens. Purely cosmetic — sim hitboxes are unchanged, so it
# never affects gameplay or net determinism. Cycles through several steps for
# mobile, where the default can look too small.
PIECE_SCALE_STEPS = [1.0, 1.15, 1.3, 1.5, 1.75]
PIECE_SCALE_LABELS = ["100%", "115%", "130%", "150%", "175%"]
PIECE_SCALE_I = 0                # index into the steps (0 = standard)
BIG_UNITS = False               # kept for backward-compat with saved prefs
BIG_UNITS_SCALE = 1.15
def _piece_scale():
    return PIECE_SCALE_STEPS[PIECE_SCALE_I % len(PIECE_SCALE_STEPS)]
def unit_vis_scale():
    return UNIT_SCALE * _piece_scale()
def tower_vis_scale():
    return TOWER_SCALE * _piece_scale()

CANDLE_R = (1092, 210)
CANDLE_L = (150, 596)

# aspect the canvas is allowed to take. The game is a top-vs-bottom lane battler,
# so it reflows happily into portrait. We allow very tall phone aspect ratios so
# the canvas FILLS the screen (no letterbox); the extra vertical space becomes
# desk/environment above and below the table, with a vignette.
ASPECT_MIN = 0.42      # tallest (portrait) canvas w/h we'll build (~19.5:9 phones)
ASPECT_MAX = 2.10      # widest (landscape) canvas w/h we'll build
_LAYOUT_AR = None      # aspect the current geometry was built for

def relayout(win_w, win_h):
    """Reflow ONLY the on-screen projection to the window's aspect ratio (portrait
    phones included). The simulation world — BOOK, RIVER_Y, BRIDGE_XS, unit
    positions — is a FIXED coordinate space and never changes, so networked peers
    with different screen shapes stay bit-for-bit deterministic. What reflows is
    how that fixed world is *projected* onto the display: the page trapezoid, its
    vertical extents, the horizontal centre, the hand strip and the menu book.
    Returns (W, H)."""
    global W, H, HAND_Y, MENU_BOOK, _LAYOUT_AR, HAND_H
    global DISP_CX, DISP_TOP_Y, DISP_BOT_Y, DISP_BOOK, DISP_CANDLE_R, DISP_CANDLE_L
    ar = max(ASPECT_MIN, min(ASPECT_MAX, win_w / max(1, win_h)))
    if ar >= 1.0:                       # landscape-ish: fix height, widen
        H = 800
        W = int(round(H * ar))
    else:                               # portrait: fix width, grow tall
        W = 800
        H = int(round(W / ar))
    W = max(600, W); H = max(600, H)
    DISP_CX = W / 2
    _LAYOUT_AR = ar

    portrait = ar < 1.0
    # how "extra tall" this screen is beyond a comfortable 4:3-ish portrait —
    # the surplus becomes generous desk padding above the cards and below the
    # table so a tall phone fills edge-to-edge instead of letterboxing.
    tall = max(0.0, (1.0 / max(0.35, ar)) - 1.20)      # 0 at ~5:6, grows when taller

    HAND_H = int((150 if portrait else 128) + min(60, tall * 90))
    HAND_Y = H - HAND_H + 40

    # the page trapezoid fills the space above the hand; on very tall screens we
    # leave a taller top desk band (cards get more breathing room up top) and a
    # bottom desk band, and the table extends toward both edges via the vignette.
    top_frac = (0.05 if portrait else 0.045) + min(0.10, tall * 0.14)
    top_margin = int(H * top_frac)
    side_margin = int(W * (0.045 if portrait else 0.11))
    board_bottom = H - HAND_H - int(H * (0.02 + min(0.05, tall * 0.06)))
    DISP_BOOK = pygame.Rect(side_margin, top_margin,
                            W - 2 * side_margin, board_bottom - top_margin)
    DISP_TOP_Y = DISP_BOOK.top + int(DISP_BOOK.h * 0.07)
    DISP_BOT_Y = DISP_BOOK.bottom - int(DISP_BOOK.h * 0.02)
    DISP_CANDLE_R = (min(W - 40, DISP_BOOK.right + int(side_margin * 0.55)), int(H * 0.26))
    DISP_CANDLE_L = (max(20, DISP_BOOK.left - int(side_margin * 0.55)), int(H * 0.74))

    mm = int(min(W, H) * 0.06)
    MENU_BOOK = pygame.Rect(mm + int(W * 0.02), mm,
                            W - 2 * (mm + int(W * 0.02)), H - 2 * mm)
    return W, H

HAND_H = 128
# display-projection params (reflowed by relayout); the sim uses the fixed BOOK
DISP_CX = W / 2
DISP_TOP_Y, DISP_BOT_Y = TOP_Y, BOT_Y
DISP_BOOK = pygame.Rect(BOOK)
DISP_CANDLE_R, DISP_CANDLE_L = CANDLE_R, CANDLE_L


# ─────────────────────────────────────────────────────────────────────────
# GRAPHICS PRESETS
# "low"           — the original fast renderer (flat fills, few particles)
# "photorealistic"— soft shadows, bloom, film grain, chromatic edges, higher
#                   light-step counts, ambient occlusion under units, richer
#                   paper texture. Heavier but much prettier.
# ─────────────────────────────────────────────────────────────────────────
GFX = "low"                     # active preset; toggled from the Options page
GFX_PRESETS = ("low", "photorealistic")
SUPERSAMPLE = True              # in photoreal, render the battle at 1.5x then
                                # downscale for true anti-aliased origami + paper
SS_FACTOR = 1.5

def gfx_hi():
    return GFX == "photorealistic"

# geometry globals a supersampled render temporarily scales (arena-dependent, so
# we snapshot the *live* values each time and scale from those, then restore).
_SS_KEYS = ("W", "H", "HAND_Y", "LIGHT_MAIN", "LIGHTS",
            # ONLY display-projection globals scale for supersampling. The sim
            # world (BOOK, RIVER_Y, BRIDGE_XS, TOP_Y, BOT_Y, unit coords) is a
            # FIXED space and must NOT be touched, or proj()'s t/scale and the
            # view-flip drift and units land wrong.
            "DISP_CX", "DISP_TOP_Y", "DISP_BOT_Y", "DISP_BOOK",
            "DISP_CANDLE_L", "DISP_CANDLE_R")

def _ss_snapshot():
    g = globals()
    return {k: g[k] for k in _SS_KEYS}

def _ss_apply(f, snap):
    """Scale the DISPLAY projection by f (restored after render). The simulation
    world is untouched, so proj() keeps the same t and only the on-screen mapping
    grows into the hi-res buffer."""
    g = globals()
    g["W"] = int(snap["W"] * f); g["H"] = int(snap["H"] * f)
    g["HAND_Y"] = int(snap["HAND_Y"] * f)
    g["LIGHT_MAIN"] = (snap["LIGHT_MAIN"][0] * f, snap["LIGHT_MAIN"][1] * f)
    g["LIGHTS"] = [((lx * f, ly * f), rr * f, col) for ((lx, ly), rr, col) in snap["LIGHTS"]]
    db = snap["DISP_BOOK"]
    g["DISP_CX"] = snap["DISP_CX"] * f
    g["DISP_TOP_Y"] = snap["DISP_TOP_Y"] * f
    g["DISP_BOT_Y"] = snap["DISP_BOT_Y"] * f
    g["DISP_BOOK"] = pygame.Rect(int(db.x * f), int(db.y * f), int(db.w * f), int(db.h * f))
    g["DISP_CANDLE_L"] = (snap["DISP_CANDLE_L"][0] * f, snap["DISP_CANDLE_L"][1] * f)
    g["DISP_CANDLE_R"] = (snap["DISP_CANDLE_R"][0] * f, snap["DISP_CANDLE_R"][1] * f)

def _ss_restore(snap):
    globals().update(snap)

# hi-res board cache, keyed by (arena, W) so it's built once per arena at scale
_SS_BOARD = {"key": None, "surf": None}
def _ss_board_cache():
    key = (CUR_ARENA, W, H)
    if _SS_BOARD["key"] != key:
        _SS_BOARD["surf"] = build_board_static()
        _SS_BOARD["key"] = key
    return _SS_BOARD["surf"]

def _ss_unscale_hot(f):
    """After a supersampled battle render, HOT holds rects/points in SS-space.
    Divide them back to base canvas coords so click hit-testing lines up."""
    inv = 1.0 / f
    def sr(r):
        return pygame.Rect(int(r.x * inv), int(r.y * inv), int(r.w * inv), int(r.h * inv))
    if isinstance(HOT.get("forfeit"), pygame.Rect):
        HOT["forfeit"] = sr(HOT["forfeit"])
    for keylist in ("tablefinds", "hand_b"):
        if keylist in HOT:
            HOT[keylist] = [(sr(r), payload) for (r, payload) in HOT[keylist]]

# ── Mobile / power settings ──────────────────────────────────────────────
#  FPS_CAP     : frame cap, cycled 60 → 30 → 12.
#                60 = smooth, 30 = battery, 12 = "cartoon" (animation on twos:
#                held poses + hand-drawn motion smears instead of tweening).
#  ORIENT_LOCK : legacy flag; the UI now *reflows* to any window aspect
#                (portrait phones included) via relayout(), so this is a no-op
#                kept only for save-file compatibility.
#  PAUSE_BG    : pause + stop drawing when the window loses focus (backgrounded).
FPS_CAP = 60
BATTERY_SAVER = True            # throttle framerate in static scenes to save battery
_last_input_t = 0.0             # wall-clock of the last user input (for idle detection)
# frame-rate options, cycled in Options. 12 = "cartoon" (animation on twos);
# 30 = battery; 60 = smooth; 90..500 = high-refresh displays (the game caps at
# whatever your panel actually supports — asking for 500 on a 120Hz screen just
# means "uncapped up to the panel").
FPS_CYCLE = [12, 30, 60, 90, 120, 240, 360, 480, 500]
ORIENT_LOCK = "landscape"       # legacy; layout auto-reflows regardless
PAUSE_BG = True
_APP_ACTIVE = True              # flipped by APP focus events
VOLUME = 0.7                    # 0..1 master audio volume (Options slider)
MUTED = False

def sfx(name, gain=1.0):
    """Fire a one-shot sound effect (safe no-op when audio is unavailable)."""
    if SOUND_OK and sound is not None:
        sound.play(name, gain)

def set_ambient(key):
    if SOUND_OK and sound is not None:
        sound.set_ambient(key)

def _present_geom(ww, wh):
    """The canvas is reflowed to match the window's aspect, so presentation is a
    simple fit-and-centre (any residual bar is a couple of pixels from integer
    rounding, not a real letterbox)."""
    sc = min(ww / W, wh / H)
    dw, dh = W * sc, H * sc
    return {"rotate": False, "sc": sc, "ox": (ww - dw) / 2, "oy": (wh - dh) / 2,
            "dw": dw, "dh": dh}

def _win_to_canvas_geom(pos, ww, wh):
    tf = _present_geom(ww, wh)
    sc = tf["sc"] or 1.0
    return ((pos[0] - tf["ox"]) / sc, (pos[1] - tf["oy"]) / sc)

def cartoon_on():
    # 12fps kicks the renderer into hand-animated "on twos" cartoon mode
    return FPS_CAP == 12

# ── Alternative game modes ────────────────────────────────────────────────
# A single toggle that reshapes every gameplay path (Play, Local 1v1, LAN).
MODES = {
    "classic":   dict(name="Classic",      blurb="Standard rules.",
                      ink_rate=1.0, tower_hp=1.0, fog=False, start_ink=5),
    "blitz":     dict(name="Blitz",        blurb="Double ink · frantic.",
                      ink_rate=2.0, tower_hp=1.0, fog=False, start_ink=6),
    "sudden":    dict(name="Sudden Death",  blurb="Towers at half HP.",
                      ink_rate=1.2, tower_hp=0.5, fog=False, start_ink=6),
    "fog":       dict(name="Fog of War",   blurb="See only what you scout.",
                      ink_rate=1.0, tower_hp=1.0, fog=True,  start_ink=5),
    "doubleink": dict(name="Flood",        blurb="Start with full ink.",
                      ink_rate=1.4, fog=False, tower_hp=1.0, start_ink=10),
    "drought":   dict(name="Drought",      blurb="Ink trickles in slowly.",
                      ink_rate=0.55, tower_hp=1.0, fog=False, start_ink=4),
    "fortress":  dict(name="Fortress",     blurb="Towers stand at double HP.",
                      ink_rate=1.1, tower_hp=2.0, fog=False, start_ink=6),
    "tempest":   dict(name="Tempest",      blurb="Fast ink AND fragile towers.",
                      ink_rate=1.8, tower_hp=0.6, fog=False, start_ink=7),
    "veil":      dict(name="The Veil",     blurb="Fog of war, at a frantic pace.",
                      ink_rate=1.6, tower_hp=1.0, fog=True,  start_ink=6),
    "living":    dict(name="Living Page",   blurb="An AI Director rewrites the battle in real time.",
                      ink_rate=1.3, tower_hp=1.0, fog=False, start_ink=6),
    "deluge":    dict(name="Deluge",        blurb="20 ink cap · ink flows 10% faster.",
                      ink_rate=1.1, tower_hp=1.0, fog=False, start_ink=8, ink_max=20),
    "lastword":  dict(name="Last Word",     blurb="No flank towers — only the king stands.",
                      ink_rate=1.0, tower_hp=1.0, fog=False, start_ink=6, king_only=True),
    "risen":     dict(name="The Risen",      blurb="The fallen rise again — a hostile horde turns on you both.",
                      ink_rate=1.2, tower_hp=1.0, fog=False, start_ink=6, risen=True),
}
MODE_ORDER = ["classic", "blitz", "sudden", "fog", "doubleink",
              "drought", "fortress", "tempest", "veil", "living",
              "deluge", "lastword", "risen"]
MODE = "classic"                # active mode; toggled from menu / arena select
def mode_cfg():
    return MODES[MODE]

def ink_cap(mode=None):
    """The max ink for a mode (default 10). Some modes raise the ceiling."""
    return MODES.get(mode or MODE, {}).get("ink_max", 10)

# ── Tower Troops ──────────────────────────────────────────────────────────
# The two flanking defensive structures can be swapped for different "tower
# troops", each a distinct emplacement with its own weapon, health and quirk.
#   hp/dmg/rng/cd    : base defensive stats (scaled by mode tower_hp)
#   weapon           : how it fires — "bolt" (single shot), "splash" (aoe on hit),
#                      "twin" (fires two staggered bolts), "frost" (slows on hit),
#                      "mortar" (long range, slow, heavy), "none" (support only)
#   special          : passive quirk — "heal" (mends nearby allies), "thorn"
#                      (retaliates when hit), None
#   color            : accent tint for the roof/marker
TOWER_TROOPS = {
    "spire":   dict(name="Ink Spire",     blurb="Balanced all-rounder.",
                    hp=1500, dmg=60, rng=235, cd=0.9, weapon="bolt",  special=None,
                    color=(150, 120, 200)),
    "bastion": dict(name="Wax Bastion",   blurb="Tanky, slow, heavy hits.",
                    hp=2400, dmg=95, rng=200, cd=1.5, weapon="bolt",  special="thorn",
                    color=(180, 90, 70)),
    "scatter": dict(name="Quill Scatter", blurb="Splash shots, low HP.",
                    hp=1100, dmg=44, rng=210, cd=1.0, weapon="splash", special=None,
                    color=(120, 170, 110)),
    "twin":    dict(name="Twin Nib",      blurb="Fires two rapid bolts.",
                    hp=1300, dmg=34, rng=225, cd=0.55, weapon="twin", special=None,
                    color=(90, 160, 200)),
    "frost":   dict(name="Frost Font",    blurb="Chills what it hits.",
                    hp=1350, dmg=40, rng=225, cd=1.0, weapon="frost", special=None,
                    color=(150, 200, 240)),
    "mortar":  dict(name="Ink Mortar",    blurb="Long range, slow, huge.",
                    hp=1250, dmg=150, rng=340, cd=2.4, weapon="mortar", special=None,
                    color=(160, 140, 80)),
    "chapel":  dict(name="Mending Chapel", blurb="No gun; heals your side.",
                    hp=1600, dmg=0, rng=0, cd=1.0, weapon="none", special="heal",
                    color=(230, 200, 120)),
    "rookery": dict(name="Paper Rookery",  blurb="No gun; hatches paper birds.",
                    hp=1500, dmg=0, rng=0, cd=1.0, weapon="none", special="spawn",
                    color=(150, 170, 150), spawn_key="papermoth", spawn_cd=6.0,
                    spawn_max=4),
}
TOWER_ORDER = ["spire", "bastion", "scatter", "twin", "frost", "mortar", "chapel", "rookery"]
TOWER_TROOP = "spire"           # active loadout for the player's flank towers

LIGHT_MAIN = CANDLE_R            # main light position (drives shadows + lambert)
AMBIENT = (56, 50, 70)          # cold fill in the dark
LIGHTS = [(CANDLE_R, 1020, (235, 205, 162)), (CANDLE_L, 580, (196, 164, 122)),
          ((640, 400), 660, (96, 88, 80))]
WEATHER = None                  # None | "lightning" | "embers" | "snow"
HAZARD = None                   # None | "lava" | "fissures"
POI = None                      # None | "inktubs"
CUR_ARENA = "study"

# themed map pool. book rect / top_y / bot_y let special maps use a bigger field.
ARENAS = {
    "study": dict(name="The Binder's Study", chapter="Chapter I",
        page=(204, 186, 150), page_line=(184, 166, 130), desk=(32, 22, 18),
        desk_grain=(24, 16, 13), cover=(58, 22, 20), river=(16, 18, 30),
        river_hi=(52, 66, 104), ribbon=(128, 30, 36),
        book=(330, 36, 620, 640), top_y=86, bot_y=648,
        ambient=(56, 50, 70), light_main=CANDLE_R,
        lights=[(CANDLE_R, 1020, (235, 205, 162)), (CANDLE_L, 580, (196, 164, 122)),
                ((640, 400), 660, (96, 88, 80))],
        candles=True, weather=None, hazard=None, poi=None),
    "storm": dict(name="The Weather Page", chapter="Chapter III",
        page=(150, 158, 170), page_line=(126, 136, 150), desk=(20, 22, 30),
        desk_grain=(14, 16, 24), cover=(28, 34, 52), river=(20, 26, 44),
        river_hi=(70, 96, 150), ribbon=(70, 84, 120),
        book=(300, 24, 680, 664), top_y=74, bot_y=664,
        ambient=(46, 52, 78), light_main=(300, 40),
        lights=[((300, 40), 1100, (150, 170, 210)), ((980, 700), 460, (90, 100, 140))],
        candles=False, weather="lightning", hazard=None, poi=None),
    "volcano": dict(name="The Ashen Leaf", chapter="Chapter VI — Finale",
        page=(120, 86, 74), page_line=(96, 66, 56), desk=(26, 14, 12),
        desk_grain=(18, 9, 8), cover=(52, 18, 14), river=(60, 20, 12),
        river_hi=(200, 90, 40), ribbon=(120, 40, 24),
        book=(310, 30, 660, 652), top_y=80, bot_y=656,
        ambient=(70, 40, 40), light_main=(640, 120),
        lights=[((640, 120), 900, (240, 120, 60)), ((640, 700), 520, (200, 80, 40))],
        candles=False, weather="embers", hazard="lava", poi=None),
    "quake": dict(name="The Fault Lines", chapter="Chapter IV",
        page=(190, 168, 132), page_line=(168, 148, 116), desk=(30, 24, 18),
        desk_grain=(22, 17, 12), cover=(60, 40, 24), river=(20, 20, 26),
        river_hi=(60, 70, 96), ribbon=(120, 78, 40),
        book=(300, 24, 680, 664), top_y=74, bot_y=664,
        ambient=(58, 52, 60), light_main=(980, 160),
        lights=[((980, 160), 980, (224, 200, 150)), ((240, 640), 480, (150, 132, 100))],
        candles=True, weather=None, hazard="fissures", poi=None),
    "inktubs": dict(name="The Spilled Wells", chapter="Chapter V",
        page=(176, 186, 168), page_line=(150, 164, 146), desk=(22, 26, 24),
        desk_grain=(15, 18, 17), cover=(30, 46, 40), river=(14, 20, 22),
        river_hi=(60, 120, 110), ribbon=(60, 100, 84),
        book=(310, 30, 660, 652), top_y=80, bot_y=656,
        ambient=(48, 58, 56), light_main=(640, 380),
        lights=[((640, 130), 760, (150, 210, 190)), ((640, 640), 520, (110, 170, 150))],
        candles=False, weather=None, hazard=None, poi="inktubs"),
    "frost": dict(name="The Cold Margin", chapter="Chapter II",
        page=(206, 214, 224), page_line=(184, 194, 208), desk=(24, 28, 34),
        desk_grain=(16, 20, 26), cover=(40, 50, 64), river=(30, 44, 60),
        river_hi=(120, 160, 200), ribbon=(90, 110, 140),
        book=(320, 34, 640, 644), top_y=82, bot_y=650,
        ambient=(58, 66, 82), light_main=(820, 120),
        lights=[((820, 120), 940, (200, 216, 240)), ((300, 640), 460, (140, 156, 186))],
        candles=False, weather="snow", hazard=None, poi=None),
    "sunken": dict(name="The Drowned Chapter", chapter="Chapter VII",
        page=(120, 140, 148), page_line=(96, 118, 128), desk=(14, 22, 26),
        desk_grain=(9, 15, 18), cover=(24, 44, 52), river=(30, 70, 90),
        river_hi=(90, 150, 180), ribbon=(50, 96, 110),
        book=(310, 30, 660, 652), top_y=80, bot_y=656,
        ambient=(40, 60, 72), light_main=(640, 120),
        lights=[((640, 120), 820, (140, 190, 210)), ((640, 660), 480, (80, 140, 170))],
        candles=False, weather=None, hazard=None, poi="inktubs"),
    "aurora": dict(name="The Aurora Folio", chapter="Chapter VIII",
        page=(30, 40, 54), page_line=(44, 58, 76), desk=(10, 12, 20),
        desk_grain=(6, 8, 14), cover=(20, 30, 48), river=(18, 28, 46),
        river_hi=(90, 220, 180), ribbon=(60, 120, 150),
        book=(320, 34, 640, 644), top_y=82, bot_y=650,
        ambient=(30, 44, 66), light_main=(640, 80),
        lights=[((420, 90), 900, (90, 220, 170)), ((880, 120), 700, (150, 120, 220)),
                ((640, 700), 460, (60, 110, 140))],
        candles=False, weather="aurora", hazard=None, poi=None),
    "dunes": dict(name="The Sandworn Leaf", chapter="Chapter IX",
        page=(212, 184, 132), page_line=(190, 160, 108), desk=(40, 30, 18),
        desk_grain=(30, 22, 12), cover=(96, 66, 30), river=(150, 120, 70),
        river_hi=(210, 180, 120), ribbon=(150, 96, 40),
        book=(300, 24, 680, 664), top_y=74, bot_y=664,
        ambient=(78, 64, 44), light_main=(760, 110),
        lights=[((760, 110), 1040, (240, 210, 150)), ((300, 620), 460, (200, 170, 110))],
        candles=False, weather="sand", hazard="quicksand", poi=None),
    "clockwork": dict(name="The Clockwork Margin", chapter="Chapter X — Finale",
        page=(160, 140, 110), page_line=(130, 112, 84), desk=(28, 22, 16),
        desk_grain=(20, 15, 10), cover=(72, 52, 28), river=(40, 34, 24),
        river_hi=(150, 120, 70), ribbon=(140, 100, 44),
        book=(310, 30, 660, 652), top_y=80, bot_y=656,
        ambient=(60, 52, 40), light_main=(640, 110),
        lights=[((640, 110), 940, (232, 200, 140)), ((300, 640), 440, (180, 150, 100)),
                ((980, 640), 440, (180, 150, 100))],
        candles=True, weather=None, hazard="gears", poi=None),
    "sandstorm": dict(name="The Sirocco Page", chapter="Chapter XI",
        page=(214, 176, 118), page_line=(190, 150, 96), desk=(58, 42, 24),
        desk_grain=(44, 31, 17), cover=(120, 84, 40), river=(176, 142, 86),
        river_hi=(226, 196, 132), ribbon=(168, 112, 48),
        book=(300, 24, 680, 664), top_y=74, bot_y=664,
        ambient=(96, 76, 48), light_main=(640, 90),
        lights=[((640, 90), 1120, (250, 210, 140)), ((300, 620), 420, (210, 170, 100)),
                ((980, 620), 420, (210, 170, 100))],
        candles=False, weather="sandstorm", hazard="quicksand", poi=None),
    "sandbar": dict(name="The Tideline Leaf", chapter="Chapter XII",
        page=(232, 216, 176), page_line=(210, 194, 154), desk=(30, 46, 58),
        desk_grain=(20, 34, 44), cover=(64, 108, 128), river=(74, 158, 178),
        river_hi=(150, 214, 224), ribbon=(210, 180, 120),
        book=(310, 30, 660, 652), top_y=80, bot_y=656,
        ambient=(78, 104, 120), light_main=(760, 90),
        lights=[((760, 90), 1080, (255, 238, 196)), ((320, 600), 420, (150, 210, 220))],
        candles=False, weather="gulls", hazard="tide", poi=None),
    "outcrop": dict(name="The Cataract Crag", chapter="Chapter XIII — Finale",
        page=(150, 156, 150), page_line=(122, 130, 124), desk=(24, 30, 30),
        desk_grain=(16, 22, 22), cover=(52, 66, 62), river=(60, 120, 140),
        river_hi=(150, 210, 224), ribbon=(90, 110, 100),
        book=(300, 24, 680, 664), top_y=74, bot_y=664,
        ambient=(58, 72, 74), light_main=(520, 90),
        lights=[((520, 90), 1020, (210, 224, 224)), ((900, 120), 620, (170, 200, 210)),
                ((640, 660), 420, (110, 150, 160))],
        candles=False, weather="mist", hazard="waterfalls", poi=None),
    "greenhouse": dict(name="The Pressed Garden", chapter="Chapter XIV",
        page=(196, 206, 168), page_line=(168, 182, 142), desk=(26, 34, 22),
        desk_grain=(18, 24, 15), cover=(44, 66, 34), river=(30, 60, 46),
        river_hi=(90, 170, 120), ribbon=(80, 130, 70),
        book=(310, 30, 660, 652), top_y=80, bot_y=656,
        ambient=(58, 72, 54), light_main=(700, 120),
        lights=[((700, 120), 980, (200, 230, 170)), ((320, 620), 460, (150, 200, 130))],
        candles=False, weather=None, hazard=None, poi="inktubs"),
    "observatory": dict(name="The Star Chart", chapter="Chapter XV",
        page=(36, 42, 62), page_line=(52, 60, 88), desk=(10, 12, 22),
        desk_grain=(6, 8, 16), cover=(24, 28, 52), river=(20, 26, 48),
        river_hi=(120, 140, 220), ribbon=(70, 84, 150),
        book=(320, 34, 640, 644), top_y=82, bot_y=650,
        ambient=(34, 40, 66), light_main=(640, 90),
        lights=[((500, 100), 880, (150, 170, 240)), ((820, 130), 700, (120, 130, 220)),
                ((640, 680), 440, (70, 90, 150))],
        candles=True, weather="aurora", hazard=None, poi=None),
    "forge": dict(name="The Bindery Forge", chapter="Chapter XVI — Finale",
        page=(150, 116, 96), page_line=(122, 90, 72), desk=(28, 16, 12),
        desk_grain=(20, 11, 8), cover=(66, 30, 20), river=(50, 24, 16),
        river_hi=(220, 120, 60), ribbon=(150, 70, 34),
        book=(300, 24, 680, 664), top_y=74, bot_y=664,
        ambient=(72, 48, 40), light_main=(640, 110),
        lights=[((640, 110), 960, (255, 160, 80)), ((300, 640), 460, (230, 120, 60)),
                ((980, 640), 460, (230, 120, 60))],
        candles=True, weather="embers", hazard="lava", poi=None),
    "moth": dict(name="The Moth Hour", chapter="Chapter XVII",
        page=(44, 40, 58), page_line=(64, 58, 82), desk=(14, 12, 20),
        desk_grain=(8, 7, 14), cover=(34, 28, 50), river=(28, 26, 46),
        river_hi=(150, 140, 220), ribbon=(120, 90, 160),
        book=(310, 30, 660, 652), top_y=80, bot_y=656,
        ambient=(40, 36, 58), light_main=(640, 100),
        lights=[((640, 100), 900, (180, 170, 235)), ((360, 620), 440, (150, 130, 210)),
                ((940, 150), 520, (120, 110, 190))],
        candles=True, weather="aurora", hazard=None, poi=None),
    "tidepool": dict(name="The Tidepool Margin", chapter="Chapter XVIII",
        page=(206, 220, 214), page_line=(168, 196, 190), desk=(22, 40, 42),
        desk_grain=(14, 28, 30), cover=(40, 76, 78), river=(46, 132, 150),
        river_hi=(150, 224, 224), ribbon=(90, 150, 150),
        book=(300, 26, 680, 660), top_y=76, bot_y=662,
        ambient=(62, 96, 100), light_main=(700, 100),
        lights=[((700, 100), 1000, (200, 236, 232)), ((320, 600), 440, (150, 210, 210))],
        candles=False, weather="gulls", hazard="tide", poi="inktubs"),
    "ledger": dict(name="The Red Ledger", chapter="Chapter XIX — Finale",
        page=(210, 194, 176), page_line=(200, 120, 110), desk=(34, 20, 18),
        desk_grain=(24, 13, 12), cover=(96, 34, 30), river=(120, 40, 40),
        river_hi=(220, 130, 120), ribbon=(180, 50, 46),
        book=(300, 24, 680, 664), top_y=74, bot_y=664,
        ambient=(84, 52, 48), light_main=(640, 110),
        lights=[((640, 110), 980, (255, 200, 180)), ((300, 640), 440, (220, 120, 110)),
                ((980, 640), 440, (220, 120, 110))],
        candles=True, weather="lightning", hazard="fissures", poi=None),
}

def set_arena(key):
    global CUR_ARENA, PAGE, PAGE_LINE, DESK, DESK_GRAIN, COVER, RIVER, RIVER_HI, RIBBON
    global BOOK, RIVER_Y, BRIDGE_XS, TOP_Y, BOT_Y, AMBIENT, LIGHTS, LIGHT_MAIN
    global WEATHER, HAZARD, POI, BOARD_STATIC
    a = ARENAS[key]; CUR_ARENA = key
    PAGE = a["page"]; PAGE_LINE = a["page_line"]; DESK = a["desk"]
    DESK_GRAIN = a["desk_grain"]; COVER = a["cover"]; RIVER = a["river"]
    RIVER_HI = a["river_hi"]; RIBBON = a["ribbon"]
    # BOOK / RIVER_Y / BRIDGE_XS / TOP_Y / BOT_Y are the FIXED simulation world
    # (identical on every peer regardless of screen shape → determinism holds).
    BOOK = pygame.Rect(*a["book"]); RIVER_Y = BOOK.centery
    BRIDGE_XS = (BOOK.left + 155, BOOK.right - 155)
    TOP_Y, BOT_Y = a["top_y"], a["bot_y"]
    # Lights are drawn in DISPLAY space, so remap the arena's baked light
    # positions from its design-space board onto the reflowed display board.
    ref = BOOK
    def _remap(px, py):
        fx = (px - ref.x) / max(1, ref.w); fy = (py - ref.y) / max(1, ref.h)
        return (DISP_BOOK.x + fx * DISP_BOOK.w, DISP_BOOK.y + fy * DISP_BOOK.h)
    AMBIENT = a["ambient"]
    LIGHTS = [(_remap(lx, ly), rad * (DISP_BOOK.w / max(1, ref.w)), col)
              for (lx, ly), rad, col in a["lights"]]
    LIGHT_MAIN = _remap(*a["light_main"])
    WEATHER = a["weather"]; HAZARD = a["hazard"]; POI = a["poi"]
    BOARD_STATIC = None

random.seed(11)

# which boss haunts which arena (single-player). None = plain skirmish.
ARENA_BOSS = {
    "study": "angler",
    "inktubs": "scribe",
    "sunken": "scribe",
    "clockwork": "clockking",
    "volcano": "angler",
    "moth": "trickster",
    "observatory": "trickster",
}
def boss_for(arena):
    return ARENA_BOSS.get(arena)

CARDS = {
    "sabertooth": dict(name="Sabertooth", cost=4, hp=520, dmg=95, spd=62, rng=26,
                       size=26, flying=False, paper=(196, 138, 76), sigil="pounce"),
    "pangolin":   dict(name="Pangolin", cost=4, hp=780, dmg=42, spd=40, rng=24,
                       size=24, flying=False, paper=(142, 106, 74), sigil="armored"),
    "crane":      dict(name="Paper Crane", cost=2, hp=210, dmg=48, spd=70, rng=26,
                       size=20, flying=True,  paper=(224, 218, 204), sigil="feather"),
    "polarbear":  dict(name="Polar Bear", cost=7, hp=1650, dmg=120, spd=30, rng=28,
                       size=34, flying=False, paper=(208, 214, 222), sigil="chill"),
    "wolverine":  dict(name="Wolverine", cost=3, hp=340, dmg=70, spd=74, rng=22,
                       size=20, flying=False, paper=(84, 62, 46), sigil="frenzy"),
    "batcolony":  dict(name="Bat Colony", cost=3, hp=140, dmg=34, spd=78, rng=22,
                       size=15, flying=True,  paper=(58, 54, 68), sigil="drinker", count=3),
    "gorilla":    dict(name="Silverback", cost=6, hp=1250, dmg=105, spd=36, rng=26,
                       size=31, flying=False, paper=(112, 112, 120), sigil="slam"),
    "baobab":     dict(name="Baobab", cost=5, hp=1100, dmg=0, spd=0, rng=0,
                       size=30, flying=False, paper=(154, 116, 78), sigil="taunt", building=True),
    "frog":       dict(name="Dart Frog", cost=2, hp=180, dmg=30, spd=64, rng=20,
                       size=17, flying=False, paper=(78, 158, 84), sigil="toxic", riverhop=True),
    "cassowary":  dict(name="Cassowary", cost=4, hp=430, dmg=88, spd=80, rng=22,
                       size=25, flying=False, paper=(40, 40, 46), sigil="sharp"),
    "honeybadger":dict(name="Honey Badger", cost=5, hp=560, dmg=80, spd=58, rng=20,
                       size=22, flying=False, paper=(60, 56, 52), sigil="relentless"),
    "mammoth":    dict(name="Mammoth", cost=8, hp=2100, dmg=140, spd=26, rng=30,
                       size=38, flying=False, paper=(150, 118, 84), sigil="towerbane"),
    "dragontree": dict(name="Dragon's Blood Tree", cost=4, hp=820, dmg=0, spd=0, rng=0,
                       size=28, flying=False, paper=(126, 44, 42), sigil="sap", building=True),
    "mothswarm":  dict(name="Ink Moth Swarm", cost=3, hp=150, dmg=30, spd=82, rng=20,
                       size=14, flying=True, paper=(84, 78, 96), sigil="drinker", count=3),
    "wolfpack":   dict(name="Wolf Pack", cost=4, hp=260, dmg=52, spd=72, rng=20,
                       size=18, flying=False, paper=(96, 92, 100), sigil="manyfold", count=3),
    "mantis":     dict(name="Paper Mantis", cost=3, hp=300, dmg=54, spd=66, rng=22,
                       size=20, flying=False, paper=(120, 158, 92), sigil="doublestrike"),
    "terrorbird": dict(name="Terror Bird", cost=4, hp=470, dmg=76, spd=70, rng=22,
                       size=26, flying=False, paper=(150, 96, 70), sigil="rush"),
    "sloth":      dict(name="Ground Sloth", cost=5, hp=2400, dmg=70, spd=24, rng=24,
                       size=33, flying=False, paper=(126, 110, 88), sigil="dormant"),
    "shark":      dict(name="Great White", cost=6, hp=1200, dmg=150, spd=68, rng=24,
                       size=30, flying=False, paper=(150, 158, 168), sigil="beach", riverhop=True),
    "squid":      dict(name="Deep Squid", cost=5, hp=740, dmg=64, spd=40, rng=40,
                       size=27, flying=False, paper=(120, 70, 120), sigil="grasp", riverhop=True),
    "monkeypuzzle":dict(name="Monkey Puzzle", cost=3, hp=680, dmg=0, spd=0, rng=0,
                       size=27, flying=False, paper=(74, 96, 66), sigil="thorns", building=True),
    "locust":     dict(name="Locust Plague", cost=5, hp=90, dmg=22, spd=48, rng=120,
                       size=16, flying=True, paper=(120, 108, 60), sigil="plague", count=4),
    # NEW 8 CARDS (Phase 1 Expansion)
    "ravencrows": dict(name="Raven Crows", cost=3, hp=120, dmg=40, spd=86, rng=24,
                       size=13, flying=True, paper=(30, 28, 36), sigil="swiftdive", count=4),
    "inkgolem":   dict(name="Ink Golem", cost=6, hp=1600, dmg=85, spd=20, rng=22,
                       size=36, flying=False, paper=(24, 22, 28), sigil="inksplash"),
    "tidalwave":  dict(name="Tidal Wave", cost=7, hp=2000, dmg=120, spd=18, rng=200,
                       size=40, flying=False, paper=(70, 130, 180), sigil="tidal"),
    "piper":      dict(name="Pipe Piper", cost=4, hp=380, dmg=0, spd=60, rng=0,
                       size=22, flying=False, paper=(160, 130, 90), sigil="charm", building=True),
    "voidcritter": dict(name="Void Critter", cost=5, hp=500, dmg=60, spd=65, rng=18,
                       size=24, flying=False, paper=(40, 20, 60), sigil="steal"),
    "frostmage":  dict(name="Frost Mage", cost=4, hp=350, dmg=50, spd=55, rng=30,
                       size=20, flying=False, paper=(180, 210, 240), sigil="freeze"),
    "ironsmith":  dict(name="Iron Smith", cost=5, hp=900, dmg=70, spd=32, rng=20,
                       size=28, flying=False, paper=(140, 140, 150), sigil="forge", building=True),
    "shadowcat":  dict(name="Shadow Cat", cost=3, hp=280, dmg=65, spd=75, rng=18,
                       size=18, flying=False, paper=(50, 48, 56), sigil="stealth"),
    # NEW 8 CARDS (Phase 2 Expansion) — reach 38 total
    "scarab":     dict(name="Gilt Scarab", cost=2, hp=240, dmg=36, spd=70, rng=18,
                       size=16, flying=False, paper=(150, 122, 40), sigil="burrow"),
    "heron":      dict(name="Ash Heron", cost=4, hp=360, dmg=92, spd=64, rng=120,
                       size=24, flying=True, paper=(180, 170, 160), sigil="snipe"),
    "boar":       dict(name="Bristle Boar", cost=4, hp=760, dmg=74, spd=88, rng=22,
                       size=25, flying=False, paper=(120, 84, 62), sigil="charge"),
    "lantern":    dict(name="Lantern Jelly", cost=3, hp=300, dmg=0, spd=0, rng=0,
                       size=24, flying=False, paper=(120, 190, 180), sigil="beacon", building=True),
    "coyote":     dict(name="Trick Coyote", cost=3, hp=320, dmg=58, spd=78, rng=20,
                       size=19, flying=False, paper=(150, 120, 90), sigil="mirror"),
    "tortoise":   dict(name="Siege Tortoise", cost=6, hp=2200, dmg=60, spd=22, rng=26,
                       size=34, flying=False, paper=(96, 116, 80), sigil="bulwark"),
    "phoenix":    dict(name="Ember Phoenix", cost=6, hp=640, dmg=96, spd=72, rng=24,
                       size=27, flying=True, paper=(220, 120, 50), sigil="rebirth"),
    "weaver":     dict(name="Silk Weaver", cost=4, hp=520, dmg=30, spd=44, rng=90,
                       size=22, flying=False, paper=(180, 150, 190), sigil="snare"),
    # NEW 8 CARDS (Phase 3 Expansion) — reach 46 total
    "warthog":    dict(name="War Warthog", cost=4, hp=880, dmg=68, spd=80, rng=20,
                       size=26, flying=False, paper=(120, 92, 76), sigil="rampage"),
    "willowisp":  dict(name="Will-o-Wisp", cost=2, hp=120, dmg=44, spd=96, rng=60,
                       size=14, flying=True, paper=(150, 230, 190), sigil="blink"),
    "obelisk":    dict(name="Ink Obelisk", cost=5, hp=1500, dmg=80, spd=0, rng=200,
                       size=30, flying=False, paper=(60, 56, 70), sigil="artillery", building=True),
    "mimic":      dict(name="Folded Mimic", cost=3, hp=460, dmg=52, spd=60, rng=18,
                       size=20, flying=False, paper=(150, 130, 96), sigil="copycat"),
    "beetle":     dict(name="Dung Beetle", cost=3, hp=700, dmg=40, spd=50, rng=18,
                       size=22, flying=False, paper=(70, 66, 58), sigil="roll"),
    "sunbird":    dict(name="Sun Bird", cost=5, hp=560, dmg=70, spd=70, rng=30,
                       size=24, flying=True, paper=(240, 190, 80), sigil="radiance"),
    "jackalope":  dict(name="Jackalope", cost=2, hp=200, dmg=48, spd=110, rng=16,
                       size=15, flying=False, paper=(180, 150, 120), sigil="leap"),
    "leviathan":  dict(name="Ink Leviathan", cost=8, hp=3000, dmg=180, spd=30, rng=40,
                       size=42, flying=False, paper=(30, 40, 66), sigil="tidal", riverhop=True),
}
SIGIL_COLORS = {"pounce": WAX, "armored": GOLD, "feather": (110, 146, 188),
                "chill": (110, 156, 196), "frenzy": WAX, "drinker": (86, 58, 106),
                "slam": GOLD, "taunt": (104, 130, 76), "toxic": (72, 150, 78),
                "sharp": (150, 36, 42), "relentless": GOLD, "towerbane": (150, 36, 42),
                "sap": (150, 36, 42), "manyfold": (110, 110, 120), "doublestrike": (120, 158, 92),
                "rush": (200, 120, 60), "dormant": (140, 120, 90), "beach": (120, 150, 170),
                "grasp": (140, 70, 140), "thorns": (80, 110, 70), "plague": (150, 140, 70),
                # Phase-1 sigil colors
                "swiftdive": (60, 60, 120), "inksplash": (80, 20, 80), "knock": (100, 180, 220),
                "charm": (200, 150, 100), "steal": (100, 20, 100), "freeze": (150, 200, 240),
                "forge": (200, 160, 100), "stealth": (100, 100, 120),
                # Phase-2 sigil colors
                "burrow": (150, 122, 40), "snipe": (200, 190, 170), "charge": (200, 120, 60),
                "beacon": (120, 200, 190), "mirror": (170, 150, 120), "bulwark": (110, 150, 90),
                "rebirth": (240, 130, 60), "snare": (190, 160, 200),
                # Phase-3 sigil colors
                "rampage": (200, 90, 60), "blink": (150, 230, 190), "artillery": (90, 80, 120),
                "copycat": (170, 150, 110), "roll": (120, 110, 90), "radiance": (250, 210, 110),
                "leap": (190, 160, 120), "tidal": (80, 130, 180),
                "tunnel": (140, 110, 80), "siege": (150, 90, 60), "carrier": (220, 190, 90),
                "wildcard": (230, 180, 90), "builder": (196, 170, 120)}

SIGIL_DESC = {"pounce": "First hit deals triple.", "armored": "Ignores first 3 hits.",
              "feather": "Flies over everything.", "chill": "Nearby foes attack slower.",
              "frenzy": "Attacks faster when hurt.", "drinker": "Heals allies on nearby death.",
              "slam": "Knocks back on heavy hits.", "taunt": "Forces foes to strike it.",
              "toxic": "Poisons on hit.", "sharp": "Retaliates in melee.",
              "relentless": "Survives one lethal blow.", "towerbane": "Only targets towers.",
              "sap": "Heals nearby allies.", "manyfold": "Alpha splits on death.",
              "doublestrike": "Strikes twice per swing.", "rush": "Charges faster in the open.",
              "dormant": "Sleeps until struck, then rages.", "beach": "River-drop; dries out over time.",
              "grasp": "Long reach from the water.", "thorns": "Allies retaliate nearby.",
              "plague": "Damages a wide area.",
              # Phase-1 sigil descriptions
              "swiftdive": "Flocking dive-bombs deal bonus damage.", "inksplash": "Splashes ink puddles on impact.",
              "knock": "Knocks back whole lanes.", "charm": "Enemies nearby forget targets.",
              "steal": "Steals enemy mana on hit.", "freeze": "Stuns target in place.",
              "forge": "Nearby armor+damage.", "stealth": "Hides until first attack.",
              # Phase-2 sigil descriptions
              "burrow": "Digs — untargetable until it strikes.", "snipe": "Very long range; slow, heavy shots.",
              "charge": "Speeds up the longer it runs.", "beacon": "Buildings pull foes toward it.",
              "mirror": "Spawns a decoy double on deploy.", "bulwark": "Halves damage from the front.",
              "rebirth": "Revives once at half health.", "snare": "Web slows the units it hits.",
              # Phase-3 sigil descriptions
              "rampage": "Each kill makes it hit harder.", "blink": "Teleports past the front line.",
              "artillery": "Lobs shells across the whole page.", "copycat": "Becomes a copy of its first foe.",
              "roll": "Curls up and bowls through a lane.", "radiance": "Heals and shields nearby allies.",
              "leap": "Bounds over the river in one hop.", "tidal": "Its wake shoves everything aside.",
              "tunnel": "Digs under the river, surfacing behind the line.",
              "siege": "Lobs powder-kegs across the page, then crumbles.",
              "carrier": "Looses bees that harry foes — but they sip your ink.",
              "wildcard": "Calls in a random generated unit; +2 cost each use.",
              "builder": "Wanders to open ground and folds a new tower troop."}

# ── Data-driven card layer ────────────────────────────────────────────────
# Cards, abilities and rare variants live in cards.json so new cards, balance
# patches and rotations ship as data. If the file is missing we fall back to the
# in-code CARDS dict above, so the game always runs.
try:
    import carddata
    if carddata.CARDS:
        CARDS = carddata.CARDS                 # swap in the JSON-loaded dict
        # merge any ability colors/descriptions the JSON knows that we don't
        for aid, ab in carddata.ABILITIES.items():
            SIGIL_COLORS.setdefault(aid, tuple(ab.get("color", (188, 146, 66))))
            SIGIL_DESC.setdefault(aid, ab.get("desc", ""))
    else:
        carddata.install_fallback(CARDS, SIGIL_COLORS, SIGIL_DESC)
    CARDDATA_OK = True
except Exception as _cd_ex:
    CARDDATA_OK = False

# Expansions: set/pack toggles layered over carddata's rotation gate. External
# packs in expansions/*.json are merged here (cards get set=pack id); the
# toggle screen lives under Options → Expansions.
try:
    import expansions as expmod
    if CARDDATA_OK:
        expmod.init(carddata)
    EXPANSIONS_OK = True
except Exception as _exp_ex:
    expmod = None
    EXPANSIONS_OK = False

try:
    import collection as PBCOLLECTION
    COLLECTION_OK = True
except Exception:
    COLLECTION_OK = False

# seasonal / holiday events — a date-driven cosmetic layer. Because the active
# event is a pure function of the calendar date, every device agrees on it with
# no networking, so a LAN full of players themes itself automatically.
try:
    import events as EVENTS_MOD
    ACTIVE_EVENT = EVENTS_MOD.active_event()
    EVENTS_OK = True
except Exception:
    EVENTS_MOD = None
    ACTIVE_EVENT = None
    EVENTS_OK = False
EVENT_PARTICLES = []          # display-only ambient overlay (never touches sim)

# sigil bank — a registry of ability glyphs. Floating badges above units (an
# accessibility aid: read a creature's ability at a glance) can be toggled.
try:
    import sigils as SIGILS_MOD
    SIGILS_OK = True
except Exception:
    SIGILS_MOD = None
    SIGILS_OK = False
SIGIL_BADGES = False          # off by default; toggle in Options (accessibility)

# Foldworks — the RTS mode ("game inside the game"), a self-contained skirmish.
try:
    import rts as rtsmod
    RTS_OK = True
except Exception:
    rtsmod = None
    RTS_OK = False
RTS_GAME = None               # active RTS match while in the "rts" scene
EXPANSIONS_SCENE = None       # lazy expansions.Scene while in the "expansions" scene

def _register_seasonal_variant():
    """If a holiday is active and it offers a card skin, register its variant spec
    with carddata so make_variant() can print any card in that seasonal style.
    Returns the variant id, or None."""
    if not (EVENTS_OK and CARDDATA_OK):
        return None
    spec = EVENTS_MOD.seasonal_variant()
    if not spec:
        return None
    vid = spec["id"]
    try:
        carddata.VARIANTS.setdefault(vid, {
            "name": spec["name"], "suffix": spec["suffix"], "rarity_bump": 0,
            "mods": spec.get("mods", {}), "tint": spec["tint"],
            "add_ability": spec.get("add_ability"),
        })
    except Exception:
        return None
    return vid

SEASONAL_VARIANT_ID = None    # set at startup if an event skin is live

_SHADE_CACHE = {}
def shade(c, f):
    # called tens of thousands of times per frame with a small set of repeating
    # (colour, factor) pairs, so memoise. Manual clamp/round beats a genexpr.
    try:
        key = (c, f)
        v = _SHADE_CACHE.get(key)
        if v is not None:
            return v
    except TypeError:                   # unhashable (list) colour: skip the cache
        return tuple(max(0, min(255, int(x * f))) for x in c)
    if len(c) != 3:                     # rare RGBA/other: general path, no cache
        return tuple(max(0, min(255, int(x * f))) for x in c)
    r = int(c[0] * f); g = int(c[1] * f); b = int(c[2] * f)
    r = 0 if r < 0 else (255 if r > 255 else r)
    g = 0 if g < 0 else (255 if g > 255 else g)
    b = 0 if b < 0 else (255 if b > 255 else b)
    v = (r, g, b)
    if len(_SHADE_CACHE) < 8192:        # bound memory; colours/factors are few
        _SHADE_CACHE[key] = v
    return v

def lerp(a, b, t): return a + (b - a) * t

def _mix(c1, c2, t):
    """Blend two RGB colours; t=0 → c1, t=1 → c2. Used for gentle card tints."""
    return (int(c1[0] + (c2[0] - c1[0]) * t),
            int(c1[1] + (c2[1] - c1[1]) * t),
            int(c1[2] + (c2[2] - c1[2]) * t))

VIEW_FLIP = False   # when True, mirror the board vertically so the local
                    # player's side (their team) always reads on the bottom.

def _vflip_y(y):
    # mirror a world-Y across the river so the far side becomes the near side
    return 2 * RIVER_Y - y

def proj(x, y):
    if VIEW_FLIP:
        y = _vflip_y(y)
    # normalise the world point within the FIXED world board…
    t = max(0.0, min(1.0, (y - BOOK.top) / BOOK.h))
    ty = 0.66 * t + 0.34 * t * t
    # …then place it in the reflowed DISPLAY trapezoid.
    sy = lerp(DISP_TOP_Y, DISP_BOT_Y, ty)
    s = lerp(S_FAR, S_NEAR, t)
    # horizontal: world offset from board centre, scaled to fill the display board
    xr = (x - BOOK.centerx) / max(1, BOOK.w)          # -0.5..0.5 across the board
    sx = DISP_CX + xr * DISP_BOOK.w * s
    return sx, sy, s

def unproj(sx, sy):
    k = (sy - DISP_TOP_Y) / max(1e-6, (DISP_BOT_Y - DISP_TOP_Y))
    k = max(0.0, min(1.0, k))
    t = (-0.66 + math.sqrt(0.66 * 0.66 + 4 * 0.34 * k)) / (2 * 0.34)
    y = BOOK.top + t * BOOK.h
    s = lerp(S_FAR, S_NEAR, t)
    xr = (sx - DISP_CX) / (max(1, DISP_BOOK.w) * s)
    x = xr * BOOK.w + BOOK.centerx
    if VIEW_FLIP:
        y = _vflip_y(y)
    return x, y

def quad(x0, y0, x1, y1):
    return [proj(x0, y0)[:2], proj(x1, y0)[:2], proj(x1, y1)[:2], proj(x0, y1)[:2]]

def light_lambert(mx, my, nx, ny, boost=0.5, floor=0.45):
    """brightness of a face at (mx,my) with outward normal (nx,ny), lit by main light"""
    lx, ly = LIGHT_MAIN[0] - mx, LIGHT_MAIN[1] - my
    d = math.hypot(lx, ly) or 1
    lam = max(0.0, (nx * lx + ny * ly) / d)
    return floor + boost * lam

# ---------------------------------------------------------------- prisms
def draw_prism(surf, pts, h, base, top_detail=True):
    """extrude a screen-space polygon upward by h; lambert-shaded side walls, lit top"""
    n = len(pts)
    cxv = sum(p[0] for p in pts) / n
    cyv = sum(p[1] for p in pts) / n
    edges = sorted([(pts[i], pts[(i + 1) % n]) for i in range(n)],
                   key=lambda e: (e[0][1] + e[1][1]))
    for a, b in edges:
        ex, ey = b[0] - a[0], b[1] - a[1]
        nx, ny = ey, -ex
        mx, my = (a[0] + b[0]) / 2, (a[1] + b[1]) / 2
        if (mx - cxv) * nx + (my - cyv) * ny < 0:
            nx, ny = -nx, -ny
        ln = math.hypot(nx, ny) or 1
        nx, ny = nx / ln, ny / ln
        if ny < -0.35:           # back-facing wall hidden by top
            continue
        lam = light_lambert(mx, my, nx, ny, boost=0.5, floor=0.48)
        # wall as vertical gradient slices: dark at base, lit at rim (fake bounce)
        slices = 4
        for si in range(slices):
            f0, f1 = si / slices, (si + 1) / slices
            sl_pts = [(a[0], a[1] - h * f0), (b[0], b[1] - h * f0),
                      (b[0], b[1] - h * f1), (a[0], a[1] - h * f1)]
            g = lam * (0.72 + 0.55 * f1)
            pygame.draw.polygon(surf, shade(base, g), sl_pts)
        pygame.draw.line(surf, shade(base, lam * 1.35), (a[0], a[1] - h), (b[0], b[1] - h), 1)
        pygame.draw.line(surf, shade(base, lam * 0.55), a, b, 1)          # contact base
        pygame.draw.line(surf, shade(base, lam * 1.1), (a[0], a[1] - h), (a[0], a[1]), 1)  # corner edge
    top = [(p[0], p[1] - h) for p in pts]
    # top face: overall candle-facing tint
    tl = light_lambert(cxv, cyv - h, 0.28, -0.72, boost=0.35, floor=0.86)
    pygame.draw.polygon(surf, shade(base, tl), top)
    if top_detail:
        for i in range(n):
            a, b = top[i], top[(i + 1) % n]
            mx, my = (a[0] + b[0]) / 2, (a[1] + b[1]) / 2
            ex, ey = b[0] - a[0], b[1] - a[1]
            nx, ny = ey, -ex
            if (mx - cxv) * nx + (my - (cyv - h)) * ny < 0: nx, ny = -nx, -ny
            ln = math.hypot(nx, ny) or 1
            lam = light_lambert(mx, my, nx / ln, ny / ln, boost=0.8, floor=0.55)
            pygame.draw.line(surf, shade(base, lam * 1.15), a, b, 2)
    return top

def draw_poly_paper(surf, pts, base, thick=3):
    if thick:
        pygame.draw.polygon(surf, shade(base, 0.42),
                            [(p[0] + 2, p[1] + thick) for p in pts])
    pygame.draw.polygon(surf, base, pts)
    n = len(pts)
    cxv = sum(p[0] for p in pts) / n
    cyv = sum(p[1] for p in pts) / n
    for i in range(n):
        a, b = pts[i], pts[(i + 1) % n]
        mx, my = (a[0] + b[0]) / 2, (a[1] + b[1]) / 2
        ex, ey = b[0] - a[0], b[1] - a[1]
        nx, ny = ey, -ex
        if (mx - cxv) * nx + (my - cyv) * ny < 0: nx, ny = -nx, -ny
        ln = math.hypot(nx, ny) or 1
        lam = light_lambert(mx, my, nx / ln, ny / ln, boost=0.7, floor=0.5)
        pygame.draw.line(surf, shade(base, lam * 1.2), a, b, 2)

# ── wire the data-driven origami part engine to these paper-drawing helpers ──
try:
    import origami
    origami.bind_drawing(draw_prism, draw_poly_paper, shade)
    ORIGAMI_OK = True
except Exception:
    ORIGAMI_OK = False
    origami = None

# ---------------------------------------------------------------- entities (sim unchanged)
class FloatText:
    def __init__(self, x, y, txt, col):
        self.x, self.y, self.txt, self.col, self.life = x, y, txt, col, 0.9
    def update(self):
        self.y -= 28 * DT; self.life -= DT

class Particle:
    def __init__(self, x, y, col, spd=90, life=0.5, r=3):
        a = random.uniform(0, math.tau)
        v = random.uniform(0.3, 1.0) * spd
        self.x, self.y = x, y
        self.vx, self.vy = math.cos(a) * v, math.sin(a) * v - 30
        self.col, self.life, self.max, self.r = col, life, life, r
    def update(self):
        self.x += self.vx * DT; self.y += self.vy * DT; self.vy += 160 * DT
        self.life -= DT

class Tower:
    def __init__(self, x, y, team, king=False, troop=None):
        self.x, self.y, self.team, self.king = x, y, team, king
        self.troop = troop            # None = default/king; else a TOWER_TROOPS key
        if king or troop is None:
            self.hp = self.maxhp = 2600 if king else 1500
            self.rng, self.dmg, self.cd = 223, 60, 0.9   # range trimmed 5% (was 235)
            self.size = 46 if king else 36
            self.weapon = "bolt"; self.special = None
            self.roof = None
        else:
            d = TOWER_TROOPS[troop]
            self.hp = self.maxhp = d["hp"]
            # troop-tower range trimmed 5% too, so none of them reach the far bank
            self.rng, self.dmg, self.cd = d["rng"] * 0.95, d["dmg"], d["cd"]
            self.size = 36
            self.weapon = d["weapon"]; self.special = d["special"]
            self.roof = d["color"]
            # spawner special: periodically hatches a friendly unit
            self.spawn_key = d.get("spawn_key")
            self.spawn_cd = d.get("spawn_cd", 6.0)
            self.spawn_max = d.get("spawn_max", 4)
            self.spawn_t = self.spawn_cd * 0.5     # first hatch comes a bit sooner
        self.t = 0.0
        self.twin_stage = 0           # for the twin-nib stagger
    @property
    def alive(self): return self.hp > 0

class Unit:
    _UID = 0
    def __init__(self, key, x, y, team):
        d = CARDS[key]
        Unit._UID += 1
        self.uid = Unit._UID              # deterministic per-unit id (sim-safe)
        self.key, self.team = key, team
        self.x, self.y = x, y
        self.hp = self.maxhp = d["hp"]
        self.dmg, self.spd, self.rng = d["dmg"], d["spd"], d["rng"]
        self.size, self.flying = d["size"], d["flying"]
        self.paper = d["paper"]; self.sigil = d["sigil"]
        self.building = d.get("building", False)
        self.cool = 0.0; self.hits_ignored = 3 if self.sigil == "armored" else 0
        self.fire_flash = 0.0                 # firing-animation timer
        self.pounce = self.sigil == "pounce"
        self.face = -1 if team == 1 else 1
        self.spawn_t = 0.35
        self.riverhop = d.get("riverhop", False)
        self.dot = 0.0                       # toxic damage-over-time on this unit
        self.hooked = False
        self.count = d.get("count", 1)
        self.asleep = self.sigil == "dormant"      # wakes when first damaged
        self.beach_t = 8.0 if self.sigil == "beach" else 0.0  # dries out
        self.split_done = False
        # rare-variant abilities ride alongside the base sigil (e.g. spirit
        # divergence spawns shadow echoes on death). Read from the card entry.
        self.variant_ability = d.get("variant_ability")
        self.spirit_done = False             # spirit-divergence: echoes spawned yet?
        self.is_echo = False                 # a shadow echo from spirit divergence
        # NEW SIGIL FLAGS (Phase 1)
        self.stunned = 0.0                   # freeze: stun duration
        self.stealthed = self.sigil == "stealth"  # stealth: hidden until first attack
        self.forge_armor = 1.0               # forge aura: damage multiplier
        self.knock_timer = 0.0               # knock: knockback stun timer
        self.charm_affected = False          # charm: enemies forgetting targets
        # NEW SIGIL FLAGS (Phase 2)
        self.burrowed = self.sigil == "burrow"     # burrow: untargetable until it strikes
        self.run_t = 0.0                     # charge: ramps speed the longer it runs
        self.snared = 0.0                    # snare: movement-slow timer
        self.anchored = self.sigil == "anchor"    # anchor: immune to push/pull
        self.warp_done = False               # warp: one-time teleport used?
        self.molten_t = 0.0                  # molten: burning-trail drip timer
        self.quill_done = False              # quill: death volley fired?
        self.reborn = False                  # rebirth: has it used its revive yet
        self.is_decoy = False                # mirror: decoy copies don't re-mirror
        # NEW SIGIL FLAGS (Phase 3)
        self.rage = 0                        # rampage: bonus damage stacks per kill
        self.blinked = False                 # blink: has it teleported yet
        self.copied = (self.sigil != "copycat")  # copycat: becomes first foe it sees
        self.rolling = 0.0                   # roll: bowl-charge timer
        self.leaped = False                  # leap: has it hopped the river
        self.ghost_t = 0.0                   # ghostly: first-wall phase timer
        self.ghost_done = self.sigil != "ghostly"  # ghostly: phase used up?
        # ── special-mechanic units ──
        self.tunneled = (self.sigil == "tunnel")   # mole: still underground, heading across
        # builder (Foldwright): wanders to open ground, then folds a tower troop.
        self.build_state = "wander" if self.sigil == "builder" else None
        self.build_t = 0.0                   # progress folding the structure (0..1)
        self.build_x = self.build_y = 0.0    # chosen build site
        self.build_tower = None              # the Tower it's raising (removed if slain)
        self.wander_t = 0.0                  # time spent wandering this deployment
        self.wander_goal = None              # current drift target
        self.life_t = 22.0 if self.sigil == "siege" else 0.0  # eagle's nest lifespan (s)
        # eagle's-nest airspace mechanic: the nest tracks its currently-airborne
        # eagle (a spawned flying unit) so it won't launch another until it's home.
        self.eagle_out = None
        # fields carried by an eagle itself (a spawned flyer): its home nest, the
        # bomb target, and its flight phase (outbound → drop → return).
        self.eagle_home = None
        self.eagle_target = None
        self.eagle_phase = None          # "out" | "return" | None (not an eagle)
        self.eagle_dmg = 0
        self.is_eagle = False            # True on a launched eagle unit
        self.landed = False              # eagle landed cleanly (not shot down)
        self.bee_cd = 0.0                    # beekeeper: interceptor launch cooldown
        self.bees = []                       # beekeeper: live bee interceptors
        self.from_carrier = None             # a bee's parent beekeeper
        self.bee_home = None                 # a bee's orbit anchor
        self.bee_phase = 0.0                 # a bee's deterministic orbit phase
    @property
    def alive(self): return self.hp > 0

class CardRotation:
    """Clash-Royale-style card rotation (a queue, NOT a shuffled deck). The 8
    deck cards form a fixed cycle: the front 4 are the hand, the 5th is the
    'next' preview. Playing a card sends it to the very back of the queue, so a
    card can never appear in the hand twice and every card comes up once per full
    rotation. Deterministic — order only changes when a card is played."""
    __slots__ = ("queue",)
    def __init__(self, deck):
        # deterministic per-unit id counter, reset each game so host & client
        # (which build identical unit sequences) assign identical uids.
        Unit._UID = 0
        # de-duplicate while preserving order so a card can't sit in the queue
        # twice, then pad to at least 5 (4 hand + 1 next) for tiny decks.
        seen = []
        for k in deck:
            if k not in seen:
                seen.append(k)
        fillers = ["sabertooth", "crane", "frog", "baobab", "wolverine"]
        fi = 0
        while len(seen) < 5:
            f = fillers[fi % len(fillers)]; fi += 1
            if f not in seen:
                seen.append(f)
            elif fi > 40:
                break
        self.queue = seen

    @property
    def hand(self):
        return self.queue[:4]

    @property
    def next_card(self):
        return self.queue[4] if len(self.queue) > 4 else self.queue[0]

    def play(self, idx):
        """Play the card in hand slot idx. The played card is refilled *in place*
        by the next card from the queue, so the other three hand slots DON'T move —
        this keeps muscle-memory intact and lets you drop two cards in quick
        succession without the second slot sliding out from under your finger.
        Returns the played card key (or None if idx is out of range)."""
        if idx < 0 or idx >= min(4, len(self.queue)):
            return None
        card = self.queue[idx]
        if len(self.queue) > 4:
            # pull the next-in-line (slot 4) into the played slot; send the played
            # card to the back. Hand slots 0..3 other than idx are untouched.
            nxt = self.queue.pop(4)
            self.queue[idx] = nxt
            self.queue.append(card)
        else:
            # tiny deck (≤4 unique cards): rotate as before so the hand refills
            self.queue.pop(idx); self.queue.append(card)
        return card

    def shadow_shuffle(self):
        """The Trickster jumbles the (hidden) hand in the dark, so you can't rely
        on memory to play a shadowed card. Only reorders the 4 hand slots among
        themselves — the deck queue behind stays intact."""
        if len(self.queue) >= 4:
            head = self.queue[:4]
            head.reverse()                 # deterministic reorder (no RNG desync)
            self.queue[:4] = head

class Game:
    # backward-compatible views into the rotation queues (read by the renderer)
    @property
    def hand(self):
        return self.rot.hand
    @property
    def next_card(self):
        return self.rot.next_card
    @property
    def hand2(self):
        return self.rot2.hand
    @property
    def next_card2(self):
        return self.rot2.next_card

    def __init__(self, boss=None, deck=None, netmode=None, seed=None, local_1v1=False,
                 seats=None, match_mode="1v1"):
        # netmode: None (single-player) | "host" | "client"
        # local_1v1: two players on one device (hotseat/controllers); no AI.
        # seats: list of seat ids this device controls (2v2). seat 0,1→team0;
        #        2,3→team1. match_mode: "1v1" | "2v2".
        # In net games team 0 is the host's side, team 1 is the client's side,
        # both sims run deterministically off the same seed and no local AI runs.
        self.netmode = netmode
        self.local_1v1 = local_1v1
        self.match_mode = match_mode
        # which seats this device owns, and which team those map to
        self.my_seats = seats if seats else ([0] if netmode else [])
        if netmode == "client" and not seats:
            self.my_seats = [2]        # default single client seat = team 1
        self.my_team = seat_team_local(self.my_seats[0]) if self.my_seats else 0
        # the LAN client controls team 1 (top); flip the view so their own side
        # reads on the bottom. Host, spectator, single-player and hotseat: no flip.
        # flip the view so this device's team always reads on the bottom
        self.view_flip = (self.my_team == 1)
        self.ai_ink2 = 5.0                   # team-1 human's ink pool in local 1v1
        self.selected2 = None                # team-1 human's selected card
        self.net = None                     # attached NetPeer, set by caller
        # reliable-input layer: sequences/acks/resends deploys so a dropped UDP
        # packet self-heals instead of stalling lockstep, and batches same-frame
        # deploys into one packet. Created lazily when a peer is attached.
        self._rel_out = None
        self._rel_in = None
        self.tick = 0                        # integer sim tick for input scheduling
        self.pending_inputs = {}             # tick -> list of (key,x,y,team)
        self.match_seed = seed if seed is not None else random.randint(0, 1 << 30)
        self.wild_plays = {0: 0, 1: 0}       # wildcard: escalating cost per team
        # ── replay recorder: capture this match's inputs for later playback ──
        self._recorder = None
        self._replay_saved = False
        if not hasattr(self, "is_replay"):
            self.is_replay = False
        if globals().get("REPLAY_OK") and not getattr(self, "is_replay", False):
            try:
                self._recorder = replaymod.Recorder({
                    "seed": self.match_seed, "mode": MODE, "arena": CUR_ARENA,
                    "deck": list(deck)[:8], "boss": boss,
                    "netmode": netmode, "local_1v1": local_1v1,
                })
            except Exception:
                self._recorder = None
        if seed is not None:
            random.seed(seed)
        self.units, self.parts, self.texts, self.projs = [], [], [], []
        self.dying = []            # DISPLAY-ONLY crumpling corpses (sim never reads)
        self.waves = []            # Tidal Wave: traveling line-attacks (sim-real)
        self.fallen = {0: 0, 1: 0}           # kindle: friendly units lost, per team
        self.mode = MODE                     # snapshot the active mode for this match
        cfg = MODES[self.mode]
        self.towers = []
        # team 0 = the local player's flank troop; team 1 = opponent's.
        # In hotseat 1v1 both are the chosen troop; vs AI the AI keeps the spire.
        # In LAN games both sides use the default so the two sims stay identical
        # (tower choice is a single-player / hotseat feature).
        if self.netmode is not None:
            p_troop = e_troop = "spire"
        else:
            p_troop = TOWER_TROOP
            e_troop = TOWER_TROOP if self.local_1v1 else "spire"
        for team, ky in ((0, BOOK.bottom - 70), (1, BOOK.top + 70)):
            kt = Tower(BOOK.centerx, ky, team, king=True)
            kt.hp = kt.maxhp = int(kt.maxhp * cfg["tower_hp"])
            self.towers.append(kt)
            # flank troop-towers sit back from the bridges (toward their own king)
            # so the bridge crossing is a bit of open ground, not a tower gauntlet.
            # In "king_only" modes there are NO flank towers — just the king, so a
            # crossing threatens the crown directly and games swing faster.
            if cfg.get("king_only"):
                continue
            py = BOOK.bottom - 190 if team == 0 else BOOK.top + 190
            troop = p_troop if team == 0 else e_troop
            for bx in BRIDGE_XS:
                pt = Tower(bx, py, team, troop=troop)
                pt.hp = pt.maxhp = int(pt.maxhp * cfg["tower_hp"])
                self.towers.append(pt)
        self.ink = float(cfg["start_ink"]); self.ai_ink = float(cfg["start_ink"])
        # Build a valid 8-card deck. Fall back to defaults for any empty slots so
        # a partially-filled deck-builder choice still produces a working hand.
        base = list(deck) if deck else []
        base = [k for k in base if k in CARDS]         # drop anything unknown
        defaults = ["sabertooth", "polarbear", "crane", "baobab",
                    "wolverine", "pangolin", "frog", "mammoth"]
        for dk in defaults:
            if len(base) >= 8: break
            if dk not in base: base.append(dk)
        self.deck = base[:8]
        # rotation-based hands (no duplicates; a queue, not a shuffled deck)
        self.rot = CardRotation(self.deck)
        self.rot2 = CardRotation(self.deck)          # player-2 hotseat rotation
        self.selected2 = None
        # ── per-seat controllers (2v2 "two players on one device") ──
        # Each owned seat gets its own rotation, ink pool and selection.
        self.dual_local = bool(self.netmode and len(self.my_seats) >= 2)
        self.seat_ctrl = {}
        if self.dual_local:
            for si, seat in enumerate(self.my_seats[:2]):
                # rotate the deck start so the two teammates open on different hands
                off = si * 2
                rotdeck = self.deck[off:] + self.deck[:off]
                self.seat_ctrl[seat] = dict(
                    rot=CardRotation(rotdeck),
                    ink=float(cfg["start_ink"]), selected=None, seat=seat)
        self.selected = None
        self.time = 0.0
        self.ai_timer = 3.0
        # the AI opponent draws from its own generator so it never perturbs the
        # sim's global random stream (keeps recorded matches & replays identical)
        self.ai_rng = random.Random((self.match_seed ^ 0x5bd1e995) & 0xffffffff)
        self.hand_reach = None
        self.hand_t = 0.0
        self.result = None          # None | "win" | "lose"
        self.forfeited = False      # set by the forfeit button
        self.forfeit_notice = None  # team that forfeit (for the notice banner)
        self.snuff_candle = None    # which side's candle to snuff (0/1) or None
        # battle lifecycle FSM (intro → playing → frenzy → ended). Shared by all
        # modes so low-health chaos + forfeit + result behave predictably.
        self.fsm = BattleFSM(intro_ticks=(0 if netmode else battlefsm.INTRO_TICKS)) if FSM_OK else None
        self.frenzy = False         # arena-chaos active (low-health mode)
        self.frenzy_fx = []         # transient arena-chaos visual bits
        self.card_bounce = {}       # (which, idx, seat) -> bounce timer (no-ink)
        self.shockwaves = []        # screen-distortion ripples {x,y,t,dur,mag}
        self.boss = boss            # None | "angler" | "scribe" | "clockking" | "ai:<persona>"
        self.hook_timer = 6.0
        self.hook_anim = None       # (unit, t, phase) phase: "cast"/"reel"
        self.hook_lure = (BOOK.centerx + 90, RIVER_Y)
        # boss ability + phase state (shared across boss types)
        self.boss_phase = 1         # escalates as the boss king loses HP
        # ── The Trickster (Candlethief): a darkness/relight timing duel ──
        self.trick_dark = False          # are the lights out right now?
        self.trick_timer = 8.0           # countdown to the next lights-out
        self.trick_dark_t = 0.0          # how long we've been in the dark
        self.trick_present = False       # is he lurking (danger to relight)?
        self.trick_present_t = 0.0       # timer for the present/absent oscillation
        self.trick_laughed = 0.0         # laugh flash timer (cosmetic)
        self.trick_shuffle_t = 0.0       # periodic hand-shadow shuffle in the dark
        self.trick_hand_shadow = False   # are the hand cards hidden/shadowed?
        self.boss_timer = 5.0       # generic ability cooldown
        self.boss_fx = []           # transient boss visual effects {kind,x,y,t}
        self.boss_announced = False
        # LLM-driven Arch-Scribe: an ai_boss.BossMind that reads the board and
        # answers the player. Lives here; the main loop starts/drains it.
        self.ai_boss_id = None      # persona id when boss == "ai:<persona>"
        self.boss_variant = None    # rare BossVariant twist for this fight
        if isinstance(boss, str) and boss.startswith("ai:"):
            self.ai_boss_id = boss.split(":", 1)[1]
            # rarely, this boss shows up as a variant — a titled twist that mostly
            # changes its placement + entourage. Seeded so it's deterministic.
            if AIBOSS_OK:
                vr = random.Random((self.match_seed ^ 0x805a17) & 0xffffffff)
                try:
                    self.boss_variant = ai_boss.roll_boss_variant(vr)
                except Exception:
                    self.boss_variant = None
            self._entourage_spawned = False
        self.boss_notes = []        # transient in-voice announcements [[text, t]]
        self.last_player_play = None  # last card the human played (boss reads it)
        # weather / hazard / POI runtime state, keyed off active arena globals
        self.flash = 0.0            # lightning brightness pulse
        self.flash_timer = random.uniform(2, 5)
        self.lava = 0.0            # how far lava has crept in from the far edge (0..1)
        self.fissures = []         # list of dicts {x,y,open,phase,timer}
        if HAZARD == "fissures":
            for _ in range(3):
                self.fissures.append(self._new_fissure())
        self.tubs = []             # ink-tub POIs {x,y,owner,progress}
        self.ink_puddles = []      # inksplash mechanic: {x, y, timer, team}
        self.quicksand = []        # dunes: soft pits that slow + sink units
        self.gears = []            # clockwork: rotating gears that shove units
        if HAZARD == "quicksand":
            for _ in range(3):
                self.quicksand.append(dict(
                    x=random.uniform(BOOK.left + 90, BOOK.right - 90),
                    y=random.choice([RIVER_Y - 170, RIVER_Y + 170, RIVER_Y - 90, RIVER_Y + 90]),
                    r=random.uniform(46, 70)))
        if HAZARD == "gears":
            for i in range(2):
                self.gears.append(dict(
                    x=BOOK.centerx + (-1 if i == 0 else 1) * 150,
                    y=RIVER_Y + (-1 if i == 0 else 1) * 120,
                    r=54, ang=0.0, spin=(1 if i == 0 else -1)))
        self.tide = 0.0            # sandbar: rising/falling tide phase (0..1)
        self.tide_dir = 1
        self.falls = []            # outcrop: waterfall columns pushing units down-page
        if HAZARD == "waterfalls":
            for _ in range(3):
                self.falls.append(dict(
                    x=random.uniform(BOOK.left + 120, BOOK.right - 120),
                    w=random.uniform(30, 46)))
        if POI == "inktubs":
            self.tubs = [dict(x=BOOK.centerx, y=RIVER_Y, owner=None, prog=0.0)]

        # ── discovery: card caches sealed on the enemy side. Break one and it
        #    spills a card into your collection. Only in single-player skirmish.
        self.caches = []           # {x,y,hp,maxhp,key,variant,cracked}
        self.found_cards = []      # queue of grant dicts to surface as toasts
        self.table_finds = []      # cards drifting at the desk edges to tap
        self.table_timer = random.uniform(8, 16)
        if self.netmode is None and not self.local_1v1 and COLLECTION_OK:
            self._seed_caches()

    def _seed_caches(self):
        """Place 1–2 wax-sealed card caches on the enemy half of the page."""
        pool = [k for k in ALL_CARDS if not unlocked(k)] or ALL_CARDS
        n = random.choice((1, 1, 2))
        for _ in range(n):
            key = random.choice(pool)
            var = carddata.roll_variant() if CARDDATA_OK else "standard"
            self.caches.append(dict(
                x=random.uniform(BOOK.left + 120, BOOK.right - 120),
                y=random.uniform(BOOK.top + 120, RIVER_Y - 90),
                hp=260.0, maxhp=260.0, key=key, variant=var, cracked=False))

    def _new_fissure(self):
        return dict(x=random.uniform(BOOK.left + 80, BOOK.right - 80),
                    y=random.choice([RIVER_Y - 150, RIVER_Y + 150, RIVER_Y - 260, RIVER_Y + 260]),
                    open=0.0, state="opening", timer=random.uniform(3, 6))

    def cost_of(self, key, team=0):
        """The ink cost to play `key` right now. Most cards are a flat cost, but
        the wildcard climbs +2 each time that team plays it this match, so calling
        in endless generated units gets pricey fast."""
        base = CARDS[key]["cost"]
        if CARDS[key].get("sigil") == "wildcard":
            return base + 2 * self.wild_plays.get(team, 0)
        return base

    def _wildcard_key(self, team):
        """Generate (once) a procedural card for a wildcard summon and register it,
        returning its key. Deterministic: seeded from the match seed + this team's
        wildcard-play index, so host and client generate the SAME unit."""
        if not globals().get("AIDIR_OK"):
            return "inkling"      # graceful fallback if the generator is absent
        idx = self.wild_plays.get(team, 0)
        seed = (self.match_seed & 0xffffffff) ^ (team * 7919) ^ (idx * 104729)
        wkey = "wild%x%d%d" % (seed & 0xffffff, team, idx)
        if wkey in CARDS:                 # already generated this exact one
            return wkey
        rng = random.Random(seed)
        try:
            card = ai_director.procedural_card(rng, taken_ids=())
            card["set"] = "wildcard"
            card["name"] = "Wild " + card.get("name", "Thing")
            card["id"] = wkey             # force our deterministic id
            key = register_card(card, force_key=wkey)
            return key or "inkling"
        except Exception:
            return "inkling"

    def deploy(self, key, x, y, team):
        # replay: log every deploy input with the tick it lands on, so the match
        # can be reproduced deterministically from seed + inputs alone.
        rec = getattr(self, "_recorder", None)
        if rec is not None:
            try:
                rec.log(self.tick, key, x, y, team)
                # snapshot any non-base card (wildcard/generated/boss) so replay
                # can re-register it before playback.
                base_keys = globals().get("BASE_CARD_KEYS")
                if key in CARDS and (base_keys is None or key not in base_keys):
                    rec.note_card(key, CARDS[key])
            except Exception:
                pass
        # wildcard: instead of a fixed unit, call in something the generators make
        if CARDS.get(key, {}).get("sigil") == "wildcard":
            gen = self._wildcard_key(team)
            self.wild_plays[team] = self.wild_plays.get(team, 0) + 1
            self.texts.append(FloatText(x, y - 30, "wild!", (230, 180, 90)))
            for _ in range(14):
                self.parts.append(Particle(x, y, (230, 180, 90), 90, 0.5, 3))
            key = gen                     # deploy the generated card instead
        # ── Tidal Wave is a SPELL, not a unit: it looses three cresting waves that
        #    roll straight up the lane, each a wide band that damages and shoves
        #    everything it washes over (buildings take reduced damage and barely
        #    budge). Keyed off the CARD, not the shared "tidal" sigil — the Ink
        #    Leviathan and River Serpent also carry that sigil (a swim-shove) and
        #    must stay ordinary units. Fully deterministic — fixed sim clock. ──
        if key == "tidalwave":
            self._cast_tidal(x, y, team, CARDS[key])
            return []
        d = CARDS[key]
        n = d.get("count", 1)
        made = []
        for i in range(n):
            ox = (i - (n - 1) / 2) * 26
            u = Unit(key, x + ox, y, team)
            self.units.append(u); made.append(u)
        # mirror: also drop a half-health decoy double a little to the side
        if d.get("sigil") == "mirror" and made and not made[0].is_decoy:
            dc = Unit(key, x + 40, y, team)
            dc.hp = dc.maxhp = made[0].maxhp * 0.5
            dc.is_decoy = True
            self.units.append(dc)
            self.texts.append(FloatText(x + 40, y - 20, "double!", (170, 150, 120)))
        for _ in range(10):
            self.parts.append(Particle(x, y, PAGE, 70, 0.4, 2))
        # a satisfying placement flourish — a quick paper-unfold ring of motes and
        # a soft ripple. Good tactile feedback, especially for touch where there's
        # no cursor to confirm the tap landed.
        _pcol = d.get("paper", PAGE)
        if isinstance(_pcol, list): _pcol = tuple(_pcol)
        for a in range(10):
            ang = a / 10 * 6.2832
            self.parts.append(Particle(x + math.cos(ang) * 8, y + math.sin(ang) * 6,
                                       shade(_pcol, 1.2), 70, 0.45, 2))
        self.add_shockwave(x, y, mag=8, dur=0.35)
        # ── on-arrival effects ──
        sig = d.get("sigil")
        if made:
            leader = made[0]
            # gale (Condor): a downdraft shoves nearby foes back on landing
            if sig == "gale":
                for f in self.enemies_of(team):
                    if isinstance(f, Unit) and not getattr(f, "anchored", False):
                        dd = math.hypot(f.x - x, f.y - y)
                        if 0 < dd < 150:
                            f.x += (f.x - x) / dd * 80
                            f.y += (f.y - y) / dd * 40
                            f.knock_timer = 0.4
                self.texts.append(FloatText(x, y - 30, "GALE!", (170, 200, 220)))
            # dazzle (Lanternfish): a burst of light blinds nearby foes (brief stun)
            if sig == "dazzle":
                for f in self.enemies_of(team):
                    if isinstance(f, Unit) and math.hypot(f.x - x, f.y - y) < 130:
                        f.stunned = max(f.stunned, 1.0)
                self.texts.append(FloatText(x, y - 30, "DAZZLE!", (240, 230, 150)))
                for _ in range(14):
                    self.parts.append(Particle(x, y, (245, 235, 170), 90, 0.5, 3))
            # warp (Moth Mage): blinks straight to the nearest foe on arrival, once
            if sig == "warp" and not leader.warp_done:
                foes = [f for f in self.enemies_of(team) if isinstance(f, Unit) and f.alive]
                if foes:
                    tgt = min(foes, key=lambda f: math.hypot(f.x - x, f.y - y))
                    # deterministic landing offset (seeded from deploy pos), so net
                    # peers agree without touching the shared RNG stream
                    wrng = random.Random((self.match_seed ^ (int(x) * 92821) ^ int(y)) & 0xffffffff)
                    for j, m in enumerate(made):
                        m.warp_done = True
                        m.x = tgt.x + wrng.uniform(-20, 20) + j * 8
                        m.y = tgt.y + (30 if team == 1 else -30)
                    self.texts.append(FloatText(tgt.x, tgt.y - 30, "warp!", (180, 150, 220)))
                    for _ in range(12):
                        self.parts.append(Particle(tgt.x, tgt.y, (180, 150, 220), 90, 0.5, 3))
        sfx("deploy", 0.7)
        return made[0] if made else None

    def try_play(self, idx, pos, team=0):
        """Returns 'played', 'noink' (not enough ink — caller should bounce and
        keep the card selected), or None (invalid)."""
        if self.result: return None
        hand = self.rot.hand
        if idx < 0 or idx >= len(hand): return None
        key = hand[idx]
        _cost = self.cost_of(key, 0)
        if self.ink < _cost:
            self._bounce(idx, "bottom")
            return "noink"
        x, y = pos
        if y < RIVER_Y + RIVER_H: y = RIVER_Y + RIVER_H + 10
        self.ink -= _cost
        self.deploy(key, x, y, 0)
        self.rot.play(idx)          # card rotates to the back of the queue
        self.last_player_play = key  # let an AI boss react to the player's style
        return "played"

    def try_play2(self, idx, pos):
        """Player 2 (team 1, top half) deploys in local hotseat 1v1."""
        if self.result: return None
        hand = self.rot2.hand
        if idx < 0 or idx >= len(hand): return None
        key = hand[idx]
        _cost = self.cost_of(key, 1)
        if self.ai_ink2 < _cost:
            self._bounce(idx, "top")
            return "noink"
        x, y = pos
        if y > RIVER_Y - RIVER_H: y = RIVER_Y - RIVER_H - 10
        self.ai_ink2 -= _cost
        self.deploy(key, x, y, 1)
        self.rot2.play(idx)
        return "played"

    def _bounce(self, idx, which, seat=None):
        """Register a brief 'not enough ink' bounce animation for a hand card."""
        if not hasattr(self, "card_bounce"):
            self.card_bounce = {}
        self.card_bounce[(which, idx, seat)] = 0.35
        sfx("error")

    def enemies_of(self, team):
        return ([u for u in self.units if u.team != team and u.alive] +
                [t for t in self.towers if t.team != team and t.alive])

    def _launch_eagle(self, nest, tgt):
        """Send an eagle aloft from the nest toward a target. It's a real flying
        unit with HP — it flaps out, drops a bomb, and returns; if it's hurt too
        much en route it breaks off early and retreats. Returns the eagle Unit."""
        eagle = Unit("eaglenest", nest.x, nest.y - 20, nest.team)
        # override into a light, fast flyer (not another nest)
        eagle.hp = eagle.maxhp = 260
        eagle.dmg = 0
        eagle.spd = 150
        eagle.rng = 0
        eagle.size = 18
        eagle.flying = True
        eagle.sigil = "none"
        eagle.life_t = 0.0
        eagle.is_eagle = True
        eagle.eagle_home = nest
        eagle.eagle_target = tgt
        eagle.eagle_phase = "out"
        eagle.eagle_dmg = nest.dmg
        eagle.paper = (120, 96, 70)
        self.units.append(eagle)
        self.texts.append(FloatText(nest.x, nest.y - 30, "eagle away!", (210, 190, 150)))
        return eagle

    def _eagle_tick(self, u):
        """Fly the eagle: out to its target (drop a bomb on arrival), then home.
        Retreats early if bloodied. Returns True if handled (skip normal logic)."""
        home = getattr(u, "eagle_home", None)
        # if bloodied below 40%, break off and run home — airspace denied
        if u.hp < u.maxhp * 0.4 and u.eagle_phase == "out":
            u.eagle_phase = "return"
            self.texts.append(FloatText(u.x, u.y - 26, "driven off!", (220, 120, 90)))
        if u.eagle_phase == "out":
            tgt = u.eagle_target
            if tgt is None or not getattr(tgt, "alive", False):
                # target gone — pick the nearest foe or just turn back
                foes = [f for f in self.enemies_of(u.team) if getattr(f, "alive", False)]
                tgt = min(foes, key=lambda f: math.hypot(f.x - u.x, f.y - u.y)) if foes else None
                u.eagle_target = tgt
            if tgt is None:
                u.eagle_phase = "return"
                return True
            dx, dy = tgt.x - u.x, tgt.y - u.y
            d = math.hypot(dx, dy) or 1
            if d < 26:
                # DROP THE BOMB: splash damage at the target, then head home
                self._eagle_bomb(u, tgt)
                u.eagle_phase = "return"
            else:
                u.x += dx / d * u.spd * DT
                u.y += dy / d * u.spd * DT
                u.face = 1 if dx >= 0 else -1
        elif u.eagle_phase == "return":
            if home is None or not home.alive:
                # nowhere to land — the eagle simply flies off the page edge
                hx = u.x
                hy = BOOK.top - 60 if u.team == 1 else BOOK.bottom + 60
            else:
                hx, hy = home.x, home.y - 20
            dx, dy = hx - u.x, hy - u.y
            d = math.hypot(dx, dy) or 1
            if d < 22:
                # landed safely — free the nest to launch again, eagle rests
                if home is not None and home.alive:
                    home.eagle_out = None
                u.landed = True               # clean landing, not a shootdown
                u.hp = 0                      # despawn (rests inside the nest)
            else:
                u.x += dx / d * u.spd * 1.15 * DT
                u.y += dy / d * u.spd * 1.15 * DT
                u.face = 1 if dx >= 0 else -1
        # occasional feather/flap motes
        if random.random() < 0.15:
            self.parts.append(Particle(u.x, u.y + 4, (150, 130, 100), 30, 0.4, 2))
        return True

    def _eagle_bomb(self, u, tgt):
        """The eagle's dropped bomb: an arcing keg that bursts for splash where the
        target stands."""
        dmg = u.eagle_dmg
        radius = 84
        for f in self.units:
            if f.alive and f.team == tgt.team and math.hypot(f.x - tgt.x, f.y - tgt.y) < radius:
                f.hp -= dmg * (1.0 if f is tgt else 0.6)
        for tw in self.towers:
            if tw.alive and tw.team == tgt.team and math.hypot(tw.x - tgt.x, tw.y - tgt.y) < radius:
                tw.hp -= dmg * 0.5
        self.add_shockwave(tgt.x, tgt.y, mag=18, dur=0.6)
        for _ in range(16):
            self.parts.append(Particle(tgt.x, tgt.y, (230, 170, 90), 120, 0.55, 3))
        self.texts.append(FloatText(tgt.x, tgt.y - 24, str(int(dmg)), WAX))
        sfx("hit", 0.7)

    def _builder_wander_goal(self, u):
        """A drift target on the builder's own half — it ambles toward open
        ground. Seeded deterministically from the match seed + the builder's
        DEPLOY position + how many goals it has picked (all identical on both
        peers), so it never touches the shared RNG stream (which would desync)."""
        u._wander_n = getattr(u, "_wander_n", 0) + 1
        if not hasattr(u, "_spawn_key"):
            u._spawn_key = (int(u.x) * 73856093) ^ (int(u.y) * 19349663)
        rng = random.Random((self.match_seed ^ u._spawn_key ^ (u._wander_n * 40503)) & 0xffffffff)
        y_lo = (RIVER_Y + RIVER_H + 40) if u.team == 0 else (BOOK.top + 60)
        y_hi = (BOOK.bottom - 60) if u.team == 0 else (RIVER_Y - RIVER_H - 40)
        gx = rng.uniform(BOOK.left + 60, BOOK.right - 60)
        gy = rng.uniform(min(y_lo, y_hi), max(y_lo, y_hi))
        return (gx, gy)

    def _spot_is_clear(self, x, y, team):
        """Is there open ground here to build? Needs room from towers, the river,
        and the board edges so structures don't overlap."""
        if x < BOOK.left + 70 or x > BOOK.right - 70:
            return False
        # keep off the river and the enemy half
        if team == 0 and y < RIVER_Y + RIVER_H + 50: return False
        if team == 1 and y > RIVER_Y - RIVER_H - 50: return False
        for tw in self.towers:
            if tw.alive and math.hypot(tw.x - x, tw.y - y) < 96:
                return False
        return True

    def _builder_troop(self, u):
        """Which tower troop the Foldwright folds — the player's chosen flank
        troop on team 0, a plain spire for the enemy, so it's deterministic."""
        if u.team == 0 and not self.netmode:
            return TOWER_TROOP if TOWER_TROOP in TOWER_TROOPS else "spire"
        return "spire"

    def _on_death(self, tgt):
        # rebirth: the phoenix rises once at half health instead of dying
        if isinstance(tgt, Unit) and tgt.sigil == "rebirth" and not tgt.reborn:
            tgt.reborn = True
            tgt.hp = tgt.maxhp * 0.5
            tgt.stunned = 0.0
            for _ in range(20):
                self.parts.append(Particle(tgt.x, tgt.y, (240, 150, 60), 150, 0.7, 3))
            self.texts.append(FloatText(tgt.x, tgt.y - 24, "reborn!", (240, 130, 60)))
            return
        # ── paper-crumple death: the folded creature bursts into scraps of its
        #    own colour and a soft collapse ring, rather than just vanishing. All
        #    deterministic in sim order, so net peers stay in lockstep. ──
        pcol = getattr(tgt, "paper", PAGE)
        if isinstance(pcol, list): pcol = tuple(pcol)
        sz = getattr(tgt, "size", 20)
        for _ in range(14):
            self.parts.append(Particle(tgt.x, tgt.y, shade(pcol, 0.9), 120, 0.6, 3))
        # bigger tumbling scraps (torn folds of the model)
        for _ in range(max(4, int(sz / 6))):
            self.parts.append(Particle(tgt.x, tgt.y, shade(pcol, 1.15),
                                       90, 0.75, max(3, int(sz * 0.18))))
        # a dark collapse puff at the feet
        for _ in range(5):
            self.parts.append(Particle(tgt.x, tgt.y + sz * 0.3, shade(pcol, 0.5), 45, 0.5, 4))
        self.add_shockwave(tgt.x, tgt.y, mag=6, dur=0.3)
        # capture a DISPLAY-ONLY dying copy so the model crumples & topples over the
        # next ~0.5s instead of vanishing. This list is never read by the sim, so
        # it can't affect determinism — the unit is already gone from self.units.
        if isinstance(tgt, Unit) and not getattr(tgt, "is_echo", False) and not getattr(tgt, "is_eagle", False):
            self.dying.append({
                "key": tgt.key, "x": tgt.x, "y": tgt.y, "team": tgt.team,
                "fam": _card_family(tgt.key), "size": sz, "paper": pcol,
                "face": getattr(tgt, "face", 1), "fly": bool(getattr(tgt, "flying", False)),
                "t": 0.0, "dur": 0.55,
                # a deterministic-looking topple direction from the last hit isn't
                # needed (display only), so a cheap per-unit value is fine
                "tip": 1 if (int(tgt.x) % 2 == 0) else -1,
            })
        # tally fallen (kindle scales off this) — count real units, not echoes/eagles
        if isinstance(tgt, Unit) and not getattr(tgt, "is_echo", False) and not getattr(tgt, "is_eagle", False):
            self.fallen[tgt.team] = self.fallen.get(tgt.team, 0) + 1
        # ── THE RISEN: in zombie mode, a fallen real unit reincarnates on the
        #    hostile horde (team 2), which enemies_of() makes an enemy of BOTH
        #    players. Deterministic: no local RNG, no re-raising zombies (guarded),
        #    so net peers stay in perfect lockstep. ──
        if (MODES.get(self.mode, {}).get("risen")
                and isinstance(tgt, Unit)
                and not getattr(tgt, "is_echo", False)
                and not getattr(tgt, "is_eagle", False)
                and not getattr(tgt, "is_zombie", False)
                and tgt.team in (0, 1)
                and not getattr(tgt, "building", False)):
            self._raise_zombie(tgt)
        # quill (Stag Beetle): looses a radial volley of quills on death
        if isinstance(tgt, Unit) and tgt.sigil == "quill" and not getattr(tgt, "quill_done", False):
            tgt.quill_done = True
            enemies = [f for f in self.enemies_of(tgt.team) if isinstance(f, Unit) and f.alive]
            for f in enemies:
                if math.hypot(f.x - tgt.x, f.y - tgt.y) < 150:
                    f.hp -= tgt.dmg * 0.5
                    self.texts.append(FloatText(f.x, f.y - 20, "quill!", (200, 180, 120)))
            for a in range(12):
                ang = a / 12 * 6.2832
                self.parts.append(Particle(tgt.x + math.cos(ang) * 10, tgt.y + math.sin(ang) * 8,
                                           (210, 190, 140), 90, 0.5, 2))
        # harvest (Mushroom): its fall spreads spores that grant its owner bonus ink
        if isinstance(tgt, Unit) and tgt.sigil == "harvest":
            if tgt.team == 0:
                self.ink = min(ink_cap(self.mode), self.ink + 2)
            else:
                self.ai_ink = min(ink_cap(self.mode), self.ai_ink + 2)
            self.texts.append(FloatText(tgt.x, tgt.y - 24, "+2 ink", (150, 200, 120)))
        # a shot-down eagle frees its nest to launch another (airspace contested)
        if isinstance(tgt, Unit) and getattr(tgt, "is_eagle", False):
            home = getattr(tgt, "eagle_home", None)
            if home is not None and getattr(home, "alive", False):
                home.eagle_out = None
            if not getattr(tgt, "landed", False):     # only if actually shot down
                self.texts.append(FloatText(tgt.x, tgt.y - 20, "shot down!", (220, 120, 90)))
                for _ in range(10):
                    self.parts.append(Particle(tgt.x, tgt.y, (150, 120, 90), 90, 0.5, 3))
        # the Foldwright (builder) slain: any structure it raised — or was mid-way
        # through folding — tears apart. The next Foldwright you deploy starts its
        # wander/search from scratch (each deploy is a fresh unit, so that's free).
        if isinstance(tgt, Unit) and getattr(tgt, "build_state", None) is not None:
            tw = getattr(tgt, "build_tower", None)
            site = (tgt.build_x, tgt.build_y) if tgt.build_state != "wander" else (tgt.x, tgt.y)
            if tw is not None and tw in self.towers:
                self.towers.remove(tw)
            if tgt.build_state in ("building", "done"):
                self.texts.append(FloatText(site[0], site[1] - 24, "it rips apart!", (200, 90, 80)))
                for _ in range(24):
                    self.parts.append(Particle(site[0] + random.uniform(-24, 24),
                                               site[1] + random.uniform(-20, 20),
                                               (196, 170, 120), 120, 0.7, 3))
        if isinstance(tgt, Unit) and tgt.sigil == "drinker":
            for au in self.units:
                if au.team == tgt.team and au.alive and math.hypot(au.x - tgt.x, au.y - tgt.y) < 120:
                    au.hp = min(au.maxhp, au.hp + au.maxhp * 0.08)
        if isinstance(tgt, Unit) and tgt.sigil == "manyfold" and not tgt.split_done:
            tgt.split_done = True
            for s in (-1, 1):
                pup = Unit(tgt.key, tgt.x + s * 16, tgt.y, tgt.team)
                pup.hp = pup.maxhp = tgt.maxhp * 0.5
                pup.dmg = int(tgt.dmg * 0.6); pup.size = int(tgt.size * 0.8)
                pup.sigil = "none"; pup.split_done = True
                pup.spawn_t = 0.2
                self.units.append(pup)
            self.texts.append(FloatText(tgt.x, tgt.y - 24, "splits!", (200, 200, 210)))
        # spirit divergence (rare variant): on death, the unit diverges into two
        # shadow echoes — 2 per unit of the base card — that hit for 25% and carry
        # 15% of the base card's health. Echoes don't re-diverge (spirit_done).
        if (isinstance(tgt, Unit) and getattr(tgt, "variant_ability", None) == "spiritdiverge"
                and not tgt.spirit_done):
            tgt.spirit_done = True
            base_count = max(1, CARDS.get(tgt.key, {}).get("count", 1))
            n_echoes = 2 * base_count
            for i in range(n_echoes):
                ang = (i / n_echoes) * 6.2832
                ex = tgt.x + math.cos(ang) * 16
                ey = tgt.y + math.sin(ang) * 12
                echo = Unit(tgt.key, ex, ey, tgt.team)
                echo.hp = echo.maxhp = max(1.0, tgt.maxhp * 0.15)
                echo.dmg = max(1, int(tgt.dmg * 0.25))
                echo.size = max(6, int(tgt.size * 0.7))
                echo.paper = shade(getattr(tgt, "paper", PAGE), 0.45)  # ghostly
                echo.sigil = "none"                 # a plain wraith; no chained deaths
                echo.variant_ability = None
                echo.spirit_done = True
                echo.split_done = True
                echo.is_echo = True
                echo.spawn_t = 0.25
                self.units.append(echo)
            for _ in range(18):
                self.parts.append(Particle(tgt.x, tgt.y, (150, 90, 190), 150, 0.7, 3))
            self.texts.append(FloatText(tgt.x, tgt.y - 24, "diverges!", (170, 110, 210)))

    def _cast_tidal(self, x, y, team, card):
        """Tidal Wave spell: loose THREE cresting waves that roll straight toward
        the enemy side in a wide band. Each wave sweeps the lane once, damaging &
        shoving whatever it washes over. Buildings take reduced damage and barely
        move. Deterministic: the waves advance on the fixed sim clock, spaced by a
        launch delay so they arrive as a three-beat set. Never uses local RNG."""
        dirn = -1 if team == 0 else 1          # team 0 pushes up-page, team 1 down
        dmg = card.get("dmg", 120)
        width = 150                            # half-width of the band (px each side)
        speed = 260                            # px/sec the wave front travels
        reach = 520                            # how far it rolls before dissipating
        for i in range(3):                     # a three-shot set
            self.waves.append({
                "x": x, "y0": y, "y": y, "team": team, "dir": dirn,
                "dmg": dmg, "width": width, "speed": speed,
                "traveled": 0.0, "reach": reach,
                "delay": i * 0.35,             # staggered launch — three beats
                "hit": set(),                  # unit ids already struck by THIS wave
                "gaps": [],                    # (gx, half) shadows carved by obstacles
                "t": 0.0,
            })
        self.texts.append(FloatText(x, y - 30, "TIDAL WAVE!", (120, 190, 235)))
        for a in range(16):
            ang = a / 16 * 6.2832
            self.parts.append(Particle(x + math.cos(ang) * 20, y + math.sin(ang) * 12,
                                       (150, 200, 235), 90, 0.5, 3))
        sfx("deploy", 0.8)

    def _tick_waves(self):
        """Advance every live Tidal Wave, applying damage + knockback to units and
        towers the front sweeps past this tick. Obstacles SPLIT the wave: a troop
        carves a small shadow, a building a wide one, so anything in a building's
        lee is shielded — you can wall off a tidal wave. Buildings also take
        reduced damage and barely move. Deterministic; stable iteration order."""
        if not self.waves:
            return
        for w in self.waves:
            w["t"] += DT
            if w["t"] < w["delay"]:
                continue                       # still winding up
            step = w["speed"] * DT
            w["y"] += w["dir"] * step
            w["traveled"] += step
            fy = w["y"]
            lo_x, hi_x = w["x"] - w["width"], w["x"] + w["width"]
            band = 46                          # vertical thickness of the crest
            # process obstacles in the order the wave REACHES them (front-to-back
            # along travel), so a building carves its shadow before things behind
            # it are tested. dir<0 → smaller y first; dir>0 → larger y first.
            foes = sorted(self.enemies_of(w["team"]),
                          key=lambda f: (w["dir"] * f.y, getattr(f, "uid", 0)))
            for f in foes:
                fid = getattr(f, "uid", id(f))
                if fid in w["hit"]:
                    continue
                if not (lo_x <= f.x <= hi_x):
                    continue
                if abs(f.y - fy) > band:
                    continue
                # blocked? if this x sits inside a shadow carved upstream, the wave
                # never reached here — spare the target entirely.
                if any(abs(f.x - gx) < gh for gx, gh in w["gaps"]):
                    continue
                is_bldg = isinstance(f, Tower) or getattr(f, "building", False)
                dmg = w["dmg"] * (0.35 if is_bldg else 1.0)
                f.hp -= dmg
                w["hit"].add(fid)
                # carve a shadow behind the obstacle: buildings block a lot, troops
                # only split the flow a little. Scales with the obstacle's size.
                osz = getattr(f, "size", 24)
                gh = (osz * 2.2) if is_bldg else (osz * 0.7)
                w["gaps"].append((f.x, gh))
                # knockback: greatly reduced on buildings, none if anchored
                if isinstance(f, Unit) and not getattr(f, "anchored", False):
                    push = 14 if is_bldg else 70
                    f.y += w["dir"] * push
                    f.x += (f.x - w["x"]) * 0.04
                    if not is_bldg:
                        f.knock_timer = 0.35
                self.texts.append(FloatText(f.x, f.y - 20, str(int(dmg)), (150, 200, 235)))
            # spray along the crest (skipping the shadowed gaps)
            if random.random() < 0.6:
                px = w["x"] + random.uniform(-w["width"], w["width"])
                if not any(abs(px - gx) < gh for gx, gh in w["gaps"]):
                    self.parts.append(Particle(px, fy, (170, 210, 240), 60, 0.4, 3))
        # retire waves that have rolled their full reach
        self.waves = [w for w in self.waves if w["traveled"] < w["reach"]]

    def _raise_zombie(self, tgt):
        """Reincarnate a fallen unit as a shambling member of the hostile horde
        (team 2). The zombie is a weakened copy that hunts the nearest of EITHER
        player's units/towers. Fully deterministic — derives everything from the
        sim state, never local RNG — so both net peers raise identical zombies."""
        try:
            z = Unit(tgt.key, tgt.x, tgt.y, 2)          # team 2 = the horde
        except Exception:
            return
        z.is_zombie = True
        z.spawn_t = 0.3
        # decayed: a fraction of its former self, slower but relentless
        z.maxhp = max(1, int(z.maxhp * 0.6))
        z.hp = z.maxhp
        z.dmg = max(1, int(getattr(z, "dmg", 1) * 0.7))
        z.spd = getattr(z, "spd", 1.0) * 0.7
        z.paper = (96, 120, 84)                          # sickly green-grey
        z.sigil = "none"                                 # strip special abilities
        for attr in ("split_done", "quill_done", "reborn", "is_echo", "is_eagle"):
            try: setattr(z, attr, True if attr in ("split_done", "quill_done", "reborn") else False)
            except Exception: pass
        self.units.append(z)
        for _ in range(12):
            self.parts.append(Particle(tgt.x, tgt.y, (120, 160, 90), 90, 0.8, 3))
        self.texts.append(FloatText(tgt.x, tgt.y - 24, "risen!", (140, 200, 110)))

    def _trickster_tick(self):
        """The Candlethief's darkness duel. Periodically he snuffs the light and
        laughs; your hand goes to shadow and you can't read your cards. He drifts
        in and out of the room — while he's *present*, relighting the candle makes
        him screech and blast all your units/structures; while he's *absent*, you
        can relight safely. Read the hints and pick your moment.

        Deterministic timing (seeded from match_seed + tick) so it's replay/net
        safe; the player's relight is the only input."""
        if self.trick_laughed > 0:
            self.trick_laughed -= DT
        if not self.trick_dark:
            # counting down to the next blackout; comes faster in later phases
            self.trick_timer -= DT
            if self.trick_timer <= 0:
                self.trick_dark = True
                self.trick_dark_t = 0.0
                self.trick_hand_shadow = True
                self.trick_laughed = 1.4
                self.snuff_candle = 0        # snuff the player's (right) candle
                # start present so the opening moment is dangerous, then oscillate
                self.trick_present = True
                self.trick_present_t = 0.0
                self.texts.append(FloatText(BOOK.centerx, BOOK.top + 90,
                                            "the lights go out…", (150, 110, 200)))
                sfx("error")
            return
        # ── in the dark ──
        self.trick_dark_t += DT
        self.trick_present_t += DT
        # he flickers between present (danger) and absent (safe) on a seeded clock
        prng = random.Random((self.match_seed ^ (int(self.trick_dark_t * 3) * 2654435761)) & 0xffffffff)
        dwell = (1.4 if self.trick_present else 1.9) + prng.uniform(-0.4, 0.6)
        if self.trick_present_t >= dwell:
            self.trick_present_t = 0.0
            self.trick_present = not self.trick_present
            if self.trick_present and prng.random() < 0.5:
                self.trick_laughed = 1.0     # a laugh when he slips back in
                sfx("error", 0.5)
        # periodically shuffle/re-shadow the hidden hand so you can't just memorise
        self.trick_shuffle_t += DT
        if self.trick_shuffle_t > 2.5:
            self.trick_shuffle_t = 0.0
            self.rot.shadow_shuffle()
        # if he's ignored too long he eventually relights himself, laughing — but
        # never blasts you for his own relight (that would be unfair)
        if self.trick_dark_t > 14.0:
            self._trickster_relight(safe=True, forced=True)

    def _trickster_relight(self, safe=None, forced=False):
        """The player strikes flint & steel (or the dark times out). If he's
        present, he screeches and damages everything you own; if not, the lights
        simply come back."""
        present = self.trick_present and not forced
        self.trick_dark = False
        self.trick_hand_shadow = False
        self.snuff_candle = None
        self.trick_timer = max(6.0, 12.0 - self.boss_phase * 2.0)
        if present:
            # SCREECH: AoE damage to all your units and towers, scaled by phase
            frac = 0.10 + 0.05 * self.boss_phase
            for u in self.units:
                if u.team == 0 and u.alive:
                    u.hp -= u.maxhp * frac
                    if u.hp <= 0: self._on_death(u)
            for tw in self.towers:
                if tw.team == 0 and tw.alive:
                    tw.hp -= tw.maxhp * (frac * 0.6)
            self.units = [u for u in self.units if u.hp > 0 or u.team == 1]
            self.add_shockwave(BOOK.centerx, RIVER_Y + 120, mag=30, dur=0.9)
            self.texts.append(FloatText(BOOK.centerx, BOOK.top + 90, "SCREECH!", (220, 70, 90)))
            for _ in range(40):
                self.parts.append(Particle(random.uniform(BOOK.left, BOOK.right),
                                           random.uniform(RIVER_Y, BOOK.bottom),
                                           (180, 60, 90), 160, 0.8, 3))
            sfx("hit", 1.0)
        else:
            self.texts.append(FloatText(BOOK.centerx, BOOK.top + 90,
                                        "light returns.", (240, 210, 150)))
            sfx("deploy", 0.5)

    def _boss_ability(self):
        """Boss special attacks, scaled by self.boss_phase. Deploys/effects come
        from the enemy side (team 1) and get nastier as the boss loses HP."""
        ph = self.boss_phase
        if self.boss == "angler":
            # the Angler summons a school of piranha-frogs on the flanks
            n = 1 + ph
            for _ in range(n):
                bx = random.choice(BRIDGE_XS) + random.randint(-40, 40)
                self.deploy("frog", bx, BOOK.top + 120, 1)
            self.texts.append(FloatText(BOOK.centerx, BOOK.top + 100, "a school rises!", WAX))
        elif self.boss == "scribe":
            # The Scribe blots the page: ink puddles bloom across the player's half
            for _ in range(2 + ph):
                px = random.uniform(BOOK.left + 60, BOOK.right - 60)
                py = random.uniform(RIVER_Y + 40, BOOK.bottom - 60)
                self.ink_puddles.append(dict(x=px, y=py, timer=4.0, team=1))
                self.boss_fx.append(dict(kind="blot", x=px, y=py, t=0.0))
            # and in later phases summons ink golems
            if ph >= 2:
                self.deploy("inkgolem", BOOK.centerx + random.randint(-80, 80), BOOK.top + 130, 1)
            self.texts.append(FloatText(BOOK.centerx, BOOK.top + 100, "the page blots!", (80, 20, 80)))
            self.add_shockwave(BOOK.centerx, BOOK.top + 120, mag=20, dur=0.7)
        elif self.boss == "clockking":
            # The Clockwork King freezes time for the player's units briefly (stun),
            # and in higher phases fires a sweeping gear volley
            for u in self.units:
                if u.team == 0 and u.alive:
                    u.stunned = max(u.stunned, 0.6 + 0.3 * ph)
            self.boss_fx.append(dict(kind="tick", x=BOOK.centerx, y=RIVER_Y, t=0.0))
            self.add_shockwave(BOOK.centerx, RIVER_Y, mag=18, dur=0.6)
            if ph >= 2:
                for _ in range(ph):
                    kx = random.choice(BRIDGE_XS)
                    self.projs.append([kx, BOOK.top + 60,
                                       min((u for u in self.units if u.team == 0 and u.alive),
                                           key=lambda u: u.y, default=None) or
                                       next(t for t in self.towers if t.team == 0 and t.king),
                                       70 + 20 * ph])
            self.texts.append(FloatText(BOOK.centerx, BOOK.top + 100, "time stutters!", (150, 120, 70)))

    def _ensure_reliable(self):
        """Lazily build the reliable-input channels once a peer is attached."""
        if self.net is None or not NETREL_OK:
            return
        if self._rel_out is None:
            self._rel_out = netreliable.ReliableOut(self.net.send)
        if self._rel_in is None:
            self._rel_in = netreliable.ReliableIn()

    def _send_deploy(self, key, x, y, team, apply_tick, seat=None):
        """Send a deploy reliably (seq + resend + same-frame batching) when the
        layer is available, else fall back to a plain unreliable send."""
        msg = {"t": "deploy", "tick": apply_tick, "key": key,
               "x": x, "y": y, "team": team, "seat": seat}
        self._ensure_reliable()
        if self._rel_out is not None:
            self._rel_out.send_deploy(msg)      # flushed once per frame
        elif self.net:
            self.net.send(msg)

    def net_pump(self, now=None):
        """Per-frame reliability tick: flush batched deploys (coalescing bursts
        within the input-delay budget), resend anything unacked, and send acks.
        Call once per frame while in a net match, after processing inbound.

        Ink-aware nudge: if you're out of ink you can't deploy again for a while,
        so there's no point holding a pending batch waiting for a follow-up —
        force it out now to shave latency. When ink is available (you might drop
        another card any moment) we let the short coalescing window do its job.

        Acks are staged onto the reliable channel so they ride along inside the
        next outgoing bundle (piggybacked) instead of costing their own packet."""
        if self._rel_out is not None:
            # stage this frame's acks so flush()/resend_due() can piggyback them
            if self._rel_in is not None:
                self._rel_out.queue_acks(self._rel_in.drain_acks())
            starved = self.ink < 1.0     # can't afford even the cheapest play soon
            self._rel_out.flush(now, force=starved)
            self._rel_out.resend_due(now)
        elif self._rel_in is not None and self.net:
            # no reliable-out channel (shouldn't happen in practice) → send acks raw
            acks = self._rel_in.drain_acks()
            if acks:
                self.net.send({"t": "ack", "seqs": acks})

    def net_recv_deploy(self, msg):
        """Handle an inbound deploy / batched inputs / bundle: dedupe by seq so a
        resend never double-applies, schedule new inputs, and absorb any
        piggybacked acks. Returns applied count."""
        self._ensure_reliable()
        applied = 0
        mt = msg.get("t")
        if mt == "bundle":
            # a bundle carries many deploys (across ticks) + piggybacked acks
            self.net_note_ack({"seqs": msg.get("acks", [])})
            for d in msg.get("deploys", []):
                seq = d.get("seq")
                if self._rel_in is None or seq is None or self._rel_in.accept(seq):
                    self.net_schedule(d["key"], d["x"], d["y"], d["team"], d["tick"])
                    applied += 1
        elif mt == "inputs":
            seqs = msg.get("seqs", [])
            for i, it in enumerate(msg.get("items", [])):
                seq = seqs[i] if i < len(seqs) else None
                if self._rel_in is None or seq is None or self._rel_in.accept(seq):
                    self.net_schedule(it["key"], it["x"], it["y"], it["team"], msg["tick"])
                    applied += 1
        else:
            seq = msg.get("seq")
            if self._rel_in is None or seq is None or self._rel_in.accept(seq):
                self.net_schedule(msg["key"], msg["x"], msg["y"], msg["team"], msg["tick"])
                applied += 1
        return applied

    def net_note_ack(self, msg):
        if self._rel_out is not None:
            if "seqs" in msg:
                self._rel_out.note_acks(msg["seqs"])
            elif "seq" in msg:
                self._rel_out.note_ack(msg["seq"])

    def net_schedule(self, key, x, y, team, tick):
        """Queue a deploy to fire on a specific integer tick (both peers)."""
        self.pending_inputs.setdefault(tick, []).append((key, x, y, team))

    def add_shockwave(self, x, y, mag=14.0, dur=0.5):
        """Spawn a screen-distortion ripple (hi-gfx only, visual — no sim effect)."""
        self.shockwaves.append({"x": x, "y": y, "t": 0.0, "dur": dur, "mag": mag})

    def opponent_forfeited(self, team):
        """A networked opponent forfeit/left. Destroy that team's king so the
        FSM resolves to a victory for the other side, snuff their candle, and
        flag a notice banner for the remaining player."""
        if self.result:
            return
        for tw in self.towers:
            if tw.team == team and tw.king:
                tw.hp = 0
        self.forfeit_notice = team          # which team bailed (for the banner)
        self.snuff_candle = (0 if team == 0 else 1)   # snuff their side's candle
        # resolve immediately through the FSM
        self.update()

    def net_local_play(self, idx, pos, seat=None):
        """Player clicks the board in a net game. Spend ink locally, broadcast the
        deploy stamped INPUT_DELAY ticks ahead, and schedule it on our own sim.
        Returns 'played' | 'noink' | None."""
        if self.result: return None
        hand = self.rot.hand
        if idx < 0 or idx >= len(hand): return None
        key = hand[idx]
        _cost = self.cost_of(key, self.my_team)
        if self.ink < _cost:
            self._bounce(idx, "bottom")
            return "noink"
        x, y = pos
        my_team = self.my_team
        seat = seat if seat is not None else (self.my_seats[0] if self.my_seats else 0)
        if my_team == 0 and y < RIVER_Y + RIVER_H: y = RIVER_Y + RIVER_H + 10
        if my_team == 1 and y > RIVER_Y - RIVER_H: y = RIVER_Y - RIVER_H - 10
        self.ink -= _cost
        apply_tick = self.tick + INPUT_DELAY
        self.net_schedule(key, x, y, my_team, apply_tick)
        if self.net:
            self._send_deploy(key, x, y, my_team, apply_tick, seat)
        self.rot.play(idx)
        return "played"

    def net_seat_play(self, seat, idx, pos):
        """Deploy from a specific local seat's controller (2v2 dual-local device).
        Each seat has its own rotation + ink but shares this device's team.
        Returns 'played' | 'noink' | None."""
        if self.result: return None
        ctrl = self.seat_ctrl.get(seat)
        if not ctrl: return None
        hand = ctrl["rot"].hand
        if idx < 0 or idx >= len(hand): return None
        key = hand[idx]
        team = seat_team_local(seat)
        _cost = self.cost_of(key, team)
        if ctrl["ink"] < _cost:
            self._bounce(idx, "seat", seat)
            return "noink"
        x, y = pos
        if team == 0 and y < RIVER_Y + RIVER_H: y = RIVER_Y + RIVER_H + 10
        if team == 1 and y > RIVER_Y - RIVER_H: y = RIVER_Y - RIVER_H - 10
        ctrl["ink"] -= _cost
        apply_tick = self.tick + INPUT_DELAY
        self.net_schedule(key, x, y, team, apply_tick)
        if self.net:
            self._send_deploy(key, x, y, team, apply_tick, seat)
        ctrl["rot"].play(idx)
        return "played"

    def _apply_tick_inputs(self):
        for (key, x, y, team) in self.pending_inputs.pop(self.tick, []):
            self.deploy(key, x, y, team)

    def update(self):
        if self.result:
            for lst in (self.parts, self.texts):
                for e in lst: e.update()
            self.parts = [p for p in self.parts if p.life > 0]
            self.texts = [t for t in self.texts if t.life > 0]
            return

        # networked deploys scheduled for this tick fire on both machines
        self._apply_tick_inputs()
        self.tick += 1

        self.time += DT
        _ir = MODES[self.mode]["ink_rate"] / 2.8 * 1.025   # +2.5% ink gain globally
        _cap = ink_cap(self.mode)
        # ── carrier (Beekeeper): each live one on your side slows your ink gain,
        #    the price of the swarm. Per-team so it hits the right pool. ──
        drain0 = sum(0.3395 for u in self.units
                     if u.alive and getattr(u, "sigil", None) == "carrier" and u.team == 0)
        drain1 = sum(0.3395 for u in self.units
                     if u.alive and getattr(u, "sigil", None) == "carrier" and u.team == 1)
        r0 = max(0.15, _ir * (1.0 - min(0.7, drain0)))
        r1 = max(0.15, _ir * (1.0 - min(0.7, drain1)))
        self.ink = min(_cap, self.ink + DT * r0)
        self.ai_ink = min(_cap, self.ai_ink + DT * r1)
        if self.local_1v1:
            self.ai_ink2 = min(_cap, self.ai_ink2 + DT * r0)
        # per-seat ink pools (2v2 dual-local device)
        for ctrl in self.seat_ctrl.values():
            _teamdrain = drain0 if ctrl.get("team", 0) == 0 else drain1
            _r = max(0.15, _ir * (1.0 - min(0.7, _teamdrain)))
            ctrl["ink"] = min(_cap, ctrl["ink"] + DT * _r)
        # decay 'not enough ink' bounce animations
        if self.card_bounce:
            for k in list(self.card_bounce):
                self.card_bounce[k] -= DT
                if self.card_bounce[k] <= 0:
                    del self.card_bounce[k]

        if self.hand_reach:
            self.hand_t += DT
            # fire the deploy as the reach animation passes its midpoint. Use a
            # crossing test (>=) with the key-cleared-to-None guard, NOT an open
            # interval — an interval one DT-step wide can land exactly on both
            # bounds and never trigger (which silently disabled the AI).
            if self.hand_t >= 0.55 and self.hand_reach[3] is not None:
                self.deploy(self.hand_reach[3], self.hand_reach[0], self.hand_reach[1], 1)
                self.hand_reach = (self.hand_reach[0], self.hand_reach[1],
                                   self.hand_reach[2], None)
            if self.hand_t > 1.2:
                self.hand_reach = None

        # local AI only runs in single-player skirmish (not net, not hotseat 1v1,
        # not during replay playback — replays inject the recorded deploys instead).
        # It draws from its OWN rng (self.ai_rng) so that whether or not the AI
        # runs doesn't perturb the sim's global random stream — that keeps a
        # recorded match and its replay bit-identical (replay disables the AI but
        # re-injects its logged deploys).
        if self.netmode is None and not self.local_1v1 and not getattr(self, "is_replay", False):
            self.ai_timer -= DT
            if self.ai_timer <= 0 and self.ai_ink >= 5 and not self.hand_reach:
                arng = self.ai_rng
                key = arng.choice(["pangolin", "wolverine", "batcolony", "gorilla",
                                   "cassowary", "honeybadger", "mothswarm"])
                if self.ai_ink >= CARDS[key]["cost"]:
                    self.ai_ink -= CARDS[key]["cost"]
                    bx = arng.choice(BRIDGE_XS)
                    self.hand_reach = (bx + arng.randint(-30, 30), BOOK.top + 130, bx, key)
                    self.hand_t = 0.0
                    self.ai_timer = arng.uniform(4.5, 7.5)

        # ---- boss phase escalation + special abilities (single-player only)
        if self.boss and self.netmode is None:
            e_king = next((t for t in self.towers if t.team == 1 and t.king), None)
            if e_king:
                frac = e_king.hp / e_king.maxhp
                new_phase = 1 if frac > 0.66 else (2 if frac > 0.33 else 3)
                if new_phase != self.boss_phase:
                    self.boss_phase = new_phase
                    self.texts.append(FloatText(BOOK.centerx, BOOK.top + 80,
                                                f"— phase {new_phase} —", WAX))
            # AI Arch-Scribes are driven by their BossMind in the main loop, not
            # by the fixed ability table below.
            if not self.ai_boss_id:
                self.boss_timer -= DT
                if self.boss_timer <= 0:
                    self._boss_ability()
                    # abilities come faster in later phases
                    self.boss_timer = max(3.0, 8.0 - self.boss_phase * 1.5)
            # The Trickster runs its own darkness duel every tick
            if self.boss == "trickster":
                self._trickster_tick()

        # ---- Angler boss: hook a player unit near the river and drag it across
        if self.boss == "angler" and self.netmode is None and not self.hook_anim:
            self.hook_timer -= DT
            if self.hook_timer <= 0:
                candidates = [u for u in self.units if u.team == 0 and u.alive
                              and not u.building and abs(u.y - RIVER_Y) < 130]
                if candidates:
                    tgt = min(candidates, key=lambda u: abs(u.y - RIVER_Y))
                    self.hook_anim = [tgt, 0.0, "cast"]
                self.hook_timer = random.uniform(9, 13)
        if self.hook_anim:
            tgt, ht, phase = self.hook_anim
            ht += DT
            if not tgt.alive:
                self.hook_anim = None
            elif phase == "cast" and ht > 0.5:
                self.hook_anim = [tgt, 0.0, "reel"]
                for _ in range(10):
                    self.parts.append(Particle(tgt.x, tgt.y, INK, 100, 0.5, 3))
            elif phase == "reel":
                lx, ly = self.hook_lure
                d = math.hypot(lx - tgt.x, ly - tgt.y) or 1
                sp = 220 * DT
                if d < sp:
                    tgt.team = 1; tgt.face = -1; tgt.hp = min(tgt.hp, tgt.maxhp * 0.6)
                    tgt.hooked = False
                    self.texts.append(FloatText(tgt.x, tgt.y - 20, "hooked!", WAX))
                    for _ in range(16):
                        self.parts.append(Particle(tgt.x, tgt.y, INK, 130, 0.6, 3))
                    self.hook_anim = None
                else:
                    tgt.hooked = True
                    tgt.x += (lx - tgt.x) / d * sp
                    tgt.y += (ly - tgt.y) / d * sp
                self.hook_anim = [tgt, ht, phase] if self.hook_anim else None

        for u in self.units:
            if not u.alive: continue
            u.spawn_t = max(0, u.spawn_t - DT)
            u.cool = max(0, u.cool - DT)
            u.fire_flash = max(0, getattr(u, "fire_flash", 0) - DT)
            # a launched eagle flies its own course (out → bomb → home); it can be
            # shot at like any flyer, and retreats if bloodied.
            if getattr(u, "is_eagle", False):
                if self._eagle_tick(u):
                    continue
            # ghostly (Glass Tortoise): the first time it nears an enemy wall it turns
            # translucent and phases — a short window of damage immunity to slip past.
            if u.sigil == "ghostly":
                if u.ghost_t > 0:
                    u.ghost_t = max(0, u.ghost_t - DT)
                elif not u.ghost_done:
                    near_wall = any(tw.alive and tw.team != u.team
                                    and math.hypot(tw.x - u.x, tw.y - u.y) < tw.size + 70
                                    for tw in self.towers)
                    if near_wall:
                        u.ghost_done = True
                        u.ghost_t = 1.6
                        self.texts.append(FloatText(u.x, u.y - 24, "phases!", (180, 220, 230)))
            if u.dot > 0:
                tick = min(u.dot, 8 * DT)
                u.hp -= tick; u.dot -= tick
                if random.random() < 0.15:
                    self.parts.append(Particle(u.x, u.y, (72, 150, 78), 40, 0.3, 2))
            if u.sigil == "dormant" and u.asleep:
                if u.hp < u.maxhp:            # took damage -> wake up enraged
                    u.asleep = False; u.dmg = int(u.dmg * 1.5)
                    self.texts.append(FloatText(u.x, u.y - 24, "awake!", WAX))
                else:
                    continue                 # sleeps: no move, no attack
            if u.beach_t > 0:                # great white drying out on land
                u.beach_t -= DT
                if abs(u.y - RIVER_Y) > RIVER_H:
                    u.hp -= u.maxhp * 0.03 * DT
            # ── siege (Eagle's Nest): a temporary emplacement — it crumbles after
            #    its lifespan, chipping HP so it visibly ages and finally collapses.
            if getattr(u, "sigil", None) == "siege" and u.life_t > 0:
                u.life_t -= DT
                if u.life_t <= 0:
                    u.hp = 0                 # crumbles
                    self.texts.append(FloatText(u.x, u.y - 24, "crumbles!", (150, 90, 60)))
                    for _ in range(18):
                        self.parts.append(Particle(u.x, u.y, (120, 100, 80), 90, 0.6, 3))
                elif u.life_t < 5.0 and random.random() < 0.05:
                    self.parts.append(Particle(u.x, u.y, (140, 120, 96), 50, 0.5, 2))  # crumbling dust
            if u.hooked: continue
            # ── bee interceptor: dies with its keeper; leashes near the keeper and
            #    only chases foes within a short range, then drifts back to orbit. ──
            if getattr(u, "from_carrier", None) is not None:
                parent = u.from_carrier
                if not parent.alive:
                    u.hp = 0
                    for _ in range(3):
                        self.parts.append(Particle(u.x, u.y, (230, 200, 70), 30, 0.3, 2))
                    continue
                bfoes = [f for f in self.enemies_of(u.team)
                         if isinstance(f, Unit)                      # bees harry units, not towers
                         and not getattr(f, "tunneled", False)
                         and math.hypot(f.x - parent.x, f.y - parent.y) < 210]
                if bfoes:
                    tgt = min(bfoes, key=lambda f: math.hypot(f.x - u.x, f.y - u.y))
                    dd = math.hypot(tgt.x - u.x, tgt.y - u.y) or 1
                    if dd <= u.rng + getattr(tgt, "size", 12):
                        if u.cool <= 0:
                            u.cool = 0.5; u.fire_flash = 0.2
                            tgt.hp -= u.dmg
                            if isinstance(tgt, Unit):
                                self.texts.append(FloatText(tgt.x, tgt.y-20, str(int(u.dmg)), (240,220,120)))
                            if tgt.hp <= 0: self._on_death(tgt)
                    else:
                        u.x += (tgt.x - u.x)/dd * u.spd * DT
                        u.y += (tgt.y - u.y)/dd * u.spd * DT
                        u.face = 1 if tgt.x >= u.x else -1
                else:
                    ang = self.time * 3.0 + getattr(u, "bee_phase", 0.0)
                    hx = parent.x + math.cos(ang) * 34
                    hy = parent.y - parent.size * 0.7 + math.sin(ang) * 20
                    dd = math.hypot(hx - u.x, hy - u.y) or 1
                    u.x += (hx - u.x)/dd * min(dd, u.spd*DT)
                    u.y += (hy - u.y)/dd * min(dd, u.spd*DT)
                continue
            # ── carrier (Beekeeper): keep a small swarm of bee interceptors aloft.
            #    Launch up to a cap; dead bees are pruned so it re-launches. Bees
            #    are tiny flyers tagged to this parent. ──
            if getattr(u, "sigil", None) == "carrier":
                u.bees = [b for b in u.bees if b.alive]
                u.bee_cd = max(0, u.bee_cd - DT)
                if len(u.bees) < 4 and u.bee_cd <= 0 and "bee" in CARDS:
                    # deterministic spawn offset by slot (no RNG → net-safe)
                    slot = len(u.bees)
                    ox = (slot - 1.5) * 10.0
                    bee = self.deploy("bee", u.x + ox, u.y - u.size * 0.6, u.team)
                    if bee is not None:
                        bee.from_carrier = u
                        bee.bee_home = u
                        # a stable orbit phase per bee slot (deterministic — both
                        # peers spawn bees in the same order), spread around a ring
                        bee.bee_phase = slot * 1.5708
                        u.bees.append(bee)
                    u.bee_cd = 1.1
                    for _ in range(3):
                        self.parts.append(Particle(u.x - u.size*0.5, u.y - u.size*0.4,
                                                   (240, 220, 120), 40, 0.4, 2))
                # the beekeeper itself still walks/fights normally below
            # ── mole tunnel: dig straight across (ignoring bridges/river) while
            #    underground & untargetable; surface once safely on the far bank. ──
            if getattr(u, "tunneled", False):
                goal_y = (BOOK.top + 120) if u.team == 0 else (BOOK.bottom - 120)
                dirn = 1 if goal_y > u.y else -1
                u.y += dirn * u.spd * 1.4 * DT      # digs a bit faster than it walks
                u.face = 1
                # a little dirt trail
                if random.random() < 0.4:
                    self.parts.append(Particle(u.x, u.y + u.size * 0.4,
                                               (120, 96, 72), 40, 0.4, 3))
                crossed = (u.y <= RIVER_Y - RIVER_H - 10) if u.team == 0 else (u.y >= RIVER_Y + RIVER_H + 10)
                if crossed:
                    u.tunneled = False              # surface — now targetable & attacks
                    self.texts.append(FloatText(u.x, u.y - 24, "surfaces!", (140, 110, 80)))
                    for _ in range(12):
                        self.parts.append(Particle(u.x, u.y, (120, 96, 72), 70, 0.5, 3))
                    sfx("deploy", 0.5)
                continue                            # skip normal move/attack while digging
            # ── the Foldwright (builder): wander to open ground on the friendly
            #    half, then spend time folding a whole new tower troop. It never
            #    fights. If it's slain mid-job, _on_death rips the structure down. ──
            if getattr(u, "build_state", None) is not None and u.build_state != "done":
                u.face = 1
                if u.build_state == "wander":
                    u.wander_t += DT
                    if u.wander_goal is None or math.hypot(u.x - u.wander_goal[0], u.y - u.wander_goal[1]) < 18:
                        u.wander_goal = self._builder_wander_goal(u)
                    gx, gy = u.wander_goal
                    ang = math.atan2(gy - u.y, gx - u.x)
                    u.x += math.cos(ang) * u.spd * 0.8 * DT
                    u.y += math.sin(ang) * u.spd * 0.8 * DT
                    u.face = 1 if math.cos(ang) >= 0 else -1
                    if u.wander_t > 2.2 and self._spot_is_clear(u.x, u.y, u.team):
                        u.build_state = "building"; u.build_t = 0.0
                        u.build_x, u.build_y = u.x, u.y
                        self.texts.append(FloatText(u.x, u.y - 26, "folding…", (196, 170, 120)))
                elif u.build_state == "building":
                    u.build_t += DT / 100.0          # folds over ~100s (20× longer)
                    # cosmetic fold-dust — seed from tick+uid so it doesn't touch
                    # the shared RNG stream (keeps lockstep deterministic).
                    prng = random.Random((self.tick * 31 + getattr(u, "_spawn_key", u.uid)) & 0xffffffff)
                    if prng.random() < 0.25:
                        self.parts.append(Particle(u.build_x + prng.uniform(-20, 20),
                                                   u.build_y + prng.uniform(-16, 16),
                                                   (196, 170, 120), 50, 0.5, 3))
                    if u.build_t >= 1.0:
                        tk = self._builder_troop(u)
                        tw = Tower(u.build_x, u.build_y, u.team, troop=tk)
                        tw.hp = tw.maxhp = int(tw.maxhp * MODES[self.mode]["tower_hp"])
                        u.build_tower = tw
                        self.towers.append(tw)
                        u.build_state = "done"
                        self.texts.append(FloatText(u.build_x, u.build_y - 30, "raised!", (150, 210, 150)))
                        for _ in range(20):
                            self.parts.append(Particle(u.build_x, u.build_y, (196, 170, 120), 90, 0.6, 3))
                        sfx("deploy", 0.7)
                continue                            # builders never move/attack normally
            # buildings like the Ink Obelisk fall through and fire at range
            if u.building and (u.dmg <= 0 or u.rng <= 0): continue
            foes = self.enemies_of(u.team)
            if u.sigil == "towerbane":
                foes = [f for f in foes if isinstance(f, Tower)]
            # burrow: burrowed units can't be targeted until they surface to strike
            foes = [f for f in foes if not (isinstance(f, Unit) and getattr(f, "burrowed", False))]
            # tunnel: the mole is underground and can't be targeted until it surfaces
            foes = [f for f in foes if not (isinstance(f, Unit) and getattr(f, "tunneled", False))]
            if not foes: continue
            taunts = [f for f in foes if isinstance(f, Unit) and f.sigil == "taunt"
                      and math.hypot(f.x - u.x, f.y - u.y) < 150]
            # beacon: an enemy building can pull this unit's aim like a soft taunt
            beacons = [f for f in foes if isinstance(f, Unit) and f.sigil == "beacon"
                       and math.hypot(f.x - u.x, f.y - u.y) < 180]
            pool = taunts or beacons or foes
            tgt = min(pool, key=lambda f: math.hypot(f.x - u.x, f.y - u.y))
            # copycat: on first sight of a foe, morph into a copy of it
            if u.sigil == "copycat" and not u.copied and isinstance(tgt, Unit):
                u.copied = True
                src = CARDS[tgt.key]
                frac = u.hp / u.maxhp
                u.maxhp = src["hp"]; u.hp = u.maxhp * frac
                u.dmg = src["dmg"]; u.spd = src["spd"]; u.rng = src["rng"]
                u.size = src["size"]; u.paper = src["paper"]; u.key = tgt.key
                self.texts.append(FloatText(u.x, u.y - 28, "mimics!", (170, 150, 110)))
            dist = math.hypot(tgt.x - u.x, tgt.y - u.y)
            reach = u.rng + getattr(tgt, "size", 20)
            # grasp (Squid / Mud Crab): long tentacle reach — strikes from noticeably
            # farther than its base range suggests.
            if u.sigil == "grasp":
                reach += 55
            # ── siege (Eagle's Nest): instead of lobbing a mortar orb, it LAUNCHES
            #    an eagle that flies out over the page, drops a bomb on its target,
            #    then wings back home. The eagle has its own HP: enemy anti-air /
            #    flyers can shoot it down and force an early retreat (airspace
            #    denial), and it can't launch another until the last one is home. ──
            if u.sigil == "siege":
                if dist <= reach and u.cool <= 0 and getattr(u, "eagle_out", None) is None:
                    u.cool = 2.6
                    u.fire_flash = 0.25
                    u.eagle_out = self._launch_eagle(u, tgt)
                    for _ in range(5):
                        self.parts.append(Particle(u.x, u.y - 20, (210, 190, 150), 60, 0.4, 3))
                continue
            # artillery (Ink Obelisk): a stationary siege piece that lobs shells the
            # length of the page — an arcing splash projectile, never melees.
            if u.sigil == "artillery":
                if dist <= u.rng and u.cool <= 0:
                    u.cool = 2.8
                    u.fire_flash = 0.25
                    self.projs.append([u.x, u.y - 30, tgt, u.dmg, "mortar"])
                    for _ in range(4):
                        self.parts.append(Particle(u.x, u.y - 20, (150, 130, 170), 60, 0.4, 3))
                continue
            # snipe (Heron): very long range, slow, heavy single bolts — it holds
            # its distance and fires rather than closing to melee.
            if u.sigil == "snipe":
                if dist <= u.rng:
                    if u.cool <= 0:
                        u.cool = 1.8
                        u.fire_flash = 0.25
                        self.projs.append([u.x, u.y - 24, tgt, u.dmg, "bolt"])
                    continue           # in range: stand and shoot, don't advance
            if dist <= reach:
                u.run_t = 0.0    # charge resets when it stops to fight
                u.rolling = 0.0  # roll momentum resets when it stops to fight
                if u.cool <= 0:
                    dmg = u.dmg
                    u.fire_flash = 0.25            # brief firing/recoil animation
                    if u.pounce: dmg *= 3; u.pounce = False
                    if u.sigil == "frenzy" and u.hp < u.maxhp / 2: u.cool = 0.45
                    else: u.cool = 0.9
                    if u.sigil == "towerbane": dmg *= 1.6
                    if u.sigil == "doublestrike": dmg *= 2      # two folds, one swing
                    # PHASE 1 NEW SIGILS:
                    if u.sigil == "stealth" and u.stealthed:
                        dmg *= 1.5  # 50% damage boost on first hit
                        u.stealthed = False  # reveal after attack
                    if u.sigil == "swiftdive":
                        nearby = [x for x in self.units if x.team == u.team and 
                                  abs(x.x - u.x) < 80 and x != u and x.alive]
                        if nearby:
                            dmg *= (1.0 + 0.3 * len(nearby))  # +30% per ally
                    # PHASE 2 NEW SIGILS:
                    if u.sigil == "burrow" and u.burrowed:
                        u.burrowed = False   # surfaces to strike, now targetable
                        u.stealthed = False
                        self.texts.append(FloatText(u.x, u.y - 24, "surfaces!", (150, 122, 40)))
                    hitlist = [tgt]
                    if u.sigil == "plague":                     # swarm hits an area
                        hitlist = [f for f in foes if math.hypot(f.x - u.x, f.y - u.y) <= reach]
                    if u.sigil == "knock":  # knockback whole lane
                        for target in foes:
                            if abs(target.y - u.y) < 100:
                                target.x += 100 * (1 if target.x > u.x else -1)
                                target.knock_timer = 0.5
                                self.texts.append(FloatText(target.x, target.y - 50, "KNOCK!", (100, 180, 220)))
                    # slam (Silverback): a heavy hit hurls the struck foe back and
                    # staggers it — single-target knockback, unlike knock's whole lane.
                    if u.sigil == "slam" and isinstance(tgt, Unit) and not getattr(tgt, "anchored", False):
                        dxk = tgt.x - u.x; dk = math.hypot(dxk, tgt.y - u.y) or 1
                        tgt.x += (dxk / dk) * 120
                        tgt.y += ((tgt.y - u.y) / dk) * 40
                        tgt.knock_timer = 0.6
                        tgt.stunned = max(tgt.stunned, 0.4)
                        self.texts.append(FloatText(tgt.x, tgt.y - 46, "SLAM!", (210, 180, 90)))
                        for _ in range(8):
                            self.parts.append(Particle(tgt.x, tgt.y, (200, 180, 120), 90, 0.5, 3))
                    for h in hitlist:
                        hd = dmg
                        # kindle: burns hotter as friendly units have fallen this match
                        if u.sigil == "kindle":
                            hd *= (1.0 + 0.06 * min(10, getattr(self, "fallen", {}).get(u.team, 0)))
                        if isinstance(h, Unit) and h.hits_ignored > 0:
                            h.hits_ignored -= 1; hd = 0
                            self.texts.append(FloatText(h.x, h.y - 24, "plated!", GOLD))
                        # bulwark: halve damage coming from the front (toward its facing)
                        if isinstance(h, Unit) and h.sigil == "bulwark":
                            frontal = (u.y - h.y) * (1 if h.team == 0 else -1) < 0
                            if frontal: hd *= 0.5
                        # phalanx (Legion): tougher when shoulder-to-shoulder — reduce
                        # incoming damage for each nearby ally.
                        if isinstance(h, Unit) and h.sigil == "phalanx":
                            allies = sum(1 for a in self.units if a.team == h.team and a is not h
                                         and a.alive and math.hypot(a.x - h.x, a.y - h.y) < 70)
                            hd *= max(0.5, 1.0 - 0.12 * min(4, allies))
                        if isinstance(h, Unit) and h.sigil == "relentless" and h.hp - hd <= 0 and not getattr(h, "_spent", False):
                            h._spent = True; hd = h.hp - 1
                            self.texts.append(FloatText(h.x, h.y - 24, "refuses!", GOLD))
                        h.hp -= hd
                        # siphon (Leech): heals itself when it damages a TOWER
                        if u.sigil == "siphon" and isinstance(h, Tower) and hd > 0:
                            u.hp = min(u.maxhp, u.hp + hd * 0.6)
                            self.texts.append(FloatText(u.x, u.y - 30, "siphon", (120, 190, 120)))
                        if h is tgt and hd:
                            self.texts.append(FloatText(h.x, h.y - 24, str(int(hd)), (232, 220, 190)))
                        if u.sigil == "toxic" and isinstance(h, Unit):
                            h.dot = min(h.dot + 24, 60)
                        # snare: the silk weaver slows whatever it hits
                        if u.sigil == "snare" and isinstance(h, Unit):
                            h.snared = 1.4
                            self.texts.append(FloatText(h.x, h.y - 36, "snared!", (190, 160, 200)))
                        # PHASE 1 NEW SIGILS:
                        if u.sigil == "inksplash" and hd > 0:  # inksplash: create puddle
                            self.ink_puddles.append(dict(x=h.x, y=h.y, timer=3.0, team=u.team))
                        if u.sigil == "freeze" and isinstance(h, Unit) and hd > 0:  # freeze: stun
                            h.stunned = 1.5
                            self.texts.append(FloatText(h.x, h.y - 50, "FREEZE!", (150, 200, 240)))
                        if u.sigil == "steal" and hd > 0:  # steal: mana drain
                            self.ai_ink = max(0, self.ai_ink - 2)
                            self.texts.append(FloatText(u.x, u.y - 50, "STEAL!", (100, 20, 100)))
                        for _ in range(4):
                            self.parts.append(Particle(h.x, h.y, INK, 80, 0.45, 2))
                        if h.hp <= 0:
                            self._on_death(h)
                            # rampage: every kill permanently ups this unit's damage
                            if u.sigil == "rampage":
                                u.rage += 1
                                u.dmg = int(CARDS[u.key]["dmg"] * (1 + 0.25 * u.rage))
                                self.texts.append(FloatText(u.x, u.y - 30, "RAMPAGE!", (200, 90, 60)))
                    # echo (Bell Frog): the strike rings a second time for a soft
                    # follow-up hit on the same target.
                    if u.sigil == "echo" and isinstance(tgt, Unit) and tgt.alive:
                        eh = dmg * 0.4
                        tgt.hp -= eh
                        self.texts.append(FloatText(tgt.x, tgt.y - 40, "echo", (150, 190, 210)))
                        if tgt.hp <= 0:
                            self._on_death(tgt)
                    # tidal: the leviathan's strike shoves everything near it aside
                    if u.sigil == "tidal":
                        for f in foes:
                            dd = math.hypot(f.x - u.x, f.y - u.y)
                            if 0 < dd < 90 and isinstance(f, Unit) and not getattr(f, "anchored", False):
                                f.x += (f.x - u.x) / dd * 40
                                f.y += (f.y - u.y) / dd * 20
                    if u.sigil == "sharp" and isinstance(tgt, Unit) and dist <= tgt.rng + u.size:
                        u.hp -= tgt.dmg * 0.3
            else:
                gx, gy = tgt.x, tgt.y
                same_side = (u.y > RIVER_Y) == (gy > RIVER_Y)
                can_swim = u.flying or u.riverhop
                if not same_side and not can_swim:
                    # must cross via a bridge: steer onto the nearest bridge lane first
                    bx = min(BRIDGE_XS, key=lambda b: abs(b - u.x))
                    if abs(u.x - bx) > BRIDGE_HALF - 4:
                        # not aligned yet — walk to the bank and slide toward the bridge x
                        bank = RIVER_Y - (RIVER_H / 2 + u.size) if u.y < RIVER_Y else RIVER_Y + (RIVER_H / 2 + u.size)
                        gx, gy = bx, bank
                    else:
                        gx = bx          # aligned: walk straight across the ribbon
                ox, oy = u.x, u.y
                spd = u.spd
                # PHASE 1: Handle stun/freeze mechanic
                if u.stunned > 0:
                    u.stunned -= DT
                    spd = 0  # Can't move while stunned
                if u.sigil == "rush" and dist > 160: spd *= 1.5   # charges across open ground
                # PHASE 2: charge ramps up speed the longer it runs uninterrupted
                if u.sigil == "charge":
                    u.run_t = min(2.5, u.run_t + DT)
                    spd *= (1.0 + 0.5 * (u.run_t / 2.5))          # up to +50%
                # PHASE 2: snare slows the target for its duration
                if u.snared > 0:
                    u.snared -= DT
                    spd *= 0.45
                # PHASE 3: rampage keeps momentum; roll bowls faster the longer it goes
                if u.sigil == "roll":
                    u.rolling = min(2.0, u.rolling + DT)
                    spd *= (1.0 + 0.8 * (u.rolling / 2.0))         # up to +80%
                # PHASE 3: blink — once, when it first meets the enemy front line,
                # teleport a chunk forward past it
                if u.sigil == "blink" and not u.blinked and dist < 220 and dist > reach:
                    u.blinked = True
                    step = -80 if u.team == 0 else 80
                    u.y += step
                    for _ in range(10):
                        self.parts.append(Particle(u.x, u.y, (150, 230, 190), 90, 0.5, 3))
                    self.texts.append(FloatText(u.x, u.y - 24, "blink!", (150, 230, 190)))
                # PHASE 3: leap — hop the river in one bound instead of using a bridge
                if u.sigil == "leap" and not u.leaped and not can_swim:
                    crossing = (u.y > RIVER_Y) != (gy > RIVER_Y)
                    if crossing and abs(u.y - RIVER_Y) < 80:
                        u.leaped = True
                        u.y = RIVER_Y + (RIVER_H if u.team == 1 else -RIVER_H) * -1 \
                              if False else (RIVER_Y - RIVER_H - u.size if u.team == 1 else RIVER_Y + RIVER_H + u.size)
                        # land on the far bank toward the target
                        u.y = (RIVER_Y + RIVER_H + u.size) if gy > RIVER_Y else (RIVER_Y - RIVER_H - u.size)
                        self.texts.append(FloatText(u.x, u.y - 24, "leap!", (190, 160, 120)))
                d = math.hypot(gx - u.x, gy - u.y) or 1
                u.x += (gx - u.x) / d * spd * DT
                u.y += (gy - u.y) / d * spd * DT
                u.face = 1 if gx >= u.x else -1
                # molten (Ember Fox): drips a burning trail that scorches foes who
                # walk through it (a short-lived damaging ember at its feet).
                if u.sigil == "molten":
                    u.molten_t += DT
                    if u.molten_t >= 0.25:
                        u.molten_t = 0.0
                        self.ink_puddles.append(dict(x=u.x, y=u.y, timer=2.0,
                                                     team=u.team, molten=True, dmg=u.dmg * 0.15))
                        for _ in range(2):
                            self.parts.append(Particle(u.x, u.y, (240, 140, 50), 40, 0.4, 2))
                # hard barrier: no ground unit may sit in open water off a bridge
                if not can_swim and abs(u.y - RIVER_Y) < RIVER_H / 2 + u.size * 0.5:
                    on_bridge = any(abs(u.x - b) < BRIDGE_HALF for b in BRIDGE_XS)
                    if not on_bridge:
                        u.y = oy if abs(oy - RIVER_Y) >= RIVER_H / 2 + u.size * 0.5 else \
                              (RIVER_Y - (RIVER_H / 2 + u.size * 0.5) if oy < RIVER_Y
                               else RIVER_Y + (RIVER_H / 2 + u.size * 0.5))

        # ---- passive auras (trees & chill units)
        for src in self.units:
            if not src.alive: continue
            if src.sigil == "sap":
                for au in self.units:
                    if au is not src and au.team == src.team and au.alive and au.hp < au.maxhp \
                       and math.hypot(au.x - src.x, au.y - src.y) < 130:
                        au.hp = min(au.maxhp, au.hp + au.maxhp * 0.06 * DT)
            if src.sigil == "thorns":       # monkey-puzzle: allied melee attackers get retaliation
                for foe in self.units:
                    if foe.team != src.team and foe.alive and math.hypot(foe.x - src.x, foe.y - src.y) < 120:
                        foe.hp -= 6 * DT     # standing near the spiked tree hurts foes
            if src.sigil == "radiance":     # sun bird: heals allies and tops up armor
                for au in self.units:
                    if au.team == src.team and au.alive and math.hypot(au.x - src.x, au.y - src.y) < 140:
                        au.hp = min(au.maxhp, au.hp + au.maxhp * 0.05 * DT)
                        if au.hits_ignored < 1 and random.random() < 0.02:
                            au.hits_ignored = 1     # a shimmering shield charge
        # chill slow: nearby enemies of a chill unit attack slower (applied as cooldown drag)
        chillers = [u for u in self.units if u.alive and u.sigil == "chill"]
        for u in self.units:
            if u.alive and chillers and any(u.team != c.team and math.hypot(u.x-c.x, u.y-c.y) < 150 for c in chillers):
                u.cool = min(u.cool + 0.4 * DT, 1.4)
        
        # PHASE 1 NEW AURAS:
        # Charm: enemies forget targets and stand idle
        charmers = [u for u in self.units if u.alive and u.sigil == "charm"]
        for charmer in charmers:
            for enemy in self.units:
                if enemy.team != charmer.team and enemy.alive and \
                   math.hypot(enemy.x - charmer.x, enemy.y - charmer.y) < 150:
                    enemy.target = None  # Forget target
                    enemy.spd = 0  # Stop moving
        
        # Forge: nearby allies get +30% damage and +20% armor
        smiths = [u for u in self.units if u.alive and u.sigil == "forge"]
        for smith in smiths:
            for ally in self.units:
                if ally.team == smith.team and ally != smith and ally.alive and \
                   math.hypot(ally.x - smith.x, ally.y - smith.y) < 140:
                    ally.dmg *= 1.3  # +30% damage
                    ally.forge_armor = 0.8  # 20% damage reduction
        
        # Update ink puddles (inksplash mechanic)
        for puddle in self.ink_puddles[:]:
            puddle["timer"] -= DT
            pdmg = puddle.get("dmg", 5) if puddle.get("molten") else 5
            for unit in self.units:
                if unit.team != puddle["team"] and unit.alive and \
                   math.hypot(unit.x - puddle["x"], unit.y - puddle["y"]) < 40:
                    unit.hp -= pdmg * DT
                    if puddle.get("molten") and random.random() < 0.2:
                        self.parts.append(Particle(unit.x, unit.y, (240, 140, 50), 30, 0.3, 2))
            if puddle["timer"] <= 0:
                self.ink_puddles.remove(puddle)

        for t in self.towers:
            if not t.alive: continue
            t.t = max(0, t.t - DT)
            # ── support special: Mending Chapel heals nearby allied units ──
            if getattr(t, "special", None) == "heal":
                for au in self.units:
                    if au.team == t.team and au.alive and au.hp < au.maxhp \
                       and math.hypot(au.x - t.x, au.y - t.y) < 200:
                        au.hp = min(au.maxhp, au.hp + au.maxhp * 0.05 * DT)
            # ── Wax Bastion thorns: melee attackers pressed against it get hurt ──
            if getattr(t, "special", None) == "thorn":
                for f in self.units:
                    if f.team != t.team and f.alive and math.hypot(f.x - t.x, f.y - t.y) < t.size + f.size + 14:
                        f.hp -= 14 * DT
                        if random.random() < 0.1:
                            self.parts.append(Particle(f.x, f.y, (200, 90, 70), 40, 0.3, 2))
            # ── Paper Rookery: periodically hatches a friendly unit (up to a cap) ──
            if getattr(t, "special", None) == "spawn" and t.spawn_key:
                t.spawn_t -= DT
                if t.spawn_t <= 0:
                    live = sum(1 for u in self.units if u.alive and u.team == t.team
                               and getattr(u, "from_tower", None) is t)
                    if live < t.spawn_max and t.spawn_key in CARDS:
                        # hatch on the tower's side, nudged toward the river
                        side = -1 if t.team == 0 else 1
                        sx = t.x + random.randint(-30, 30)
                        sy = t.y + side * (t.size + 18)
                        u = self.deploy(t.spawn_key, sx, sy, t.team)
                        if u is not None:
                            u.from_tower = t          # tag so we can cap per-tower
                        for _ in range(6):
                            self.parts.append(Particle(sx, sy, (220, 224, 210), 50, 0.5, 3))
                        if SOUND_OK:
                            sfx("deploy", 0.4)
                    t.spawn_t = t.spawn_cd
            if getattr(t, "weapon", "bolt") == "none":
                continue
            foes = [u for u in self.units if u.team != t.team and u.alive and not u.hooked]
            # towers defend their own half: they won't reach across the river to
            # the far bank. A little slack past the river-line lets them contest
            # units mid-crossing on a bridge, but not units sitting on the far side.
            slack = 24
            if t.team == 0:      # bottom towers: don't target far above the river
                foes = [u for u in foes if u.y >= RIVER_Y - slack]
            else:                # top towers: don't target far below the river
                foes = [u for u in foes if u.y <= RIVER_Y + slack]
            near = [u for u in foes if math.hypot(u.x - t.x, u.y - t.y) < t.rng]
            if near and t.t <= 0:
                tgt = min(near, key=lambda u: math.hypot(u.x - t.x, u.y - t.y))
                wp = getattr(t, "weapon", "bolt")
                # each projectile: [x, y, target, dmg, kind]
                if wp == "twin":
                    # two staggered bolts: fire one now, queue the second shortly
                    self.projs.append([t.x, t.y - 30, tgt, t.dmg, "bolt"])
                    self.projs.append([t.x - 10, t.y - 30, tgt, t.dmg, "bolt_delay"])
                    t.t = t.cd
                elif wp == "splash":
                    self.projs.append([t.x, t.y - 30, tgt, t.dmg, "splash"])
                    t.t = t.cd
                elif wp == "frost":
                    self.projs.append([t.x, t.y - 30, tgt, t.dmg, "frost"])
                    t.t = t.cd
                elif wp == "mortar":
                    self.projs.append([t.x, t.y - 30, tgt, t.dmg, "mortar"])
                    t.t = t.cd
                else:
                    self.projs.append([t.x, t.y - 30, tgt, t.dmg, "bolt"])
                    t.t = t.cd

        for p in self.projs[:]:
            tgt = p[2]
            if not tgt.alive: self.projs.remove(p); continue
            kind = p[4] if len(p) > 4 else "bolt"
            d = math.hypot(tgt.x - p[0], tgt.y - p[1]) or 1
            sp = (220 if kind == "mortar" else 340) * DT
            # the delayed twin bolt idles a beat before it starts tracking
            if kind == "bolt_delay":
                p[4] = "bolt"       # promote; a one-tick stagger
            if d < sp:
                dmg = p[3]
                if isinstance(tgt, Unit):
                    dmg *= tgt.forge_armor
                    if getattr(tgt, "ghost_t", 0) > 0:      # ghostly: phased, unhittable
                        dmg = 0
                        self.texts.append(FloatText(tgt.x, tgt.y - 30, "phased", (180, 220, 230)))
                if kind == "splash" or kind == "mortar":
                    # area damage around the impact point
                    radius = 70 if kind == "splash" else 90
                    for f in self.units:
                        if f.alive and f.team == tgt.team and math.hypot(f.x - tgt.x, f.y - tgt.y) < radius:
                            f.hp -= dmg * (1.0 if f is tgt else 0.6)
                    for _ in range(12):
                        self.parts.append(Particle(tgt.x, tgt.y, (230, 170, 90), 120, 0.5, 3))
                    self.texts.append(FloatText(tgt.x, tgt.y - 24, str(int(dmg)), WAX))
                else:
                    if kind == "frost" and isinstance(tgt, Unit):
                        tgt.snared = max(tgt.snared, 1.2)   # chill = slow
                        self.texts.append(FloatText(tgt.x, tgt.y - 36, "chill", (150, 200, 240)))
                    tgt.hp -= dmg
                    self.texts.append(FloatText(tgt.x, tgt.y - 24, str(int(dmg)), WAX))
                    for _ in range(6): self.parts.append(Particle(tgt.x, tgt.y, INK, 90, .4, 2))
                self.projs.remove(p)
            else:
                # remember the prior position (index 5) so the renderer can draw
                # a motion trail; pad the list first if it's a short/legacy proj
                prev = (p[0], p[1])
                p[0] += (tgt.x - p[0]) / d * sp; p[1] += (tgt.y - p[1]) / d * sp
                if len(p) > 5: p[5] = prev
                elif len(p) == 5: p.append(prev)

        self._tick_waves()          # Tidal Wave line-attacks sweep the lane
        self.units = [u for u in self.units if u.alive]
        for lst in (self.parts, self.texts):
            for e in lst: e.update()
        self.parts = [p for p in self.parts if p.life > 0]
        self.texts = [t for t in self.texts if t.life > 0]
        # advance & retire display-only dying corpses (never affects the sim)
        for dcorpse in self.dying:
            dcorpse["t"] += DT
        self.dying = [d for d in self.dying if d["t"] < d["dur"]]
        for fx in self.boss_fx: fx["t"] += DT
        self.boss_fx = [fx for fx in self.boss_fx if fx["t"] < 1.0]
        for sw in self.shockwaves: sw["t"] += DT
        self.shockwaves = [sw for sw in self.shockwaves if sw["t"] < sw["dur"]]

        self._update_environment()

        # ---- win / lose (driven through the shared battle FSM)
        p_king = next(t for t in self.towers if t.team == 0 and t.king)
        e_king = next(t for t in self.towers if t.team == 1 and t.king)
        if self.fsm is not None:
            dead = None
            if not e_king.alive: dead = "enemy"
            elif not p_king.alive: dead = "player"
            self.fsm.feed(max(0.0, p_king.hp / p_king.maxhp),
                          max(0.0, e_king.hp / e_king.maxhp),
                          forfeited=self.forfeited, dead=dead)
            self.frenzy = self.fsm.frenzy
            self.result = self.fsm.result
            if self.frenzy:
                self._arena_frenzy()
        else:
            if not e_king.alive: self.result = "win"
            elif not p_king.alive: self.result = "lose"

        # ── replay: once the match resolves, finalise + save the recording once,
        #    stamped with the exact ruleset/version this battle ran on and added
        #    as a leaf to the persistent patch tree. ──
        if self.result and self._recorder is not None and not self._replay_saved:
            self._replay_saved = True
            try:
                base_stamp = carddata.data_stamp() if CARDDATA_OK else ""
                patch_meta = getattr(carddata, "ACTIVE_PATCH", None) if CARDDATA_OK else None
                patch_hash = patch_meta.get("hash", "") if isinstance(patch_meta, dict) else ""
                gen = self._recorder.cards          # cards generated this match
                bstamp = None
                if globals().get("PATCHTREE_OK"):
                    rs = patchtree.ruleset_stamp(base_stamp, patch_hash)
                    bstamp = patchtree.battle_stamp(rs, self.match_seed, gen)
                    tree = patchtree.load_tree()
                    tree.add_battle(base_stamp, patch_hash, self.match_seed,
                                    generated_cards=gen, result=self.result)
                    patchtree.save_tree(tree)
                self._recorder.finish(self.result, self.tick,
                                      base_stamp=base_stamp, patch_hash=patch_hash,
                                      battle_stamp=bstamp)
                replaymod.save(self._recorder)
            except Exception:
                pass

    def _arena_frenzy(self):
        """Low-health mode: when a king is near death the page 'goes crazy' with
        per-arena chaos. Escalates with fsm.frenzy_since. Kept light on RNG so it
        doesn't wreck net determinism (net games seed the same, and it's driven
        by shared king HP, so both peers run identical chaos)."""
        ar = CUR_ARENA
        sev = 1.0 + min(2.0, (self.fsm.frenzy_since if self.fsm else 0) / 120.0)
        # a universal ominous pulse: the candles gutter (visual handled in render)
        if ar == "volcano":
            # lava surges faster and spits fireballs at random lanes
            self.lava = min(0.5, self.lava + DT * 0.03 * sev)
            if random.random() < 0.04 * sev:
                bx = random.choice(BRIDGE_XS)
                self.frenzy_fx.append(dict(kind="fireball", x=bx, y=BOOK.top + 40,
                                           vy=260, t=0.0))
        elif ar == "storm":
            # relentless lightning that zaps a unit — but tuned so it HARASSES
            # rather than clears the field. A rare strike doing a flat, capped
            # chunk (never a near-one-shot), skewed toward sturdier units so it
            # doesn't instantly vaporise fresh troops and stall the match.
            if random.random() < 0.018 * sev:
                self.flash = 1.0
                alive = [u for u in self.units if u.alive and u.spawn_t <= 0]
                if alive:
                    # prefer the healthiest target in absolute HP (a big beast is a
                    # lightning rod; small fresh units are mostly spared)
                    tgt = max(alive, key=lambda u: u.hp)
                    dmg = min(tgt.maxhp * 0.18, 140)      # capped: never a one-shot
                    tgt.hp -= dmg
                    self.frenzy_fx.append(dict(kind="bolt", x=tgt.x, y=tgt.y, t=0.0))
                    self.add_shockwave(tgt.x, tgt.y, mag=12, dur=0.4)
        elif ar in ("sandbar", "sunken"):
            # the tide floods hard, dragging ground units toward the spine
            self.tide = min(1.0, self.tide + DT * 0.2 * sev)
            for u in self.units:
                if u.alive and not u.flying:
                    u.y += (RIVER_Y - u.y) * 0.02 * sev * DT
        elif ar == "outcrop":
            # the cataract roars: every waterfall doubles its shove
            for u in self.units:
                if u.alive and not u.flying and u.y < RIVER_Y:
                    for fa in self.falls:
                        if abs(u.x - fa["x"]) < fa["w"] * 1.6:
                            u.y += 60 * sev * DT
        elif ar in ("dunes", "sandstorm"):
            # a whiteout: sand buries clumps, chipping stationary units
            for u in self.units:
                if u.alive and not u.flying and random.random() < 0.01 * sev:
                    u.snared = max(u.snared, 0.6)
                    u.hp -= u.maxhp * 0.02
        elif ar == "clockwork":
            # gears spin wild, flinging units harder
            for gr in self.gears:
                gr["spin"] *= 1.0 + 0.002 * sev
        elif ar == "quake":
            # constant tremors: fissures open twice as often (handled by spawn)
            if random.random() < 0.03 * sev and len(self.fissures) < 6:
                self.fissures.append(self._new_fissure())
        elif ar == "frost":
            # a deep freeze randomly locks units in ice
            for u in self.units:
                if u.alive and random.random() < 0.008 * sev:
                    u.stunned = max(u.stunned, 1.0)
        elif ar == "aurora":
            # the aurora warps space: units drift sideways as if on ice
            for u in self.units:
                if u.alive:
                    u.x += math.sin(self.time * 3 + u.y * 0.05) * 30 * sev * DT
        else:
            # generic page-quake: everything gets a small random shove
            for u in self.units:
                if u.alive and random.random() < 0.02 * sev:
                    u.x += random.uniform(-14, 14); u.y += random.uniform(-8, 8)
        # advance/cull frenzy fx
        for fx in self.frenzy_fx:
            fx["t"] += DT
            if fx.get("kind") == "fireball":
                fx["y"] += fx["vy"] * DT
                for u in self.units:
                    if u.alive and abs(u.x - fx["x"]) < 40 and abs(u.y - fx["y"]) < 40:
                        u.hp -= u.maxhp * 0.4; fx["t"] = 9
        self.frenzy_fx = [fx for fx in self.frenzy_fx
                          if fx["t"] < 1.2 and fx.get("y", 0) < BOOK.bottom + 40]

    def _update_environment(self):
        # lightning: charge a flash, then decay it
        if WEATHER == "lightning":
            self.flash = max(0.0, self.flash - DT * 2.5)
            self.flash_timer -= DT
            if self.flash_timer <= 0:
                self.flash = 1.0
                self.flash_timer = random.uniform(2.5, 6)
                sfx("thunder", 0.5)
        # volcano lava creeps in from the enemy's far edge, damaging back-row units
        if HAZARD == "lava":
            self.lava = min(0.34, self.lava + DT * 0.006)
            edge = BOOK.top + BOOK.h * self.lava
            for u in self.units:
                if u.alive and u.y < edge:
                    u.hp -= u.maxhp * 0.25 * DT
                    if random.random() < 0.3:
                        self.parts.append(Particle(u.x, u.y, (240, 120, 50), 60, 0.4, 3))
        # earthquake: fissures open, swallow units standing on them, then reseal & move
        if HAZARD == "fissures":
            for fz in self.fissures:
                fz["timer"] -= DT
                if fz["state"] == "opening":
                    fz["open"] = min(1.0, fz["open"] + DT * 1.5)
                    if fz["timer"] <= 0: fz["state"] = "open"; fz["timer"] = random.uniform(2.5, 4)
                elif fz["state"] == "open":
                    for u in self.units:
                        if u.alive and not u.flying and math.hypot(u.x - fz["x"], u.y - fz["y"]) < 34:
                            u.hp -= u.maxhp * 0.9 * DT     # falling in is near-lethal
                            if random.random() < 0.4:
                                self.parts.append(Particle(u.x, u.y, INK, 40, 0.4, 2))
                    if fz["timer"] <= 0: fz["state"] = "closing"; fz["timer"] = 2
                elif fz["state"] == "closing":
                    fz["open"] = max(0.0, fz["open"] - DT * 1.2)
                    if fz["open"] <= 0:
                        idx = self.fissures.index(fz); self.fissures[idx] = self._new_fissure()
        # dunes: quicksand pits slow ground units and slowly sink (damage) them
        if HAZARD == "quicksand":
            for qs in self.quicksand:
                for u in self.units:
                    if u.alive and not u.flying and math.hypot(u.x - qs["x"], u.y - qs["y"]) < qs["r"]:
                        u.snared = max(u.snared, 0.3)     # reuse snare slow
                        u.hp -= u.maxhp * 0.05 * DT
                        if random.random() < 0.15:
                            self.parts.append(Particle(u.x, u.y, (200, 170, 110), 30, 0.4, 2))
        # clockwork: spinning gears shove nearby ground units along their rim
        if HAZARD == "gears":
            for gr in self.gears:
                gr["ang"] = (gr["ang"] + gr["spin"] * DT * 1.4) % math.tau
                for u in self.units:
                    if not u.alive or u.flying: continue
                    dx, dy = u.x - gr["x"], u.y - gr["y"]
                    dd = math.hypot(dx, dy)
                    if dd < gr["r"] + u.size:
                        # tangential shove
                        tx, ty = -dy / (dd or 1), dx / (dd or 1)
                        u.x += tx * gr["spin"] * 60 * DT
                        u.y += ty * gr["spin"] * 60 * DT
                        u.hp -= u.maxhp * 0.03 * DT
        # sandbar: the tide rises and falls; at high tide the river widens and
        # ground units caught in the widened band get slowed (wading)
        if HAZARD == "tide":
            self.tide += self.tide_dir * DT * 0.06
            if self.tide >= 1.0: self.tide = 1.0; self.tide_dir = -1
            elif self.tide <= 0.0: self.tide = 0.0; self.tide_dir = 1
            band = RIVER_H / 2 + self.tide * 46
            for u in self.units:
                if u.alive and not u.flying and not u.riverhop and abs(u.y - RIVER_Y) < band:
                    u.snared = max(u.snared, 0.25)
        # outcrop: waterfalls push ground units down-page (toward team 0) and chip HP
        if HAZARD == "waterfalls":
            for fa in self.falls:
                for u in self.units:
                    if u.alive and not u.flying and abs(u.x - fa["x"]) < fa["w"] and u.y < RIVER_Y:
                        u.y += 40 * DT            # shoved downstream
                        u.hp -= u.maxhp * 0.02 * DT
                        if random.random() < 0.2:
                            self.parts.append(Particle(u.x, u.y, (180, 220, 235), 40, 0.4, 2))
        # ink tubs: capture by standing near; owner gains bonus ink
        if POI == "inktubs":
            for tub in self.tubs:
                near0 = [u for u in self.units if u.team == 0 and u.alive and math.hypot(u.x-tub["x"], u.y-tub["y"]) < 70]
                near1 = [u for u in self.units if u.team == 1 and u.alive and math.hypot(u.x-tub["x"], u.y-tub["y"]) < 70]
                if near0 and not near1: tub["prog"] = min(1.0, tub["prog"] + DT * 0.5); tub["owner"] = 0
                elif near1 and not near0: tub["prog"] = min(1.0, tub["prog"] + DT * 0.5); tub["owner"] = 1
                if tub["owner"] == 0 and tub["prog"] >= 1.0:
                    self.ink = min(ink_cap(self.mode), self.ink + DT * 0.7)
                elif tub["owner"] == 1 and tub["prog"] >= 1.0:
                    self.ai_ink = min(ink_cap(self.mode), self.ai_ink + DT * 0.7)

        # ── discovery: your units chip sealed card caches; break one to claim it
        for cache in self.caches:
            if cache["cracked"]:
                continue
            attackers = [u for u in self.units if u.team == 0 and u.alive
                         and math.hypot(u.x - cache["x"], u.y - cache["y"]) < 46 + u.size]
            for u in attackers:
                cache["hp"] -= u.dmg * DT * 1.6
            if cache["hp"] <= 0:
                cache["cracked"] = True
                self._claim_cache(cache)

        # ── discovery: cards occasionally drift in at the desk edges to tap
        if COLLECTION_OK and not self.result:
            self.table_timer -= DT
            if self.table_timer <= 0 and len(self.table_finds) < 2:
                self.table_timer = random.uniform(14, 26)
                self._spawn_table_find()
            for tf in self.table_finds:
                tf["life"] -= DT
                tf["y"] += math.sin(self.time * 1.5 + tf["phase"]) * 6 * DT
            self.table_finds = [tf for tf in self.table_finds if tf["life"] > 0]

    def _claim_cache(self, cache):
        for _ in range(24):
            self.parts.append(Particle(cache["x"], cache["y"], WAX, 150, 0.7, 3))
        key = cache["key"]
        vk = carddata.make_variant(key, cache["variant"]) if CARDDATA_OK else key
        grant = PBCOLLECTION.grant(vk, how="cache")
        self.found_cards.append(grant)
        sfx("coin")
        nm = CARDS.get(vk, {}).get("name", key)
        self.texts.append(FloatText(cache["x"], cache["y"] - 30, "found: " + nm, GOLD))

    def _spawn_table_find(self):
        pool = [k for k in ALL_CARDS if not unlocked(k)] or ALL_CARDS
        key = random.choice(pool)
        var = carddata.roll_variant() if CARDDATA_OK else "standard"
        side = random.choice((-1, 1))
        self.table_finds.append(dict(
            x=(60 if side < 0 else W - 60),
            y=random.uniform(H * 0.35, H * 0.75),
            key=key, variant=var, life=14.0, phase=random.uniform(0, 6)))

    def collect_table_find(self, tf):
        vk = carddata.make_variant(tf["key"], tf["variant"]) if CARDDATA_OK else tf["key"]
        grant = PBCOLLECTION.grant(vk, how="table")
        self.found_cards.append(grant)
        sfx("coin")
        if tf in self.table_finds:
            self.table_finds.remove(tf)

# ---------------------------------------------------------------- worn page texture
def make_page_texture():
    """logical-space page with grain, grime, stains, tears, dog-ear; alpha holes = tears"""
    rng = random.Random(7)
    LT = pygame.Surface((BOOK.w, BOOK.h), pygame.SRCALPHA)
    LT.fill(PAGE)
    # paper grain: soft mottling
    for _ in range(2600):
        x, y = rng.randrange(BOOK.w), rng.randrange(BOOK.h)
        f = rng.uniform(0.93, 1.07)
        pygame.draw.line(LT, shade(PAGE, f), (x, y), (x + rng.randint(1, 4), y), 1)
    # ruled lines + faint red margin
    for yy in range(24, BOOK.h, 26):
        pygame.draw.line(LT, PAGE_LINE, (14, yy), (BOOK.w - 14, yy), 1)
    pygame.draw.line(LT, (172, 92, 84, 90), (30, 10), (30, BOOK.h - 10), 1)
    # foxing freckles
    for _ in range(150):
        x, y = rng.randrange(BOOK.w), rng.randrange(BOOK.h)
        r = rng.randint(1, 3)
        pygame.draw.circle(LT, (150, 116, 74, rng.randint(30, 70)), (x, y), r)
    # big faint blotches
    for (bx, by, bw, bh, a) in ((60, 80, 200, 150, 16), (BOOK.w - 220, BOOK.h - 200, 240, 170, 14),
                                (BOOK.w - 180, 60, 160, 120, 12)):
        blob = pygame.Surface((bw, bh), pygame.SRCALPHA)
        pygame.draw.ellipse(blob, (110, 86, 52, a), blob.get_rect())
        LT.blit(blob, (bx, by))
    # coffee/ink ring stains
    for (cx, cy, r) in ((110, BOOK.h - 130, 42), (BOOK.w - 120, 150, 34)):
        for i in range(90):
            a = rng.uniform(0, math.tau)
            rr = r + rng.uniform(-2, 2)
            pygame.draw.circle(LT, (96, 70, 40, rng.randint(20, 60)),
                               (int(cx + math.cos(a) * rr), int(cy + math.sin(a) * rr)), 2)
    # ink spatter near the river
    ry = RIVER_Y - BOOK.top
    for _ in range(70):
        x = rng.randrange(BOOK.w)
        y = ry + rng.randint(-70, 70)
        pygame.draw.circle(LT, (20, 18, 26, rng.randint(60, 150)), (x, y), rng.randint(1, 3))
    # old creases: light/dark line pairs
    for (x0, y0, x1, y1) in ((40, 200, BOOK.w - 60, 260), (80, BOOK.h - 90, BOOK.w - 40, BOOK.h - 180)):
        pygame.draw.line(LT, shade(PAGE, 1.15), (x0, y0), (x1, y1), 1)
        pygame.draw.line(LT, shade(PAGE, 0.78), (x0, y0 + 2), (x1, y1 + 2), 1)
    # scuffed grime along edges
    for _ in range(240):
        side = rng.choice("tblr")
        if side == "t": x, y = rng.randrange(BOOK.w), rng.randint(0, 26)
        elif side == "b": x, y = rng.randrange(BOOK.w), BOOK.h - rng.randint(1, 26)
        elif side == "l": x, y = rng.randint(0, 26), rng.randrange(BOOK.h)
        else: x, y = BOOK.w - rng.randint(1, 26), rng.randrange(BOOK.h)
        pygame.draw.circle(LT, (92, 74, 50, rng.randint(14, 40)), (x, y), rng.randint(2, 5))
    # the river bed baked in (rough shoreline)
    rr = pygame.Rect(0, ry - RIVER_H // 2, BOOK.w, RIVER_H)
    pygame.draw.rect(LT, RIVER, rr)
    for x in range(0, BOOK.w, 7):
        j1 = rng.randint(-4, 4); j2 = rng.randint(-4, 4)
        pygame.draw.line(LT, shade(RIVER, 1.4), (x, rr.top + j1), (x + 7, rr.top + j1), 2)
        pygame.draw.line(LT, shade(RIVER, 1.4), (x, rr.bottom + j2), (x + 7, rr.bottom + j2), 2)
        pygame.draw.line(LT, (60, 48, 34, 120), (x, rr.top - 3 + j1), (x + 7, rr.top - 3 + j1), 2)
    # torn edges: transparent notches (draw replaces pixels on SRCALPHA)
    def notch(px, py, ang, ln, wd):
        ax = px + math.cos(ang) * ln; ay = py + math.sin(ang) * ln
        pygame.draw.polygon(LT, (0, 0, 0, 0),
            [(px - wd, py), (px + wd, py), (ax, ay)] if abs(math.sin(ang)) > 0.5
            else [(px, py - wd), (px, py + wd), (ax, ay)])
    for _ in range(26):
        e = rng.choice("tblr")
        if e == "t": notch(rng.randrange(BOOK.w), 0, math.pi / 2, rng.randint(4, 14), rng.randint(3, 9))
        elif e == "b": notch(rng.randrange(BOOK.w), BOOK.h, -math.pi / 2, rng.randint(4, 14), rng.randint(3, 9))
        elif e == "l": notch(0, rng.randrange(BOOK.h), 0, rng.randint(4, 12), rng.randint(3, 9))
        else: notch(BOOK.w, rng.randrange(BOOK.h), math.pi, rng.randint(4, 12), rng.randint(3, 9))
    # one honest tear near bottom-left + deckle shadow
    pygame.draw.polygon(LT, (0, 0, 0, 0),
        [(0, BOOK.h - 60), (46, BOOK.h - 44), (70, BOOK.h - 12), (30, BOOK.h), (0, BOOK.h)])
    pygame.draw.lines(LT, (120, 98, 66, 160), False,
        [(0, BOOK.h - 58), (44, BOOK.h - 43), (68, BOOK.h - 12), (30, BOOK.h - 2)], 2)
    # dog-eared corner top-right: carve + folded flap
    pygame.draw.polygon(LT, (0, 0, 0, 0), [(BOOK.w - 64, 0), (BOOK.w, 0), (BOOK.w, 64)])
    pygame.draw.polygon(LT, shade(PAGE, 0.8), [(BOOK.w - 64, 0), (BOOK.w - 64, 44), (BOOK.w - 20, 64), (BOOK.w, 64)])
    pygame.draw.polygon(LT, shade(PAGE, 0.62), [(BOOK.w - 64, 0), (BOOK.w, 64), (BOOK.w - 64, 44)])
    pygame.draw.line(LT, shade(PAGE, 1.2), (BOOK.w - 64, 0), (BOOK.w, 64), 2)

    # ── photorealistic extras: subtle organic fibers, soft ink bleed, deckle ──
    if gfx_hi():
        river_top = ry - RIVER_H // 2 - 6
        river_bot = ry + RIVER_H // 2 + 6
        def _on_page(yy):
            return not (river_top <= yy <= river_bot)   # keep the river clean
        # soft, irregular paper fibers — short, low-contrast, never a grid
        for _ in range(700):
            x, y = rng.randrange(BOOK.w), rng.randrange(BOOK.h)
            if not _on_page(y):
                continue
            ang = rng.uniform(0, math.tau); ln = rng.randint(4, 14)
            f = rng.uniform(0.95, 1.05)
            col = shade(PAGE, f)
            pygame.draw.line(LT, (col[0], col[1], col[2]),
                             (x, y), (x + math.cos(ang) * ln, y + math.sin(ang) * ln), 1)
        # gentle broad tonal mottling (large soft blobs, not lines) for depth
        for _ in range(30):
            x, y = rng.randrange(BOOK.w), rng.randrange(BOOK.h)
            if not _on_page(y):
                continue
            rad = rng.randint(20, 60)
            blob = pygame.Surface((rad * 2, rad * 2), pygame.SRCALPHA)
            a = rng.randint(6, 14)
            tone = shade(PAGE, rng.uniform(0.9, 1.06))
            pygame.draw.circle(blob, (tone[0], tone[1], tone[2], a), (rad, rad), rad)
            LT.blit(blob, (x - rad, y - rad))
        # ink bleed feathering gently into the paper from the river banks only
        for _ in range(160):
            x = rng.randrange(BOOK.w)
            side = rng.choice((-1, 1))
            base = ry + side * (RIVER_H // 2)
            yy = base + side * rng.randint(2, 14)
            a = max(0, 46 - abs(yy - base) * 3)
            pygame.draw.circle(LT, (24, 20, 30, a), (x, yy), 1)
        # soft deckle shadow inset around the page edge (handmade-paper look)
        edge = pygame.Surface((BOOK.w, BOOK.h), pygame.SRCALPHA)
        for i in range(8):
            a = 18 - i * 2
            if a <= 0: break
            pygame.draw.rect(edge, (60, 46, 30, a), (i, i, BOOK.w - 2 * i, BOOK.h - 2 * i),
                             width=1, border_radius=8)
        LT.blit(edge, (0, 0))
    return LT

def build_board_static():
    """bake desk, cover, page stack, strip-projected worn page, bridges, desk props"""
    S = pygame.Surface((W, H))
    S.fill(DESK)
    rng = random.Random(3)
    for i in range(0, W, 90):
        pygame.draw.line(S, DESK_GRAIN, (i, 0), (i + 40, H), 3)
        pygame.draw.line(S, shade(DESK, 1.12), (i + 22, 0), (i + 62, H), 1)
        # knots / scratches so the desk shares the wear language
        if rng.random() < 0.5:
            kx, ky = i + rng.randint(0, 80), rng.randint(0, H)
            pygame.draw.ellipse(S, DESK_GRAIN, (kx, ky, rng.randint(8, 22), rng.randint(4, 9)), 2)
    for _ in range(60):
        x, y = rng.randrange(W), rng.randrange(H)
        pygame.draw.line(S, shade(DESK, rng.uniform(0.75, 1.2)),
                         (x, y), (x + rng.randint(6, 40), y + rng.randint(-3, 3)), 1)

    cover_q = quad(BOOK.left - 26, BOOK.top - 18, BOOK.right + 26, BOOK.bottom + 22)
    bs = pygame.Surface((W, H), pygame.SRCALPHA)
    pygame.draw.polygon(bs, (5, 3, 2, 150), [(p[0] - 26, p[1] + 20) for p in cover_q])
    S.blit(bs, (0, 0))
    # leather cover: base + directional edge light + scuffs
    pygame.draw.polygon(S, COVER, cover_q)
    for i in range(4):
        a, b = cover_q[i], cover_q[(i + 1) % 4]
        mx, my = (a[0] + b[0]) / 2, (a[1] + b[1]) / 2
        ex, ey = b[0] - a[0], b[1] - a[1]
        nx, ny = ey, -ex
        ln = math.hypot(nx, ny) or 1
        lam = light_lambert(mx, my, nx / ln, ny / ln, boost=1.1, floor=0.5)
        pygame.draw.line(S, shade(COVER, lam * 1.6), a, b, 3)
    for _ in range(90):
        p = rng.random()
        e0, e1 = cover_q[0], cover_q[1]
        x = lerp(e0[0], e1[0], p) + rng.randint(-6, 6)
        y = lerp(e0[1], e1[1], p) + rng.randint(2, 26)
        pygame.draw.line(S, shade(COVER, rng.uniform(1.1, 1.5)), (x, y), (x + rng.randint(2, 9), y + rng.randint(-2, 2)), 1)
    for i in range(4):
        pq = quad(BOOK.left - 14 + i * 3, BOOK.top - 10 + i * 2,
                  BOOK.right + 14 - i * 3, BOOK.bottom + 14 - i * 3)
        pygame.draw.polygon(S, shade(PAGE, 0.5 + i * 0.08), pq)
        pygame.draw.line(S, shade(PAGE, 0.42), pq[3], pq[2], 1)   # stacked-edge line

    # strip-project the worn page texture onto the trapezoid (DISPLAY extents)
    LT = make_page_texture()
    ltw, lth = LT.get_size()
    for sy in range(int(DISP_TOP_Y), int(DISP_BOT_Y) + 1):
        k = (sy - DISP_TOP_Y) / max(1e-6, (DISP_BOT_Y - DISP_TOP_Y))
        t = (-0.66 + math.sqrt(0.66 * 0.66 + 4 * 0.34 * k)) / (2 * 0.34)
        ly = int(min(lth - 1, max(0, t * lth)))
        s = lerp(S_FAR, S_NEAR, t)
        wpx = max(1, int(DISP_BOOK.w * s))
        row = LT.subsurface((0, ly, ltw, 1))
        S.blit(pygame.transform.scale(row, (wpx, 1)), (int(DISP_CX - wpx / 2), sy))

    # gutter curl shading (baked)
    gs = pygame.Surface((W, H), pygame.SRCALPHA)
    for i in range(16):
        a = max(0, int(46 - i * 2.6))
        for sgn in (-1, 1):
            yy = RIVER_Y + sgn * (RIVER_H // 2 + 20 - i)
            p1 = proj(BOOK.left, yy); p2 = proj(BOOK.right, yy)
            pygame.draw.line(gs, (30, 20, 10, a), p1[:2], p2[:2], 2)
    S.blit(gs, (0, 0))

    # ribbon bridges as shallow prisms with shadow
    for bx in BRIDGE_XS:
        bq = quad(bx - 34, RIVER_Y - RIVER_H // 2 - 8, bx + 34, RIVER_Y + RIVER_H // 2 + 8)
        bsh = pygame.Surface((W, H), pygame.SRCALPHA)
        pygame.draw.polygon(bsh, (5, 3, 2, 130), [(p[0] + 5, p[1] + 6) for p in bq])
        S.blit(bsh, (0, 0))
        draw_prism(S, bq, 6, RIBBON, top_detail=True)
        tail_y = RIVER_Y + RIVER_H // 2 + 8
        t1 = proj(bx - 34, tail_y); t2 = proj(bx + 34, tail_y); t3 = proj(bx, tail_y + 16)
        pygame.draw.polygon(S, shade(RIBBON, 0.85), [(t1[0], t1[1] - 6), (t3[0], t3[1] - 4), (t2[0], t2[1] - 6)])
        # ribbon weave grime
        rr2 = random.Random(int(bx))
        for _ in range(26):
            px = lerp(bq[0][0], bq[1][0], rr2.random())
            py = lerp(bq[0][1], bq[3][1], rr2.random()) - 6
            pygame.draw.line(S, shade(RIBBON, rr2.uniform(0.6, 1.25)), (px, py), (px + rr2.randint(2, 6), py), 1)

    # ---- desk props sharing the light: inkwell + quill, scattered card, wax stamp
    def prop_shadow(x, y, w):
        dx, dy = x - LIGHT_MAIN[0], y - LIGHT_MAIN[1]
        d = math.hypot(dx, dy) or 1
        sh2 = pygame.Surface((W, H), pygame.SRCALPHA)
        pygame.draw.ellipse(sh2, (6, 4, 3, 120),
                            (x + dx / d * w * 0.8 - w, y + dy / d * w * 0.35 - w * 0.3, w * 2.2, w * 0.8))
        S.blit(sh2, (0, 0))
    if not ARENAS[CUR_ARENA]["candles"]:
        return S
    # Desk props are tucked into the DESK MARGINS around the reflowed page, not
    # at fixed 1280x800 pixels — otherwise on portrait/mobile they land on top of
    # the board. If a margin is too thin to hold a prop (very tall/narrow phones),
    # the prop is skipped rather than drawn over the play area.
    left_margin = DISP_BOOK.left
    right_margin = W - DISP_BOOK.right
    bottom_margin = H - DISP_BOOK.bottom

    # inkwell + quill: left desk margin, only if there's room for them
    if left_margin >= 70:
        ix = max(28, DISP_BOOK.left - int(left_margin * 0.5))
        iy = int(DISP_BOOK.top + DISP_BOOK.h * 0.42)
        prop_shadow(ix, iy + 26, 34)
        well = [(ix - 26, iy + 26), (ix - 20, iy - 18), (ix + 20, iy - 18), (ix + 26, iy + 26)]
        draw_prism(S, well, 10, (34, 30, 40))
        pygame.draw.ellipse(S, INK, (ix - 15, iy - 32, 30, 12))
        pygame.draw.ellipse(S, (96, 108, 148), (ix - 15, iy - 32, 30, 12), 1)
        pygame.draw.circle(S, (168, 178, 210), (ix + 8, iy - 28), 2)   # glass glint
        # quill leaning out of the well (lean toward the board so it stays clear)
        pygame.draw.line(S, (210, 202, 184), (ix + 4, iy - 28), (ix + 62, iy - 96), 3)
        pygame.draw.polygon(S, (222, 216, 200),
            [(ix + 26, iy - 52), (ix + 76, iy - 108), (ix + 56, iy - 106), (ix + 20, iy - 44)])
        pygame.draw.polygon(S, shade((222, 216, 200), 0.7),
            [(ix + 26, iy - 52), (ix + 56, iy - 106), (ix + 40, iy - 100), (ix + 20, iy - 44)])

    # scattered spare card: bottom-left desk corner, only if there's a bottom band
    if bottom_margin >= 70 and left_margin >= 50:
        scx, scy, ang = max(40, DISP_BOOK.left - int(left_margin * 0.4)), \
                        int(DISP_BOOK.bottom + bottom_margin * 0.45), -0.28
        ca, sa = math.cos(ang), math.sin(ang)
        cpts = [(scx + (-34 * ca - -46 * sa), scy + (-34 * sa + -46 * ca)),
                (scx + (34 * ca - -46 * sa), scy + (34 * sa + -46 * ca)),
                (scx + (34 * ca - 46 * sa), scy + (34 * sa + 46 * ca)),
                (scx + (-34 * ca - 46 * sa), scy + (-34 * sa + 46 * ca))]
        prop_shadow(scx, scy + 20, 40)
        draw_prism(S, cpts, 3, shade(PAGE, 0.9), top_detail=False)
        pygame.draw.circle(S, shade(WAX, 0.9), (scx + 8, scy + 12), 8)
        pygame.draw.circle(S, shade(WAX, 0.55), (scx + 8, scy + 12), 8, 2)
        for _ in range(16):   # grubby card face
            px, py = scx + rng.randint(-30, 30), scy + rng.randint(-40, 36)
            pygame.draw.circle(S, shade(PAGE, rng.uniform(0.72, 0.9)), (px, py - 3), rng.randint(1, 2))

    # wax seal stamp: right desk margin, only if there's room
    if right_margin >= 60:
        wx = min(W - 24, DISP_BOOK.right + int(right_margin * 0.5))
        wy = int(DISP_BOOK.top + DISP_BOOK.h * 0.40)
        prop_shadow(wx, wy + 14, 26)
        pygame.draw.ellipse(S, shade(WAX, 0.8), (wx - 20, wy, 40, 16))
        draw_prism(S, [(wx - 8, wy + 6), (wx - 6, wy - 34), (wx + 6, wy - 34), (wx + 8, wy + 6)], 4, (86, 62, 40))
        pygame.draw.circle(S, (110, 80, 52), (wx, wy - 36), 7)
    return S

# ---------------------------------------------------------------- entity drawing
def cast_shadow(layer, sx, sy, s, size, light, stretch=2.4, alpha=95):
    dx, dy = sx - light[0], sy - light[1]
    d = math.hypot(dx, dy) or 1
    dx, dy = dx / d, dy / d
    flick = 1.0 + 0.07 * math.sin(pygame.time.get_ticks() * 0.013 + sx)
    L = size * s * stretch * flick
    wshort = size * s * 0.85
    px, py = -dy, dx
    def _poly(spread, a):
        w = wshort * spread
        return [(sx + px * w, sy + py * w * 0.4),
                (sx - px * w, sy - py * w * 0.4),
                (sx - px * w * 0.55 + dx * L, sy - py * w * 0.25 + dy * L * 0.45),
                (sx + px * w * 0.55 + dx * L, sy + py * w * 0.25 + dy * L * 0.45)], a
    if gfx_hi():
        # layered penumbra: four stacked polygons, wide+faint to tight+dark, so
        # the cast shadow softens with distance from the body (umbra→penumbra),
        # plus a soft radial contact-AO blob right under the feet.
        for spread, a in ((1.55, int(alpha * 0.18)),
                          (1.35, int(alpha * 0.30)),
                          (1.12, int(alpha * 0.44)),
                          (1.00, int(alpha * 0.72))):
            pts, aa = _poly(spread, a)
            pygame.draw.polygon(layer, (6, 4, 3, aa), pts)
        # soft contact ambient-occlusion: a radial-gradient blob (cached by size)
        ao = max(3, int(size * s * 0.95))
        blob = _ao_blob(ao)
        layer.blit(blob, (int(sx - ao), int(sy - ao * 0.55)))
    else:
        pts, aa = _poly(1.0, alpha)
        pygame.draw.polygon(layer, (8, 5, 4, aa), pts)

_AO_CACHE = {}
def _ao_blob(ao):
    """A cached soft radial contact-shadow blob (darkest at centre, fading out),
    keyed by radius bucket so we don't rebuild it per unit per frame."""
    key = max(4, (ao // 3) * 3)             # bucket to reduce cache churn
    blob = _AO_CACHE.get(key)
    if blob is None:
        blob = pygame.Surface((key * 2, key * 2), pygame.SRCALPHA)
        for i in range(6, 0, -1):
            rr = key * i / 6
            a = int(120 * (1 - i / 6) ** 1.4) + 18
            pygame.draw.ellipse(blob, (4, 3, 2, a),
                                (key - rr, key * 0.55 - rr * 0.55, rr * 2, rr * 1.1))
        _AO_CACHE[key] = blob
    return blob

def _origami_animal_legacy(screen, u, x, y, s, f, c, hgt, tms):
    """The original hand-tuned per-key model drawing, kept as a fallback for when
    the data-driven origami recipe engine is unavailable or errors. Preserves the
    exact look of the launch roster."""
    if u.key in ("crane", "batcolony", "mothswarm"):
        wing = math.sin(tms * 0.02 + x) * s * 0.35
        body = [(x - s * f, y), (x + s * 0.9 * f, y - s * 0.25),
                (x + s * 1.3 * f, y + s * 0.15), (x, y + s * 0.4)]
        top = draw_prism(screen, body, hgt * 0.5, c)
        w1 = [(top[0][0], top[0][1]), (x - s * 0.5 * f, y - s - wing - hgt * 0.5),
              (x + s * 0.55 * f, y - s * 0.15 - hgt * 0.5)]
        draw_poly_paper(screen, w1, shade(c, 0.9), thick=2)
        pygame.draw.polygon(screen, shade(c, 1.15),
            [(x + s * 1.3 * f, y + s * 0.15 - hgt * 0.5), (x + s * 1.8 * f, y - s * 0.1 - hgt * 0.5),
             (x + s * 1.25 * f, y - s * 0.2 - hgt * 0.5)])
    elif u.key in ("baobab", "dragontree"):
        trunk = [(x - s * 0.5, y + s * 0.5), (x - s * 0.34, y - s * 0.1),
                 (x + s * 0.34, y - s * 0.1), (x + s * 0.5, y + s * 0.5)]
        draw_prism(screen, trunk, s * 0.9, c)
        crown_col = (96, 116, 68) if u.key == "baobab" else (150, 54, 50)
        crown = [(x - s * 1.1, y - s * 0.55), (x, y - s * 1.0), (x + s * 1.1, y - s * 0.55)]
        draw_prism(screen, crown, s * 0.5, crown_col)
    else:
        body = [(x - s * f, y - s * 0.2), (x + s * 0.9 * f, y - s * 0.28),
                (x + s * 1.05 * f, y + s * 0.3), (x - s * 1.15 * f, y + s * 0.35)]
        for lx0, lx1 in ((-0.85, -0.4), (0.35, 0.8)):
            leg = [(x + s * lx0 * f, y + s * 0.28), (x + s * lx1 * f, y + s * 0.28),
                   (x + s * (lx1 - 0.06) * f, y + s * 0.62), (x + s * (lx0 + 0.06) * f, y + s * 0.62)]
            pygame.draw.polygon(screen, shade(c, 0.55), leg)
        top = draw_prism(screen, body, hgt, c)
        hx0 = x + s * 0.75 * f
        head = [(hx0, y - s * 0.5 - hgt), (hx0 + s * 0.75 * f, y - s * 0.1 - hgt),
                (hx0 + s * 0.35 * f, y + s * 0.12 - hgt)]
        draw_poly_paper(screen, head, shade(c, 1.08), thick=3)
        if u.key == "sabertooth":
            pygame.draw.polygon(screen, (232, 226, 208),
                [(hx0 + s * 0.62 * f, y - hgt + s * 0.02), (hx0 + s * 0.7 * f, y - hgt + s * 0.4),
                 (hx0 + s * 0.48 * f, y - hgt + s * 0.06)])
        if u.key == "pangolin":
            for i in range(3):
                px = x - s * 0.6 * f + i * s * 0.45 * f
                pygame.draw.polygon(screen, shade(c, 0.68),
                    [(px, y - s * 0.28 - hgt), (px + s * 0.28 * f, y - s * 0.6 - hgt),
                     (px + s * 0.5 * f, y - s * 0.28 - hgt)])
        if u.key in ("polarbear", "gorilla"):
            hump = [(x - s * 0.45 * f, y - s * 0.3 - hgt), (x, y - s * 0.68 - hgt),
                    (x + s * 0.45 * f, y - s * 0.34 - hgt)]
            draw_poly_paper(screen, hump, shade(c, 1.05), thick=2)
        if u.key == "mammoth":
            pygame.draw.line(screen, (238, 232, 214), (hx0 + s*0.5*f, y-hgt+s*0.1),
                             (hx0 + s*0.95*f, y-hgt+s*0.5), 4)
            pygame.draw.line(screen, (220, 214, 196), (hx0 + s*0.45*f, y-hgt+s*0.16),
                             (hx0 + s*0.8*f, y-hgt+s*0.56), 3)
        if u.key == "honeybadger":
            stripe = [(x - s*0.9*f, y - s*0.16 - hgt), (x + s*0.5*f, y - s*0.2 - hgt),
                      (x + s*0.5*f, y - hgt), (x - s*0.9*f, y - s*0.04 - hgt)]
            draw_poly_paper(screen, stripe, shade(c, 1.5), thick=1)
        if u.key == "cassowary":
            pygame.draw.polygon(screen, (150, 36, 42),
                [(hx0 + s*0.1*f, y - s*0.55 - hgt), (hx0 + s*0.3*f, y - s*0.85 - hgt), (hx0 + s*0.45*f, y - s*0.5 - hgt)])

def _draw_eagle(screen, u, x, y, s, f, hgt, tms):
    """A folded paper eagle: a body with flapping delta wings and a hooked beak,
    carrying a slung powder-keg while outbound (dropped once it bombs). Faces its
    travel direction; wings beat on a sine."""
    face = getattr(u, "face", 1) or 1
    beat = math.sin(tms * 0.02 + (u.uid & 0xff))          # wing flap
    body_col = shade((150, 120, 88), 1.0)
    wing_col = shade((120, 96, 68), 1.0)
    yy = y - hgt * 0.4
    # cast a faint moving ground shadow (sold the "in the air" read)
    sh = pygame.Surface((int(s * 3), int(s)), pygame.SRCALPHA)
    pygame.draw.ellipse(sh, (10, 8, 6, 70), (0, 0, int(s * 3), int(s)))
    screen.blit(sh, (x - s * 1.5, y + s * 0.6))
    # tail + body (a slim folded lozenge pointing the way it flies)
    bx = x
    body = [(bx - face * s * 1.1, yy), (bx + face * s * 0.5, yy - s * 0.35),
            (bx + face * s * 1.2, yy), (bx + face * s * 0.5, yy + s * 0.35)]
    pygame.draw.polygon(screen, body_col, body)
    pygame.draw.polygon(screen, shade(body_col, 0.7), body, 2)
    # hooked beak + eye at the leading edge
    bkx = bx + face * s * 1.2
    pygame.draw.polygon(screen, (210, 180, 90),
                        [(bkx, yy - 2), (bkx + face * s * 0.35, yy), (bkx, yy + 3)])
    pygame.draw.circle(screen, (30, 24, 20), (int(bx + face * s * 0.55), int(yy - s * 0.12)), max(1, int(s * 0.08)))
    # two delta wings that beat (near wing lower, far wing raised for depth)
    lift = beat * s * 0.7
    for sgn, wc in ((1, wing_col), (-1, shade(wing_col, 0.82))):
        tip_y = yy - abs(lift) * (1 if sgn > 0 else 0.7) - s * 0.1
        wing = [(bx - face * s * 0.2, yy),
                (bx - face * s * 0.5, tip_y - sgn * s * 0.15),
                (bx + face * s * 0.4, yy - s * 0.05)]
        pygame.draw.polygon(screen, wc, wing)
        pygame.draw.polygon(screen, shade(wc, 0.65), wing, 1)
    # slung powder-keg bomb while still outbound
    if getattr(u, "eagle_phase", None) == "out":
        kx, ky = int(bx), int(yy + s * 0.5)
        pygame.draw.circle(screen, (60, 48, 38), (kx, ky), max(2, int(s * 0.32)))
        pygame.draw.circle(screen, shade((60, 48, 38), 1.5), (kx, ky), max(2, int(s * 0.32)), 1)
        # a tiny lit fuse spark
        fx = kx + math.sin(tms * 0.03) * 2
        pygame.draw.circle(screen, (255, 200, 120), (int(fx), ky - int(s * 0.4)), 1)
    # health pip when hurt (so you can see it being driven off)
    if u.hp < u.maxhp:
        bw = s * 1.6
        ty = yy - s - 6
        pygame.draw.rect(screen, (30, 24, 20), (x - bw / 2, ty, bw, 4))
        pygame.draw.rect(screen, (210, 150, 90), (x - bw / 2, ty, bw * max(0, u.hp / u.maxhp), 4))


def _draw_echo(screen, u, x, y, s, f, hgt, tms):
    """Render a spirit-divergence echo. Two toggleable styles.

    'apparition' — fold the ORIGINAL card model into a small offscreen surface,
    then stamp it back darkened + semi-transparent with a faint drifting double,
    so it reads as a shadow of the real creature.
    'wraith' — a bespoke tattered apparition: a hovering dark shroud with a torn
    hem and two ember eyes; lighter-weight and clearly "not the real thing"."""
    style = globals().get("ECHO_STYLE", "apparition")
    drift = math.sin(tms * 0.004 + (u.uid & 0xff)) * s * 0.12
    if style == "apparition" and ORIGAMI_OK:
        try:
            pad = int(s * 3) + 12
            tmp = pygame.Surface((pad, pad), pygame.SRCALPHA)
            cx, cy = pad // 2, pad // 2
            fam = _card_family(u.key)
            card = CARDS.get(u.key)
            recipe = origami.recipe_for(u.key, fam, card)
            ghost = (40, 26, 56)                      # deep violet-black paper
            anim = {"t": tms * 0.001, "move": 0.0, "phase": (u.uid & 0xff) * 0.1,
                    "attack": 0.0, "breathe": math.sin(tms * 0.003 + u.uid)}
            for part in recipe:
                origami.draw_part(tmp, part, cx, cy, s, f, ghost, hgt * 0.6,
                                  tms * 0.001, 0.0, anim)
            # a faint drifting double behind, then the main ghost, both alpha'd
            tmp.set_alpha(70)
            screen.blit(tmp, (x - cx + drift * 1.8, y - cy - s * 0.15),
                        special_flags=pygame.BLEND_PREMULTIPLIED if False else 0)
            tmp.set_alpha(150)
            screen.blit(tmp, (x - cx, y - cy))
            # a soft violet aura ring
            aura = pygame.Surface((pad, pad), pygame.SRCALPHA)
            pygame.draw.circle(aura, (150, 90, 200, 40), (cx, cy), int(s * 0.9))
            screen.blit(aura, (x - cx, y - cy), special_flags=pygame.BLEND_ADD)
            return
        except Exception:
            pass
    # 'wraith' (or fallback): a bespoke tattered shroud
    yy = y - abs(math.sin(tms * 0.004 + u.uid)) * s * 0.25      # hover
    body = shade((46, 30, 60), 1.0)
    hood = [(x, yy - s * 1.1), (x + s * 0.7, yy - s * 0.2),
            (x + s * 0.5, yy + s * 0.5), (x - s * 0.5, yy + s * 0.5),
            (x - s * 0.7, yy - s * 0.2)]
    # tattered hem: a few torn triangles along the bottom
    hem = []
    for i in range(-3, 4):
        hem.append((x + i * s * 0.18, yy + s * 0.5))
        hem.append((x + i * s * 0.18 + s * 0.09, yy + s * (0.5 + (0.25 if i % 2 else 0.12))))
    aura = pygame.Surface((int(s * 3), int(s * 3)), pygame.SRCALPHA)
    pygame.draw.circle(aura, (150, 90, 200, 45), (int(s * 1.5), int(s * 1.5)), int(s * 0.95))
    screen.blit(aura, (x - s * 1.5, yy - s * 1.5), special_flags=pygame.BLEND_ADD)
    pygame.draw.polygon(screen, body, hood)
    pygame.draw.polygon(screen, shade(body, 0.6), hood, 2)
    if len(hem) >= 3:
        pygame.draw.polygon(screen, shade(body, 0.75), hem)
    # two ember eyes
    for ex in (-0.22, 0.22):
        pygame.draw.circle(screen, (210, 130, 90), (int(x + ex * s), int(yy - s * 0.35)), max(1, int(s * 0.08)))


def _hi_model_detail(screen, u, x, y, s, f, c, hgt):
    """A light-touch relief pass for photorealistic mode: a warm rim-light on the
    candle-facing side of the model, a cool bounce on the far side, and one or two
    fold creases across the body. All derived from the main light direction so it
    stays consistent with the scene's shading. Purely cosmetic."""
    # direction from the model to the main light (screen space, normalised)
    lx, ly = LIGHT_MAIN
    dx, dy = lx - x, ly - (y - hgt * 0.5)
    d = math.hypot(dx, dy) or 1.0
    dx, dy = dx / d, dy / d
    top = y - hgt              # top face sits a little above the base
    # rim light: a short bright arc on the lit edge of the silhouette
    rim = shade(c, 1.9)
    rw = s * 0.82
    # perpendicular to the light dir, offset toward the light
    px, py = -dy, dx
    ax, ay = x + dx * s * 0.55, top + dy * s * 0.3
    pygame.draw.line(screen, rim,
                     (ax + px * rw, ay + py * rw * 0.5),
                     (ax - px * rw * 0.4, ay - py * rw * 0.2), 2)
    # a fainter warm dab right at the crest
    hl = pygame.Surface((int(s), int(s)), pygame.SRCALPHA)
    pygame.draw.circle(hl, (255, 240, 210, 42), (int(s / 2), int(s / 2)), max(2, int(s * 0.3)))
    screen.blit(hl, (ax - s / 2, ay - s / 2), special_flags=pygame.BLEND_ADD)
    # cool ambient bounce on the shadowed side (subtractive-ish, drawn dark-cool)
    bx, by = x - dx * s * 0.5, top - dy * s * 0.25
    pygame.draw.line(screen, shade((70, 74, 96), 1.0),
                     (bx + px * rw * 0.6, by + py * rw * 0.3),
                     (bx - px * rw * 0.6, by - py * rw * 0.3), 1)
    # 1–2 fold creases across the body: a thin darker line + a bright side,
    # seeded per-unit so they sit in a stable place on each model
    crn = 2 if s >= 16 else 1
    cr = shade(c, 0.66); crl = shade(c, 1.5)
    for i in range(crn):
        off = (-0.28 + i * 0.5) * s
        cxa, cya = x - rw * 0.7, top + off
        cxb, cyb = x + rw * 0.7, top + off + f * s * 0.12
        pygame.draw.line(screen, cr, (cxa, cya), (cxb, cyb), 1)
        pygame.draw.line(screen, crl, (cxa, cya - 1), (cxb, cyb - 1), 1)


def _crumple_part(part, p, seed, tip):
    """Return a display-only crumpled copy of one recipe part at death-progress p
    (0→1). Each fold bursts outward briefly, rotates, shrinks, and its height
    collapses to the page — a sheet of paper being crushed. Deterministic per part
    (seeded from its index) so the crush looks consistent frame to frame."""
    ax, ay = part["at"]
    # a cheap per-part pseudo-random direction (no RNG object needed)
    h = (seed * 2654435761) & 0xffffffff
    dirx = ((h & 0xff) / 255.0 - 0.5) * 2.0
    diry = (((h >> 8) & 0xff) / 255.0 - 0.5) * 2.0
    spin = (((h >> 16) & 0xff) / 255.0 - 0.5) * 2.0
    # burst outward then settle inward: sin(pi*p) peaks mid-death
    burst = math.sin(p * math.pi) * 0.5
    inward = 1.0 - 0.35 * p                      # overall pull toward centre late
    nax = ax * inward + dirx * burst
    nay = ay * inward + diry * burst - burst * 0.3   # slight upward pop
    q = dict(part)
    q["at"] = [nax, nay]
    q["size"] = max(0.05, part["size"] * (1.0 - 0.45 * p))       # shrink
    q["ang"] = part.get("ang", 0.0) + (spin + tip * 0.6) * p * 1.8  # tumble/fold
    q["h"] = part.get("h", 0.0) * (1.0 - p)                       # flatten to page
    q["tone"] = part.get("tone", 1.0) * (1.0 - 0.25 * p)          # dim as it dies
    return q


def _draw_waves(screen, g):
    """Draw the Tidal Wave crests as translucent cresting bands rolling up the
    lane, with notches punched out where obstacles have split the flow — so you
    can see a building casting a dry shadow behind it. Display-only."""
    for w in getattr(g, "waves", []):
        if w["t"] < w["delay"]:
            continue
        fade = max(0.25, 1.0 - w["traveled"] / w["reach"])
        gaps = w.get("gaps", [])
        cx, cy, sc = proj(w["x"], w["y"])
        half = int(w["width"] * sc)
        bw, bh = half * 2 + 20, 26
        band = pygame.Surface((bw, bh), pygame.SRCALPHA)
        # draw the band in world-x slices onto the temp surface, skipping shadows
        x0 = w["x"] - w["width"]
        seg = 12
        wx = x0
        while wx < w["x"] + w["width"]:
            midx = wx + seg / 2
            if not any(abs(midx - gx) < gh for gx, gh in gaps):
                # local x on the band surface
                lx = int((midx - x0) * sc)
                sw = max(2, int(seg * sc) + 1)
                pygame.draw.ellipse(band, (70, 140, 190, int(150 * fade)), (lx, 8, sw, 16))
                pygame.draw.ellipse(band, (170, 210, 240, int(210 * fade)), (lx, 0, sw, 10))
            wx += seg
        screen.blit(band, (int(cx - bw / 2), int(cy - bh / 2)))


def _draw_dying(screen, shadow_layer, d):
    """Render a DISPLAY-ONLY dying corpse as a per-fold PAPER CRUSH: each fold of
    the model scatters, tumbles and collapses flat to the page over the corpse's
    short lifetime, then the whole thing fades to scraps. Never touches the sim."""
    p = min(1.0, d["t"] / d["dur"])          # 0→1 over the death
    if p >= 1.0:
        return
    sx, sy, sc = proj(d["x"], d["y"])
    s = max(6, d["size"] * sc * unit_vis_scale())
    pad = int(s * 3.4)
    tmp = pygame.Surface((pad, pad), pygame.SRCALPHA)
    cx, cy = pad // 2, int(pad * 0.60)
    hgt = s * 0.5
    try:
        recipe = origami.recipe_for(d["key"], d["fam"], CARDS.get(d["key"]))
        anim = {"t": 0.0, "move": 0.0, "phase": 0.0, "attack": 0.0, "breathe": 0.0}
        # draw folds back-to-front, each crumpled per its own scatter
        for i, part in enumerate(recipe):
            cp = _crumple_part(part, p, i + 1, d["tip"])
            origami.draw_part(tmp, cp, cx, cy, s, d["face"], d["paper"], hgt, 0.0, 0.0, anim)
    except Exception:
        pygame.draw.circle(tmp, shade(d["paper"], 1.0), (cx, cy), max(2, int(s * 0.7 * (1 - p))))
    # a gentle global squash + a slight whole-body lean as it goes down
    squash = 1.0 - 0.35 * p
    lean = d["tip"] * 14 * (p ** 0.7)
    rot = pygame.transform.rotozoom(tmp, lean, 1.0)
    rw, rh = rot.get_size()
    rot = pygame.transform.smoothscale(rot, (rw, max(1, int(rh * squash))))
    # fade over the back half of the death
    alpha = int(255 * (1.0 - max(0.0, (p - 0.45) / 0.55)))
    rot.set_alpha(max(0, alpha))
    # a flattening ground shadow that spreads as the folds collapse
    shcol = (8, 5, 4, int(120 * (1 - p)))
    pygame.draw.ellipse(shadow_layer, shcol,
                        (sx - s * (1.1 + p), sy + s * 0.35, s * (2.2 + p * 2), s * (0.7 + p * 0.3)))
    screen.blit(rot, (sx - rot.get_width() / 2, sy + s * 0.4 - rot.get_height()))


class _DyingProxy:
    """(retained for compatibility; the crush path no longer needs a unit proxy)"""
    def __init__(self, d):
        self.key = d["key"]; self.face = d["face"]; self.team = d["team"]
        self.size = d["size"]; self.paper = d["paper"]; self.uid = 0


def origami_animal(screen, shadow_layer, u):
    sx, sy, sc = proj(u.x, u.y)
    # cartoon "boil": hand-inked frames never sit perfectly still — add a tiny
    # per-pose wobble so held frames feel drawn rather than frozen.
    if cartoon_on():
        beat = int(pygame.time.get_ticks() / 83)     # ~12fps pose index
        jr = random.Random(hash((id(u) & 0xffff, beat)))
        sx += jr.uniform(-1.4, 1.4); sy += jr.uniform(-1.4, 1.4)
    s = u.size * sc * unit_vis_scale()
    # unfold pop: fresh units spring open with a slight ease-out overshoot, so a
    # deploy reads as a sheet of paper snapping into shape rather than fading in.
    if u.spawn_t > 0:
        p = max(0.0, 1.0 - u.spawn_t / 0.35)      # 0→1 over the spawn window
        p = min(1.0, p)
        # ease-out-back: overshoots ~8% then settles to 1.0
        c1, c3 = 1.9, 2.9
        grow = 1 + c3 * (p - 1) ** 3 + c1 * (p - 1) ** 2
        grow = max(0.2, grow)
    else:
        grow = 1.0
    s = max(6, s * grow)
    f = u.face
    # firing animation: a quick lunge toward the enemy + a muzzle flash, easing back
    ff = getattr(u, "fire_flash", 0)
    if ff > 0:
        prog = ff / 0.25
        lunge = math.sin(prog * math.pi) * s * 0.30
        sx += f * lunge
    c = u.paper
    hgt = max(4, s * 0.5)          # extrusion height: the 3D read

    # ── flying separation: airborne pieces lift off the page, and their shadow
    #    detaches and sinks below with a growing gap so they READ as flying. ──
    fly = getattr(u, "flying", False)
    tms = pygame.time.get_ticks()
    if fly:
        hover = (math.sin(tms * 0.004 + sx * 0.05) * 0.5 + 0.5)   # 0..1 bob
        altitude = s * (0.85 + hover * 0.35)                       # lift height
        shadow_gap = altitude                                      # detached shadow
        shx, shy = sx, sy + s * 0.4
        # small, soft, detached shadow on the ground (shrinks as it climbs)
        shrink = 1.0 - 0.25 * hover
        cast_shadow(shadow_layer, shx, shy, 1.0, s * 0.78 * shrink, LIGHT_MAIN,
                    stretch=2.0, alpha=70)
        pygame.draw.ellipse(shadow_layer, (8, 5, 4, 60),
                            (shx - s * 0.8 * shrink, shy, s * 1.6 * shrink, s * 0.5 * shrink))
        sy -= altitude                       # the model itself flies up
    else:
        cast_shadow(shadow_layer, sx, sy + s * 0.4, 1.0, s, LIGHT_MAIN)
        pygame.draw.ellipse(shadow_layer, (8, 5, 4, 130),
                            (sx - s * 1.15, sy + s * 0.35, s * 2.3, s * 0.7))
    x, y = sx, sy
    if u.riverhop and not fly:
        y -= abs(math.sin(tms * 0.008 + sx)) * s * 0.5

    # ── fold the model from its recipe (data-driven, JSON-authorable) ──
    drawn = False
    # spirit-divergence echoes render as shadowy apparitions. Two styles, toggled
    # in the debug menu: "apparition" folds the ORIGINAL card model but ghostly
    # (dark, semi-transparent, drifting); "wraith" is a bespoke tattered form.
    if getattr(u, "is_echo", False):
        _draw_echo(screen, u, x, y, s, f, hgt, tms)
        drawn = True
    # a launched eagle draws as a folded raptor with a slung bomb (outbound) —
    # never as its nest model.
    elif getattr(u, "is_eagle", False):
        _draw_eagle(screen, u, x, y, s, f, hgt, tms)
        drawn = True
    if not drawn and ORIGAMI_OK:
        try:
            fam = _card_family(u.key)
            card = CARDS.get(u.key)
            recipe = origami.recipe_for(u.key, fam, card)
            # wing flap factor: fast for flyers, gentle idle otherwise
            flap = math.sin(tms * (0.02 if fly else 0.008) + x * 0.1)
            # ── animation state driving per-part motion (all cosmetic) ──
            # units move by changing x/y directly (not vx/vy), so estimate walk
            # speed from the frame-to-frame world delta, tracked render-side only.
            px = getattr(u, "_anim_px", None); py = getattr(u, "_anim_py", None)
            if px is not None:
                wspd = math.hypot(u.x - px, u.y - py)     # world px moved since last frame
            else:
                wspd = 0.0
            u._anim_px, u._anim_py = u.x, u.y
            # normalise: ~a full walk cycle when covering >~2px/frame
            walk = 0.0 if (fly or getattr(u, "building", False)) else min(1.0, wspd / 2.2)
            move = getattr(u, "_anim_move", 0.0) * 0.6 + walk * 0.4   # smooth
            u._anim_move = move
            anim = {
                "t": tms * 0.001,
                "move": move,
                "phase": (id(u) & 0xff) * 0.1,       # de-sync the crowd
                "attack": (math.sin((ff / 0.25) * math.pi) if ff > 0 else 0.0),
                "breathe": math.sin(tms * 0.0025 + (id(u) & 0xff)),
                "style": _ANIM_STYLE.get(fam, ""),   # family motion flavour
            }
            for part in recipe:
                origami.draw_part(screen, part, x, y, s, f, c, hgt, tms * 0.001, flap, anim)
            drawn = True
        except Exception:
            drawn = False

    if not drawn:
        _origami_animal_legacy(screen, u, x, y, s, f, c, hgt, tms)
    # ── hi-mode character detail: a rim-light along the candle-facing edge and a
    #    couple of fold creases, so the paper models read with more relief. Cheap
    #    (a handful of AA lines per unit) and only in photorealistic mode. ──
    if drawn and gfx_hi() and s >= 9 and not getattr(u, "is_echo", False):
        _hi_model_detail(screen, u, x, y, s, f, c, hgt)
    ty_ = y - s * 0.9 - hgt
    # muzzle flash: a warm spark at the unit's leading edge when it just fired.
    # When the tiny diffusion model is available we blit a generated ember tile
    # for a richer, non-repeating flash; otherwise the drawn circles.
    if ff > 0 and gfx_hi():
        prog = ff / 0.25
        fa = int(200 * math.sin(prog * math.pi))
        fx0 = x + f * s * 1.1
        fy0 = y - hgt * 0.5
        sp = None
        if DIFFUSION_OK:
            sz = max(8, int(s * 1.4))
            base = vfx_sprite("ember", size=sz, seed=(id(u) >> 4) & 7)
            if base.get_width():
                sp = base.copy()
                sp.set_alpha(fa)
        if sp is not None:
            screen.blit(sp, (fx0 - sp.get_width() / 2, fy0 - sp.get_height() / 2),
                        special_flags=pygame.BLEND_ADD)
        else:
            spark = pygame.Surface((int(s), int(s)), pygame.SRCALPHA)
            pygame.draw.circle(spark, (255, 220, 150, fa), (int(s / 2), int(s / 2)), int(s * 0.4))
            pygame.draw.circle(spark, (255, 250, 220, fa), (int(s / 2), int(s / 2)), int(s * 0.2))
            screen.blit(spark, (fx0 - s / 2, fy0 - s / 2), special_flags=pygame.BLEND_ADD)
    pygame.draw.polygon(screen, P_ACCENT if u.team == 0 else E_ACCENT,
        [(x - s, ty_), (x - s + 10, ty_ + 4), (x - s, ty_ + 8)])
    bw = s * 1.8
    pygame.draw.rect(screen, (30, 24, 20), (x - bw / 2, ty_ - 8, bw, 5))
    pygame.draw.rect(screen, P_ACCENT if u.team == 0 else E_ACCENT,
                     (x - bw / 2, ty_ - 8, bw * u.hp / u.maxhp, 5))

def draw_tower(screen, shadow_layer, t):
    sx, sy, sc = proj(t.x, t.y)
    s = t.size * sc * tower_vis_scale()
    # troop towers use their own accent roof; king + defaults use team color
    if getattr(t, "roof", None) and not t.king:
        acc = t.roof
    else:
        acc = P_ACCENT if t.team == 0 else E_ACCENT
    cast_shadow(shadow_layer, sx, sy + s * 0.5, 1.0, s * 1.3, LIGHT_MAIN, stretch=3.0, alpha=115)
    pygame.draw.ellipse(shadow_layer, (8, 5, 4, 130),
                        (sx - s * 1.25, sy + s * 0.42, s * 2.5, s * 0.9))
    tiers = 3 if t.king else 2
    base_y = sy + s * 0.55
    for i in range(tiers):
        w = s * (1.2 - i * 0.3); hh = s * 0.5
        yb = base_y - i * hh
        tier = [(sx - w, yb), (sx - w * 0.86, yb - s * 0.12), (sx + w * 0.86, yb - s * 0.12), (sx + w, yb)]
        draw_prism(screen, tier, hh, shade(PAGE, 0.96), top_detail=False)
        # roof overhang casts a soft contact band onto the wall below
        pygame.draw.line(screen, shade(PAGE, 0.45), (sx - w * 0.9, yb - hh + 2), (sx + w * 0.9, yb - hh + 2), 3)
        # pagoda roof: two lit/unlit faces = 3D pyramid
        rw = w * 1.18; ry = yb - hh
        peak = (sx, ry - s * 0.42)
        lamL = light_lambert(sx - rw / 2, ry - s * 0.2, -0.8, -0.35, boost=0.7, floor=0.4)
        lamR = light_lambert(sx + rw / 2, ry - s * 0.2, 0.8, -0.35, boost=0.7, floor=0.4)
        pygame.draw.polygon(screen, shade(acc, lamL), [(sx - rw, ry), peak, (sx, ry + s * 0.06)])
        pygame.draw.polygon(screen, shade(acc, lamR), [(sx + rw, ry), peak, (sx, ry + s * 0.06)])
        pygame.draw.line(screen, shade(acc, max(lamL, lamR) * 1.3), (sx - rw, ry), peak, 2)
        pygame.draw.line(screen, shade(acc, max(lamL, lamR) * 1.3), (sx + rw, ry), peak, 2)
    if t.king:
        top_y = base_y - tiers * s * 0.5 - s * 0.42
        pygame.draw.polygon(screen, GOLD, [(sx, top_y - 14), (sx + 9, top_y - 2), (sx, top_y - 5), (sx - 9, top_y - 2)])
    bw = s * 2
    hb_y = base_y - tiers * s * 0.5 - s * 0.6
    pygame.draw.rect(screen, (30, 24, 20), (sx - bw / 2, hb_y, bw, 6))
    pygame.draw.rect(screen, acc, (sx - bw / 2, hb_y, bw * max(0, t.hp) / t.maxhp, 6))

def draw_boss_hand(screen, wrist, tip, spread=1.0, holding=None, sfont=None):
    wx, wy = wrist
    tx, ty = tip
    dx, dy = tx - wx, ty - wy
    d = math.hypot(dx, dy) or 1
    ux, uy = dx / d, dy / d
    px, py = -uy, ux
    arm_w = 46
    arm = [(wx + px * arm_w, wy + py * arm_w),
           (wx - px * arm_w, wy - py * arm_w),
           (wx - px * arm_w * 1.2 - ux * 260, wy - py * arm_w * 1.2 - uy * 260),
           (wx + px * arm_w * 1.2 - ux * 260, wy + py * arm_w * 1.2 - uy * 260)]
    pygame.draw.polygon(screen, HAND_COL, arm)
    palm_l = d * 0.42
    palm = [(wx + px * 40, wy + py * 40),
            (wx - px * 40, wy - py * 40),
            (wx - px * 34 + ux * palm_l, wy - py * 34 + uy * palm_l),
            (wx + px * 34 + ux * palm_l, wy + py * 34 + uy * palm_l)]
    pygame.draw.polygon(screen, HAND_COL, palm)
    pygame.draw.line(screen, HAND_RIM, palm[0], palm[3], 2)
    # knuckle sheen: subtle candle-side gradient
    pygame.draw.line(screen, shade(HAND_COL, 1.6), palm[3], palm[2], 2)
    base_x, base_y = wx + ux * palm_l, wy + uy * palm_l
    for i in range(4):
        off = (i - 1.5) * 22 * spread
        fx, fy = base_x + px * off, base_y + py * off
        seg1 = d * 0.30; seg2 = d * 0.26
        bend = 0.28 + 0.1 * math.sin(i * 1.7)
        mx = fx + ux * seg1 + px * off * 0.15
        my = fy + uy * seg1 + abs(off) * 0.06
        ex = mx + (ux * math.cos(bend) - uy * math.sin(bend)) * seg2 * 0.9
        ey = my + (uy * math.cos(bend) + ux * math.sin(bend)) * seg2 * 0.9
        for (ax, ay, bx2, by2, w0, w1) in ((fx, fy, mx, my, 11, 8), (mx, my, ex, ey, 8, 4)):
            ddx, ddy = bx2 - ax, by2 - ay
            dd = math.hypot(ddx, ddy) or 1
            qx, qy = -ddy / dd, ddx / dd
            pygame.draw.polygon(screen, HAND_COL,
                [(ax + qx * w0, ay + qy * w0), (ax - qx * w0, ay - qy * w0),
                 (bx2 - qx * w1, by2 - qy * w1), (bx2 + qx * w1, by2 + qy * w1)])
            pygame.draw.line(screen, HAND_RIM, (ax + qx * w0, ay + qy * w0),
                             (bx2 + qx * w1, by2 + qy * w1), 2)
    thx, thy = wx + ux * palm_l * 0.4 + px * 48, wy + uy * palm_l * 0.4 + py * 48
    pygame.draw.polygon(screen, HAND_COL,
        [(wx + px * 36, wy + py * 36), (thx, thy), (thx + ux * 30, thy + uy * 30),
         (wx + px * 20 + ux * 40, wy + py * 20 + uy * 40)])
    if holding:
        cx, cy = base_x + ux * d * 0.34, base_y + uy * d * 0.34
        card = pygame.Rect(0, 0, 46, 62); card.center = (cx, cy)
        pygame.draw.rect(screen, shade(PAGE, 0.55), card.move(3, 4), border_radius=4)
        pygame.draw.rect(screen, shade(PAGE, 0.9), card, border_radius=4)
        pygame.draw.rect(screen, shade(PAGE, 0.5), card, 2, border_radius=4)
        pygame.draw.circle(screen, SIGIL_COLORS.get(CARDS[holding]["sigil"], GOLD),
                           (card.centerx, card.centery + 16), 7)
        if sfont:
            g2 = sfont.render(CARDS[holding]["name"].split()[0], True, INK)
            screen.blit(g2, (card.centerx - g2.get_width() / 2, card.y + 6))

_LIGHT_CACHE = {"key": None, "base": None, "lights": None}
def _radial_light(maxr, warm, steps, falloff, hi):
    """Render ONE light's radial gradient into its own tight surface, once. The
    surface is 2*maxr square with the light at its centre, so it can be blitted
    anywhere cheaply instead of redrawing dozens of full-screen circles/frame."""
    R = max(2, int(maxr))
    surf = pygame.Surface((R * 2, R * 2))
    surf.fill((0, 0, 0))
    cx = cy = R
    for i in range(steps, 0, -1):
        r = maxr * i / steps
        k = (1 - i / steps) ** falloff
        col = tuple(int(warm[j] * k) for j in range(3))
        pygame.draw.circle(surf, col, (cx, cy), int(r))
    if hi:
        for i in range(8, 0, -1):
            r = maxr * 0.10 * i / 8
            k = (1 - i / 8) ** 1.2
            col = tuple(min(255, int(warm[j] * k * 1.4)) for j in range(3))
            pygame.draw.circle(surf, col, (cx, cy), int(r))
    surf.set_colorkey((0, 0, 0))
    return surf


def build_lightmap(t, flicker=True, dynamic=None):
    hi = gfx_hi()
    lm = pygame.Surface((W, H))
    lm.fill(shade(AMBIENT, 1.12) if hi else AMBIENT)
    fl = 1.0
    if flicker:
        tt = pygame.time.get_ticks() * 0.001
        fl = 0.92 + 0.06 * math.sin(tt * 11) + 0.04 * math.sin(tt * 23 + 1.7)
    steps = 56 if hi else 24
    falloff = 2.2 if hi else 1.8
    # ── static lights: each light's gradient is rendered ONCE into a tight cached
    #    sprite, then blitted (additively) every frame. Rebuilt only if the light
    #    set / resolution / preset changed — not per frame. This removes hundreds
    #    of full-screen circle draws that dominated hi-mode cost. ──
    key = (W, H, hi, steps, falloff, tuple((tuple(p), r, tuple(c)) for p, r, c in LIGHTS))
    if _LIGHT_CACHE["key"] != key:
        sprites = []
        for (lx, ly), maxr, warm in LIGHTS:
            sprites.append((_radial_light(maxr, warm, steps, falloff, hi), maxr))
        _LIGHT_CACHE["key"] = key
        _LIGHT_CACHE["lights"] = sprites
    for ((lx, ly), maxr, warm), (spr, r0) in zip(LIGHTS, _LIGHT_CACHE["lights"]):
        # flicker scales brightness cheaply via per-blit alpha (no re-render)
        spr.set_alpha(int(255 * min(1.0, fl)))
        lm.blit(spr, (int(lx - r0), int(ly - r0)), special_flags=pygame.BLEND_ADD)
    # ── dynamic colored lights from units, projectiles and spells (hi mode) ──
    if hi and dynamic:
        dsteps = 10
        for (dx, dy), dr, dcol, di in dynamic:
            R = max(2, int(dr))
            ls = pygame.Surface((R * 2, R * 2))
            ls.fill((0, 0, 0)); ls.set_colorkey((0, 0, 0))
            for i in range(dsteps, 0, -1):
                r = dr * i / dsteps
                k = (1 - i / dsteps) ** 1.6 * di
                col = tuple(min(255, int(dcol[j] * k)) for j in range(3))
                pygame.draw.circle(ls, col, (R, R), int(r))
            lm.blit(ls, (int(dx - R), int(dy - R)), special_flags=pygame.BLEND_ADD)
    return lm

# sigil → emitted light color (dynamic colored lighting in hi mode)
SIGIL_LIGHT = {
    "pounce": (200, 60, 40), "toxic": (90, 200, 70), "freeze": (120, 200, 255),
    "frost": (140, 210, 255), "chill": (140, 210, 255), "radiance": (255, 210, 120),
    "blink": (140, 240, 200), "rampage": (255, 110, 50), "tidal": (80, 150, 230),
    "beacon": (255, 180, 80), "rebirth": (255, 150, 60), "artillery": (150, 130, 255),
    "inksplash": (90, 120, 220), "forge": (255, 140, 70),
    "molten": (255, 120, 50), "warp": (120, 220, 200), "siphon": (150, 90, 200),
    "echo": (170, 150, 230), "gale": (160, 210, 235),
}

def apply_postfx(screen, t):
    """Photorealistic post pass: crispen (unsharp) + bloom + film grain + vignette.
    No-op in low mode. Runs once at the very end of render(). Tuned to reuse
    downscales and cached tiles so it stays cheap on mobile."""
    if not gfx_hi():
        return
    # one shared half-res downscale feeds both the unsharp blur and (further
    # reduced) the bloom, so we pay for the big smoothscale once.
    half = pygame.transform.smoothscale(screen, (W // 2, H // 2))
    # ---- unsharp mask: subtract a blurred (upscaled-half) copy to sharpen edges
    blur = pygame.transform.smoothscale(half, (W, H))
    blur.set_alpha(70)
    hi = screen.copy()
    hi.blit(blur, (0, 0), special_flags=pygame.BLEND_SUB)
    hi.set_alpha(60)
    screen.blit(hi, (0, 0), special_flags=pygame.BLEND_ADD)
    # ---- bloom: reduce the half further, keep brights (mult by self), add back
    small = pygame.transform.smoothscale(half, (W // 6, H // 6))
    small.blit(small, (0, 0), special_flags=pygame.BLEND_MULT)
    bloom = pygame.transform.smoothscale(small, (W, H))
    bloom.set_alpha(90)
    screen.blit(bloom, (0, 0), special_flags=pygame.BLEND_ADD)
    # ---- film grain: a scrolling low-alpha noise tile (small blits are cheap)
    grain = _grain_tile()
    ox = int((t * 37) % 64); oy = int((t * 53) % 64)
    for gx in range(-64, W, 64):
        for gy in range(-64, H, 64):
            screen.blit(grain, (gx + ox, gy + oy))
    # ---- warm cinematic vignette on top of the cool one (cached; static per size)
    screen.blit(_warm_vignette(), (0, 0))

_VIGNETTE = {"key": None, "surf": None}
def _warm_vignette():
    key = (W, H)
    if _VIGNETTE["key"] != key:
        vg = pygame.Surface((W, H), pygame.SRCALPHA)
        for i in range(8):
            a = i * 10
            inset = i * 26
            pygame.draw.ellipse(vg, (10, 6, 4, a),
                                (-inset, -inset, W + inset * 2, H + inset * 2), width=40)
        _VIGNETTE["key"] = key
        _VIGNETTE["surf"] = vg
    return _VIGNETTE["surf"]

_GRAIN = None
def _grain_tile():
    global _GRAIN
    if _GRAIN is None:
        _GRAIN = pygame.Surface((64, 64), pygame.SRCALPHA)
        rr = random.Random(99)
        for x in range(64):
            for y in range(64):
                v = rr.randint(0, 26)
                _GRAIN.set_at((x, y), (v, v, v, 22))
    return _GRAIN

# ── Black & white ink-sketch card art ──────────────────────────────────────
# Each card gets a distinctive hand-inked illustration: a creature silhouette
# built from the card's "family" (inferred from its key/flags), rendered in a
# cross-hatched pen style with ink blotches and spatter. Cached per (key, size).
INK_BLACK = (24, 20, 18)
INK_DK = (44, 38, 34)
SKETCH_BG = (222, 210, 180)      # aged-paper tone the sketch sits on

# map each card to a silhouette family used by the generator
_CARD_FAMILY = {
    "sabertooth": "feline", "shadowcat": "feline", "coyote": "canine",
    "wolverine": "mustelid", "honeybadger": "mustelid", "wolfpack": "canine",
    "pangolin": "armored", "scarab": "insect", "beetle": "insect",
    "mantis": "insect", "locust": "insect", "weaver": "insect",
    "crane": "bird", "batcolony": "bird", "mothswarm": "insect",
    "terrorbird": "bird", "ravencrows": "bird", "heron": "bird",
    "phoenix": "bird", "sunbird": "bird", "cassowary": "bird",
    "gorilla": "ape", "sloth": "ape", "mammoth": "tusker", "boar": "tusker",
    "warthog": "tusker", "frog": "amphibian", "shark": "fish", "squid": "cephalopod",
    "leviathan": "cephalopod", "tidalwave": "wave", "lantern": "jelly",
    "baobab": "tree", "dragontree": "tree", "monkeypuzzle": "tree",
    "obelisk": "monolith", "ironsmith": "monolith", "piper": "monolith",
    "inkgolem": "golem", "voidcritter": "blob", "mimic": "blob",
    "frostmage": "figure", "mothmage": "figure", "foldknight": "figure",
    "inkwitch": "figure", "willowisp": "wisp", "jackalope": "rabbit",
    "tortoise": "shelled",
    # ── expanse set ──
    "stagbeetle": "insect", "emberfox": "canine", "leech": "amphibian",
    "legion": "figure", "bellfrog": "amphibian",
    "condor": "bird", "mushroom": "tree", "rhino": "tusker",
    "wisppack": "wisp", "golemsm": "golem", "serpent": "cephalopod",
    # ── tideturn set ──
    "lanternfish": "fish", "dunestrider": "feline", "papercutkite": "bird",
    "bastioram": "tusker", "embermoth": "insect", "glasstortoise": "shelled",
    "saltgolem": "golem", "reedcolony": "insect", "inkwren": "bird",
    "cinderhound": "canine", "marbleaegis": "monolith", "willothewisp": "wisp",
    # ── pennywise set (cheap 1-2 ink) ──
    "inkling": "blob", "papermoth": "insect", "sparkmouse": "mustelid",
    "pebbletoad": "amphibian", "cinderling": "wisp", "dustsprite": "wisp",
    "reedskiff": "insect", "thistleboar": "tusker", "gullpair": "bird",
    "hedgemage": "figure", "beetlecart": "shelled", "emberwren": "bird",
    "mudcrab": "shelled", "sproutling": "tree",
    # ── menagerie set (special-mechanic units) ──
    "mole": "mustelid", "eaglenest": "monolith", "beekeeper": "ape",
    "wildcard": "wisp", "foldwright": "monolith",
}

def _card_family(key):
    if key in _CARD_FAMILY:
        return _CARD_FAMILY[key]
    d = CARDS.get(key, {})
    if d.get("building"):
        return "monolith"
    if d.get("flying"):
        return "bird"
    return "feline"


# per-family motion flavour, read by the origami animator. Each is a light tweak
# on top of the shared walk/breathe cycle so a family MOVES like what it is —
# cephalopods & serpents undulate, jellies pulse, wisps flicker, insects skitter.
_ANIM_STYLE = {
    "cephalopod": "undulate", "wave": "undulate",
    "jelly": "pulse", "amphibian": "hop",
    "wisp": "flicker", "blob": "wobble",
    "insect": "skitter", "bird": "flap",
    "fish": "swim", "wisppack": "flicker",
}

_SKETCH_CACHE = {}

# ── diffusion-generated VFX sprites (tiny, portable, cached) ────────────────
# A small pure-numpy diffusion model generates abstract texture tiles (ember,
# ink, frost, aura, smoke) that we turn into additive/alpha VFX sprites. Purely
# cosmetic + cached at first use, so it never touches the sim or the hot path.
try:
    import diffusion as _diffusion
    DIFFUSION_OK = True
except Exception:
    DIFFUSION_OK = False
    _diffusion = None

_DIFFUSION_MODEL = None
_VFX_SPRITE_CACHE = {}

def _diffusion_model():
    global _DIFFUSION_MODEL
    if _DIFFUSION_MODEL is None and DIFFUSION_OK:
        try:
            _DIFFUSION_MODEL = _diffusion.TinyDiffusion.load_or_train()
        except Exception:
            _DIFFUSION_MODEL = False      # mark as unavailable
    return _DIFFUSION_MODEL or None

def vfx_sprite(concept, size=48, seed=0, additive=True):
    """A cached pygame VFX sprite generated by the tiny diffusion model (falls
    back to the procedural tile if the model isn't available). Alpha follows
    luminance so bright bits show and dark bits fade — ideal for additive bursts."""
    key = (concept, size, seed, additive)
    if key in _VFX_SPRITE_CACHE:
        return _VFX_SPRITE_CACHE[key]
    arr = None
    m = _diffusion_model()
    try:
        if m is not None:
            arr = m.generate(concept, size=size, seed=seed)
        elif DIFFUSION_OK:
            arr = _diffusion.procedural_tile(concept, size=size, seed=seed)
    except Exception:
        arr = None
    if arr is None:
        surf = pygame.Surface((size, size), pygame.SRCALPHA)
        _VFX_SPRITE_CACHE[key] = surf
        return surf
    # build an SRCALPHA surface; alpha = luminance (so the tile reads as a glow)
    surf = pygame.Surface((size, size), pygame.SRCALPHA)
    import numpy as _np
    lum = (0.299 * arr[..., 0] + 0.587 * arr[..., 1] + 0.114 * arr[..., 2]).astype(_np.uint8)
    rgba = _np.dstack([arr[..., 0], arr[..., 1], arr[..., 2], lum])
    try:
        pygame.surfarray.blit_array(surf, _np.transpose(rgba[..., :3], (1, 0, 2)))
        alpha = pygame.surfarray.pixels_alpha(surf)
        alpha[:] = _np.transpose(lum, (1, 0))
        del alpha
    except Exception:
        pass
    _VFX_SPRITE_CACHE[key] = surf
    return surf


_FONT_CACHE = {}
def _font(size, bold=False, italic=False):
    """Cached SysFont — creating a font every frame is a real pygame hitch, so we
    memoise by (size,bold,italic). Falls back to the default font if the family
    is unavailable."""
    key = (size, bold, italic)
    fnt = _FONT_CACHE.get(key)
    if fnt is None:
        try:
            fnt = pygame.font.SysFont("georgia,dejavuserif", size, bold=bold, italic=italic)
        except Exception:
            fnt = pygame.font.Font(None, size)
        _FONT_CACHE[key] = fnt
    return fnt

def _hatch(surf, poly, seed, dark=INK_DK, dens=1.0, cross=True):
    """Fill a polygon region with pen cross-hatching, clipped to the polygon."""
    if len(poly) < 3:
        return
    xs = [p[0] for p in poly]; ys = [p[1] for p in poly]
    x0, y0, x1, y1 = min(xs), min(ys), max(xs), max(ys)
    w, h = int(x1 - x0) + 2, int(y1 - y0) + 2
    if w < 2 or h < 2:
        return
    layer = pygame.Surface((w, h), pygame.SRCALPHA)
    lp = [(p[0] - x0, p[1] - y0) for p in poly]
    rr = random.Random(seed)
    step = max(3, int(6 / dens))
    for off in range(-h, w, step):
        a = rr.randint(70, 130)
        pygame.draw.line(layer, (*dark, a), (off, 0), (off + h, h), 1)
    if cross:
        for off in range(0, w + h, step + 1):
            a = rr.randint(40, 90)
            pygame.draw.line(layer, (*dark, a), (off, 0), (off - h, h), 1)
    mask = pygame.Surface((w, h), pygame.SRCALPHA)
    pygame.draw.polygon(mask, (255, 255, 255, 255), lp)
    layer.blit(mask, (0, 0), special_flags=pygame.BLEND_RGBA_MULT)
    surf.blit(layer, (x0, y0))

def _ink_stroke(surf, pts, seed, width=2, wobble=1.4):
    """A slightly wobbly hand-drawn ink stroke (double-passed for weight)."""
    rr = random.Random(seed)
    jit = [(x + rr.uniform(-wobble, wobble), y + rr.uniform(-wobble, wobble)) for x, y in pts]
    if len(jit) >= 2:
        pygame.draw.lines(surf, INK_BLACK, False, jit, width)
        jit2 = [(x + rr.uniform(-1, 1), y + rr.uniform(-1, 1)) for x, y in jit]
        pygame.draw.lines(surf, INK_DK, False, jit2, max(1, width - 1))

def _blotches(surf, w, h, seed, n=7):
    """Scatter ink blotches: a few big irregular blobs + fine spatter specks."""
    rr = random.Random(seed)
    for _ in range(n):
        cx, cy = rr.randint(4, max(5, w - 4)), rr.randint(4, max(5, h - 4))
        R = rr.randint(3, 9)
        for _ in range(rr.randint(3, 6)):
            ox, oy = rr.randint(-R, R), rr.randint(-R, R)
            a = rr.randint(120, 210)
            pygame.draw.circle(surf, (*INK_BLACK, a), (cx + ox, cy + oy), rr.randint(2, R))
        if rr.random() < 0.5:
            pygame.draw.line(surf, (*INK_BLACK, 150), (cx, cy),
                             (cx + rr.randint(-6, 6), cy + rr.randint(4, 12)), rr.randint(1, 2))
    for _ in range(rr.randint(30, 55)):
        sx, sy = rr.randint(0, max(1, w - 1)), rr.randint(0, max(1, h - 1))
        pygame.draw.circle(surf, (*INK_BLACK, rr.randint(60, 160)), (sx, sy), rr.randint(0, 1) + 1)

def _sil_body(fam, cx, cy, s, rr):
    """Return the main body polygon for a family silhouette (local coords)."""
    if fam in ("feline", "canine", "mustelid"):
        return [(cx - s, cy + s * 0.2), (cx - s * 0.7, cy - s * 0.4),
                (cx - s * 0.1, cy - s * 0.5), (cx + s * 0.8, cy - s * 0.3),
                (cx + s, cy + s * 0.1), (cx + s * 0.6, cy + s * 0.5),
                (cx - s * 0.6, cy + s * 0.5)]
    if fam == "ape":
        return [(cx - s * 0.7, cy + s * 0.6), (cx - s * 0.8, cy - s * 0.3),
                (cx - s * 0.3, cy - s * 0.8), (cx + s * 0.3, cy - s * 0.8),
                (cx + s * 0.8, cy - s * 0.3), (cx + s * 0.7, cy + s * 0.6)]
    if fam == "tusker":
        return [(cx - s, cy + s * 0.3), (cx - s * 0.9, cy - s * 0.3),
                (cx - s * 0.2, cy - s * 0.55), (cx + s * 0.7, cy - s * 0.4),
                (cx + s, cy), (cx + s * 0.8, cy + s * 0.5), (cx - s * 0.7, cy + s * 0.55)]
    if fam == "bird":
        return [(cx - s * 0.3, cy + s * 0.5), (cx - s * 0.5, cy - s * 0.1),
                (cx, cy - s * 0.6), (cx + s * 0.6, cy - s * 0.3),
                (cx + s * 0.3, cy + s * 0.2), (cx + s * 0.2, cy + s * 0.55)]
    if fam == "insect":
        return [(cx - s * 0.5, cy), (cx, cy - s * 0.5), (cx + s * 0.6, cy - s * 0.1),
                (cx + s * 0.4, cy + s * 0.4), (cx - s * 0.3, cy + s * 0.45)]
    if fam == "golem":
        # a hulking stack of slabs: huge shoulders, small head notch, stumpy legs
        return [(cx - s * 0.9, cy + s * 0.7), (cx - s * 0.95, cy - s * 0.1),
                (cx - s * 0.55, cy - s * 0.6), (cx - s * 0.2, cy - s * 0.55),
                (cx - s * 0.15, cy - s * 0.8), (cx + s * 0.15, cy - s * 0.8),
                (cx + s * 0.2, cy - s * 0.55), (cx + s * 0.55, cy - s * 0.6),
                (cx + s * 0.95, cy - s * 0.1), (cx + s * 0.9, cy + s * 0.7)]
    if fam == "monolith":
        # a leaning standing stone with a chipped crown
        return [(cx - s * 0.42, cy + s * 0.85), (cx - s * 0.5, cy - s * 0.55),
                (cx - s * 0.28, cy - s * 0.9), (cx + s * 0.1, cy - s * 0.98),
                (cx + s * 0.34, cy - s * 0.7), (cx + s * 0.46, cy + s * 0.85)]
    if fam == "armored":
        # a low dome of plates with a small snout
        return [(cx - s * 0.95, cy + s * 0.4), (cx - s * 0.7, cy - s * 0.3),
                (cx - s * 0.2, cy - s * 0.6), (cx + s * 0.35, cy - s * 0.55),
                (cx + s * 0.8, cy - s * 0.15), (cx + s * 1.05, cy + s * 0.2),
                (cx + s * 0.9, cy + s * 0.4)]
    if fam == "amphibian":
        # a squat crouch: wide haunches, throat bulge
        return [(cx - s * 0.85, cy + s * 0.45), (cx - s * 0.9, cy),
                (cx - s * 0.4, cy - s * 0.45), (cx + s * 0.25, cy - s * 0.5),
                (cx + s * 0.75, cy - s * 0.1), (cx + s * 0.85, cy + s * 0.3),
                (cx + s * 0.5, cy + s * 0.5)]
    if fam == "wisp":
        # a rising teardrop flame with a curling tail
        return [(cx - s * 0.15, cy + s * 0.75), (cx - s * 0.45, cy + s * 0.25),
                (cx - s * 0.35, cy - s * 0.35), (cx, cy - s * 0.85),
                (cx + s * 0.35, cy - s * 0.3), (cx + s * 0.42, cy + s * 0.3),
                (cx + s * 0.1, cy + s * 0.7)]
    if fam == "rabbit":
        # a sitting silhouette with the ears the whole point
        return [(cx - s * 0.55, cy + s * 0.55), (cx - s * 0.6, cy),
                (cx - s * 0.25, cy - s * 0.4), (cx - s * 0.22, cy - s * 1.0),
                (cx - s * 0.05, cy - s * 0.45), (cx + s * 0.12, cy - s * 1.05),
                (cx + s * 0.28, cy - s * 0.4), (cx + s * 0.6, cy + s * 0.1),
                (cx + s * 0.45, cy + s * 0.55)]
    if fam == "figure":
        # a robed biped: head, shoulders, and a hem that flares at the floor
        return [(cx - s * 0.7, cy + s * 0.85), (cx - s * 0.32, cy + s * 0.1),
                (cx - s * 0.4, cy - s * 0.3), (cx - s * 0.18, cy - s * 0.55),
                (cx - s * 0.14, cy - s * 0.8), (cx + s * 0.14, cy - s * 0.8),
                (cx + s * 0.18, cy - s * 0.55), (cx + s * 0.4, cy - s * 0.3),
                (cx + s * 0.32, cy + s * 0.1), (cx + s * 0.7, cy + s * 0.85)]
    if fam in ("fish", "cephalopod"):
        return [(cx - s, cy), (cx - s * 0.4, cy - s * 0.5), (cx + s * 0.5, cy - s * 0.35),
                (cx + s, cy), (cx + s * 0.5, cy + s * 0.35), (cx - s * 0.4, cy + s * 0.5)]
    if fam == "tree":
        return [(cx - s * 0.9, cy - s * 0.1), (cx - s * 0.3, cy - s * 0.8),
                (cx + s * 0.3, cy - s * 0.9), (cx + s * 0.9, cy - s * 0.2),
                (cx + s * 0.5, cy + s * 0.2), (cx - s * 0.5, cy + s * 0.2)]
    # rounded/organic mass for everything else
    return [(cx - s * 0.7, cy + s * 0.5), (cx - s * 0.8, cy - s * 0.2),
            (cx - s * 0.3, cy - s * 0.7), (cx + s * 0.3, cy - s * 0.7),
            (cx + s * 0.8, cy - s * 0.2), (cx + s * 0.7, cy + s * 0.5)]

def make_card_sketch(key, w, h):
    """Build a black & white ink-sketch surface for a card, sized (w, h)."""
    ck = (key, w, h)
    if ck in _SKETCH_CACHE:
        return _SKETCH_CACHE[ck]
    fam = _card_family(key)
    rr = random.Random(hash(("sketch", key)) & 0xffffffff)
    surf = pygame.Surface((w, h), pygame.SRCALPHA)
    surf.fill(SKETCH_BG)
    cx, cy = w * 0.5, h * 0.52
    s = min(w, h) * 0.34
    body = _sil_body(fam, cx, cy, s, rr)
    _hatch(surf, body, hash(key) & 0xffff, dens=1.1, cross=True)
    _hatch(surf, body, (hash(key) >> 3) & 0xffff, dens=1.6, cross=False)
    _ink_stroke(surf, body + [body[0]], hash(key) & 0x7fff, width=3, wobble=1.6)
    # family-specific ink accents
    if fam in ("feline", "canine", "mustelid"):
        hx, hy = cx + s * 0.85, cy - s * 0.25
        pygame.draw.circle(surf, INK_BLACK, (int(hx), int(hy)), int(s * 0.28), 2)
        _ink_stroke(surf, [(hx + s * 0.1, hy - s * 0.25), (hx + s * 0.25, hy - s * 0.5)], 11, 2)
        _ink_stroke(surf, [(cx - s, cy + s * 0.2), (cx - s * 1.4, cy - s * 0.2), (cx - s * 1.3, cy + s * 0.3)], 7, 2)
        if fam == "feline":
            _ink_stroke(surf, [(hx - s * 0.1, hy + s * 0.2), (hx - s * 0.15, hy + s * 0.55)], 3, 2)
    elif fam == "bird":
        _ink_stroke(surf, [(cx + s * 0.6, cy - s * 0.3), (cx + s * 1.0, cy - s * 0.2), (cx + s * 0.55, cy - s * 0.1)], 5, 2)
        for i in range(3):
            _ink_stroke(surf, [(cx - s * 0.3, cy - s * 0.2 + i * 6), (cx + s * 0.3, cy + i * 6)], 10 + i, 1)
        pygame.draw.circle(surf, INK_BLACK, (int(cx + s * 0.45), int(cy - s * 0.32)), 2)
    elif fam == "tusker":
        _ink_stroke(surf, [(cx + s * 0.8, cy + s * 0.2), (cx + s * 1.2, cy + s * 0.5)], 4, 3)
        _ink_stroke(surf, [(cx + s * 0.9, cy + s * 0.1), (cx + s * 1.15, cy + s * 0.35)], 6, 2)
    elif fam == "golem":
        # slab seams + two deep-set eye pits
        _ink_stroke(surf, [(cx - s * 0.7, cy), (cx + s * 0.7, cy - s * 0.05)], 51, 2)
        _ink_stroke(surf, [(cx - s * 0.5, cy + s * 0.35), (cx + s * 0.55, cy + s * 0.3)], 53, 2)
        for ex in (-0.07, 0.07):
            pygame.draw.circle(surf, INK_BLACK, (int(cx + s * ex * 2), int(cy - s * 0.68)), 2)
    elif fam == "monolith":
        # carved rune notches down the face
        for i in range(3):
            _ink_stroke(surf, [(cx - s * 0.15, cy - s * 0.4 + i * s * 0.35),
                               (cx + s * 0.15, cy - s * 0.32 + i * s * 0.35)], 55 + i, 2)
    elif fam == "armored":
        # overlapping plate arcs across the dome
        for i in range(3):
            pygame.draw.arc(surf, INK_BLACK,
                            (int(cx - s * 0.8 + i * s * 0.4), int(cy - s * 0.55),
                             int(s * 0.8), int(s * 0.9)), 0.4, 2.6, 2)
    elif fam == "amphibian":
        # a wide mouth line + a big round eye + splayed toes
        _ink_stroke(surf, [(cx + s * 0.2, cy - s * 0.1), (cx + s * 0.75, cy - s * 0.05)], 61, 2)
        pygame.draw.circle(surf, INK_BLACK, (int(cx + s * 0.35), int(cy - s * 0.35)), int(s * 0.1), 2)
        for i in range(3):
            _ink_stroke(surf, [(cx - s * 0.5 + i * 8, cy + s * 0.45),
                               (cx - s * 0.65 + i * 8, cy + s * 0.62)], 63 + i, 1)
    elif fam == "wisp":
        # inner flame curl + drifting sparks
        _ink_stroke(surf, [(cx - s * 0.1, cy + s * 0.4), (cx - s * 0.2, cy - s * 0.1),
                           (cx + s * 0.05, cy - s * 0.45)], 67, 2)
        for i in range(3):
            pygame.draw.circle(surf, INK_BLACK,
                               (int(cx + s * (0.4 + i * 0.12)), int(cy - s * (0.5 + i * 0.2))), 1)
    elif fam == "rabbit":
        # inner-ear lines, an eye, whiskers
        _ink_stroke(surf, [(cx - s * 0.18, cy - s * 0.5), (cx - s * 0.16, cy - s * 0.9)], 71, 1)
        _ink_stroke(surf, [(cx + s * 0.16, cy - s * 0.5), (cx + s * 0.18, cy - s * 0.95)], 73, 1)
        pygame.draw.circle(surf, INK_BLACK, (int(cx + s * 0.18), int(cy - s * 0.18)), 2)
        for i in range(2):
            _ink_stroke(surf, [(cx + s * 0.3, cy - s * 0.05 + i * 4),
                               (cx + s * 0.6, cy - s * 0.12 + i * 6)], 75 + i, 1)
    elif fam == "figure":
        # the wizard outfit: a wide hat brim with a bent tip…
        _ink_stroke(surf, [(cx - s * 0.55, cy - s * 0.62), (cx + s * 0.55, cy - s * 0.62)], 31, 3)
        _ink_stroke(surf, [(cx - s * 0.14, cy - s * 0.66), (cx + s * 0.05, cy - s * 1.15),
                           (cx + s * 0.38, cy - s * 1.32)], 33, 2)
        # …a gnarled staff with an orb at the crown…
        _ink_stroke(surf, [(cx + s * 0.62, cy + s * 0.85), (cx + s * 0.55, cy - s * 0.1),
                           (cx + s * 0.64, cy - s * 0.62)], 37, 2)
        pygame.draw.circle(surf, INK_BLACK, (int(cx + s * 0.63), int(cy - s * 0.7)),
                           max(2, int(s * 0.09)), 2)
        # …a rope belt at the waist, and a face kept in hood-shadow
        _ink_stroke(surf, [(cx - s * 0.3, cy + s * 0.06), (cx + s * 0.3, cy + s * 0.1)], 41, 2)
        pygame.draw.circle(surf, INK_BLACK, (int(cx), int(cy - s * 0.68)),
                           max(2, int(s * 0.1)))
    elif fam == "insect":
        for i in range(3):
            _ink_stroke(surf, [(cx - s * 0.2 + i * s * 0.3, cy + s * 0.3),
                                (cx - s * 0.4 + i * s * 0.3, cy + s * 0.7)], 20 + i, 1)
        _ink_stroke(surf, [(cx + s * 0.4, cy - s * 0.2), (cx + s * 0.7, cy - s * 0.6)], 9, 1)
    elif fam in ("fish", "cephalopod"):
        _ink_stroke(surf, [(cx - s, cy), (cx - s * 1.4, cy - s * 0.4), (cx - s * 1.4, cy + s * 0.4), (cx - s, cy)], 8, 2)
        for i in range(3):
            _ink_stroke(surf, [(cx + s * 0.3 + i * 5, cy + s * 0.3), (cx + s * 0.3 + i * 5, cy + s * 0.7)], 30 + i, 1)
    elif fam == "tree":
        _ink_stroke(surf, [(cx, cy + s * 0.2), (cx, cy + s * 0.8)], 3, 2)
        for i in range(4):
            ang = -1.2 + i * 0.6
            _ink_stroke(surf, [(cx, cy), (cx + math.cos(ang) * s * 0.8, cy + math.sin(ang) * s * 0.6)], 40 + i, 1)
    elif fam in ("monolith", "golem", "figure"):
        for i in range(4):
            yy = cy - s * 0.4 + i * s * 0.3
            _ink_stroke(surf, [(cx - s * 0.4, yy), (cx + s * 0.4, yy)], 50 + i, 1)
    # ink blotches + spatter over everything
    bl = pygame.Surface((w, h), pygame.SRCALPHA)
    _blotches(bl, w, h, hash(("blot", key)) & 0xffff, n=rr.randint(5, 8))
    surf.blit(bl, (0, 0))
    # faint vignette
    vig = pygame.Surface((w, h), pygame.SRCALPHA)
    for i in range(6):
        a = 8 + i * 5
        pygame.draw.rect(vig, (60, 46, 30, a), (i, i, w - 2 * i, h - 2 * i), 1)
    surf.blit(vig, (0, 0))
    _SKETCH_CACHE[ck] = surf
    return surf

RARITY_FRAME = {
    "common":    None,                    # plain ink border (most cards)
    "uncommon":  (150, 170, 130),         # soft green
    "rare":      (110, 150, 200),         # blue
    "epic":      (180, 120, 210),         # violet
    "legendary": (225, 180, 80),          # gold
}
def draw_card_border(surf, body, affordable, rarity="common"):
    """A decorative double border with corner flourishes, drawn in ink. Rare+
    cards get a coloured outer keyline (and a corner gem for epic/legendary) so
    prized cards read at a glance — subtle, in keeping with the parchment look."""
    col = shade(PAGE, 1.0 if affordable else 0.66)
    dark = shade(col, 0.42)
    rc = RARITY_FRAME.get(rarity)
    if rc is not None:
        if not affordable: rc = shade(rc, 0.7)
        pygame.draw.rect(surf, rc, body, 3, border_radius=8)          # coloured rim
        pygame.draw.rect(surf, shade(dark, 1.0), body.inflate(-4, -4), 1, border_radius=7)
    else:
        pygame.draw.rect(surf, dark, body, 3, border_radius=8)
    inner = body.inflate(-10, -10)
    pygame.draw.rect(surf, shade(col, 0.6), inner, 1, border_radius=6)
    fl = 12
    for corner in ("tl", "tr", "bl", "br"):
        if corner == "tl":
            cxp, cyp, dx, dy = inner.left, inner.top, 1, 1
        elif corner == "tr":
            cxp, cyp, dx, dy = inner.right, inner.top, -1, 1
        elif corner == "bl":
            cxp, cyp, dx, dy = inner.left, inner.bottom, 1, -1
        else:
            cxp, cyp, dx, dy = inner.right, inner.bottom, -1, -1
        pygame.draw.line(surf, dark, (cxp, cyp), (cxp + dx * fl, cyp), 2)
        pygame.draw.line(surf, dark, (cxp, cyp), (cxp, cyp + dy * fl), 2)
        pygame.draw.circle(surf, dark, (int(cxp + dx * fl * 0.5), int(cyp + dy * fl * 0.5)), 2)
    # epic / legendary: a small gem at the top-centre of the frame
    if rarity in ("epic", "legendary") and rc is not None:
        gx, gy = body.centerx, body.top + 3
        pygame.draw.circle(surf, rc, (gx, gy), 4)
        pygame.draw.circle(surf, shade(rc, 1.5), (gx - 1, gy - 1), 2)

def draw_card(surf, key, rect, font, sfont, affordable, cost_override=None):
    d = CARDS[key]
    body = pygame.Rect(rect)
    pygame.draw.rect(surf, (12, 8, 6), body.move(4, 6), border_radius=8)
    col = shade(PAGE, 1.0 if affordable else 0.66)
    pygame.draw.rect(surf, col, body, border_radius=8)
    # per-card colour identity: a faint wash of the creature's own paper colour
    # tints the name/cost bands so cards read apart at a glance without changing
    # the parchment look. Kept subtle (mostly desaturated toward the page).
    paper = d.get("paper", (170, 150, 110))
    if isinstance(paper, list): paper = tuple(paper)
    band = _mix(col, paper, 0.22 if affordable else 0.12)   # gentle tint
    # header band height (name) and footer band height (cost + sigil)
    head_h = 20
    foot_h = 22
    # ── art window fills the whole card *around* the name header and the
    #    cost/sigil footer, so the background art hugs those elements ──
    art = pygame.Rect(body.x + 6, body.y + head_h, body.w - 12, body.h - head_h - foot_h)
    if art.w > 8 and art.h > 8:
        sketch = make_card_sketch(key, art.w, art.h)
        if not affordable:
            sketch = sketch.copy(); sketch.set_alpha(150)
        clip = pygame.Surface((art.w, art.h), pygame.SRCALPHA)
        pygame.draw.rect(clip, (255, 255, 255, 255), (0, 0, art.w, art.h), border_radius=5)
        framed = sketch.copy()
        framed.blit(clip, (0, 0), special_flags=pygame.BLEND_RGBA_MULT)
        surf.blit(framed, art.topleft)
        pygame.draw.rect(surf, shade(col, 0.45), art, 1, border_radius=5)
    # ── decorative border (rarity-tinted for rare+ cards) ──
    rar = carddata.rarity(key) if CARDDATA_OK else "common"
    draw_card_border(surf, body, affordable, rar)
    # ── name header band (tinted by the card's colour) ──
    pygame.draw.rect(surf, band, (body.x + 2, body.y + 2, body.w - 4, head_h - 2),
                     border_top_left_radius=7, border_top_right_radius=7)
    # a thin colour keyline just under the header sells the identity subtly
    pygame.draw.line(surf, shade(paper, 0.7),
                     (body.x + 6, body.y + head_h - 1), (body.right - 6, body.y + head_h - 1), 1)
    nm = d["name"]
    name = sfont.render(nm, True, INK)
    # shrink to fit if the name is too wide for the header
    if name.get_width() > body.w - 12:
        tiny = _font(12)
        name = tiny.render(nm, True, INK)
    surf.blit(name, (body.centerx - name.get_width() / 2, body.y + 3))
    # ── footer band with sigil (bottom-left) + cost (bottom-right) ──
    pygame.draw.rect(surf, band, (body.x + 2, body.bottom - foot_h, body.w - 4, foot_h - 2),
                     border_bottom_left_radius=7, border_bottom_right_radius=7)
    # sigil seal — bottom-left, now bearing the ability's glyph (from the bank)
    sig = d["sigil"]
    sc2 = SIGIL_COLORS.get(sig, GOLD)
    sgx, sgy = body.x + 15, body.bottom - 13
    if sig and sig != "none" and SIGILS_OK:
        pygame.draw.circle(surf, (30, 24, 20), (sgx, sgy), 10)
        pygame.draw.circle(surf, sc2, (sgx, sgy), 9)
        pygame.draw.circle(surf, shade(sc2, 0.55), (sgx, sgy), 9, 2)
        SIGILS_MOD.draw_glyph(surf, SIGILS_MOD.glyph_for(sig), sgx, sgy, 6, shade(sc2, 0.35))
    else:
        pygame.draw.circle(surf, sc2, (sgx, sgy), 9)
        pygame.draw.circle(surf, shade(sc2, 0.6), (sgx, sgy), 9, 2)
        pygame.draw.circle(surf, shade(sc2, 1.35), (sgx - 3, sgy - 3), 3)
    # cost seal — bottom-right corner (ringed by cost-tier colour: cheap→green,
    # mid→amber, pricey→red — a fast at-a-glance read of what a card costs)
    cval = cost_override if cost_override is not None else d["cost"]
    tier = (90, 170, 90) if cval <= 3 else ((210, 175, 70) if cval <= 6 else (200, 90, 70))
    cgx, cgy = body.right - 15, body.bottom - 13
    pygame.draw.circle(surf, INK, (cgx, cgy), 11)
    pygame.draw.circle(surf, tier, (cgx, cgy), 11, 2)
    cost = font.render(str(cval), True, PAGE)
    surf.blit(cost, (cgx - cost.get_width() / 2, cgy - cost.get_height() / 2))

BOARD_STATIC = None

def draw_hazards(screen, g, tnow):
    # lava: glowing molten band creeping in from the far (enemy) edge
    if HAZARD == "lava" and g.lava > 0:
        edge_y = BOOK.top + BOOK.h * g.lava
        band = quad(BOOK.left, BOOK.top, BOOK.right, edge_y)
        pygame.draw.polygon(screen, (48, 16, 12), band)
        for i in range(6):
            yy = lerp(BOOK.top + 6, edge_y, i / 6)
            pts = []
            for xx in range(BOOK.left, BOOK.right, 18):
                px_, py_, _ = proj(xx, yy)
                pts.append((px_, py_ + math.sin(xx * 0.04 + tnow * 3 + i) * 3))
            glow = (240, 120 + int(40 * math.sin(tnow * 4 + i)), 40)
            pygame.draw.lines(screen, glow, False, pts, 2)
        # molten shoreline
        sh = [proj(BOOK.left, edge_y)[:2], proj(BOOK.right, edge_y)[:2]]
        pygame.draw.line(screen, (255, 180, 80), sh[0], sh[1], 3)
    # fissures: jagged dark cracks that open and reseal
    if HAZARD == "fissures":
        for fz in g.fissures:
            if fz["open"] <= 0.01: continue
            cx, cy, _ = proj(fz["x"], fz["y"])
            w = 30 * fz["open"]
            rr = random.Random(int(fz["x"]))
            pts_top, pts_bot = [], []
            for i in range(7):
                fx = fz["x"] - 34 + i * 11
                jitter = rr.uniform(-6, 6)
                p = proj(fx, fz["y"] + jitter)
                pts_top.append((p[0], p[1] - w))
                pts_bot.append((p[0], p[1] + w))
            poly = pts_top + pts_bot[::-1]
            pygame.draw.polygon(screen, (10, 8, 12), poly)
            pygame.draw.lines(screen, (60, 30, 40), False, pts_top, 2)
            if fz["state"] == "open":
                pygame.draw.line(screen, (150, 60, 40), (cx - w, cy), (cx + w, cy), 1)
    # ink tubs POI
    if POI == "inktubs":
        for tub in g.tubs:
            cx, cy, sc = proj(tub["x"], tub["y"])
            r = int(40 * sc)
            pygame.draw.ellipse(screen, (30, 40, 44), (cx - r, cy - r * 0.5, r * 2, r))
            pygame.draw.ellipse(screen, (12, 18, 22), (cx - r * 0.8, cy - r * 0.4, r * 1.6, r * 0.8))
            # ink surface with capture tint
            oc = (60, 130, 120) if tub["owner"] == 0 else (150, 60, 150) if tub["owner"] == 1 else (40, 60, 60)
            surf_r = r * 0.7
            pygame.draw.ellipse(screen, oc, (cx - surf_r, cy - surf_r * 0.5, surf_r * 2, surf_r))
            # capture ring
            if tub["prog"] > 0:
                pygame.draw.arc(screen, shade(oc, 1.5),
                                (cx - r, cy - r * 0.7, r * 2, r * 1.4),
                                0, tub["prog"] * math.tau, 3)

    # dunes: quicksand pits
    if HAZARD == "quicksand":
        for qs in g.quicksand:
            cx, cy, sc = proj(qs["x"], qs["y"])
            r = int(qs["r"] * sc)
            pygame.draw.ellipse(screen, (170, 140, 90), (cx - r, cy - r * 0.5, r * 2, r))
            pygame.draw.ellipse(screen, (140, 112, 70), (cx - r * 0.7, cy - r * 0.35, r * 1.4, r * 0.7))
            # slow swirl rings
            for k in range(3):
                rr = r * (0.3 + 0.22 * k)
                pygame.draw.arc(screen, (120, 96, 60),
                                (cx - rr, cy - rr * 0.5, rr * 2, rr),
                                tnow + k, tnow + k + 2.4, 2)
    # clockwork: rotating gears
    if HAZARD == "gears":
        for gr in g.gears:
            cx, cy, sc = proj(gr["x"], gr["y"])
            r = int(gr["r"] * sc)
            pygame.draw.circle(screen, (90, 70, 40), (int(cx), int(cy)), r)
            pygame.draw.circle(screen, (150, 120, 70), (int(cx), int(cy)), r, 3)
            for k in range(10):
                a = gr["ang"] + k * math.tau / 10
                tx, ty = cx + math.cos(a) * r, cy + math.sin(a) * r * 0.6
                pygame.draw.circle(screen, (150, 120, 70), (int(tx), int(ty)), max(2, int(4 * sc)))
            pygame.draw.circle(screen, (60, 46, 28), (int(cx), int(cy)), int(r * 0.3))
    # sandbar: a translucent tide band that breathes over the river
    if HAZARD == "tide":
        band = RIVER_H / 2 + g.tide * 46
        q = quad(BOOK.left, RIVER_Y - band, BOOK.right, RIVER_Y + band)
        surf = pygame.Surface((W, H), pygame.SRCALPHA)
        a = int(70 + 60 * g.tide)
        pygame.draw.polygon(surf, (120, 190, 205, a), q)
        screen.blit(surf, (0, 0))
        # foam line along the tide edge
        for edge in (RIVER_Y - band, RIVER_Y + band):
            pts = []
            for xx in range(BOOK.left, BOOK.right, 14):
                px_, py_, _ = proj(xx, edge)
                pts.append((px_, py_ + math.sin(xx * 0.05 + tnow * 3) * 2))
            pygame.draw.lines(screen, (235, 248, 250), False, pts, 2)
    # outcrop: cascading waterfall columns falling down the page
    if HAZARD == "waterfalls":
        for fa in g.falls:
            for yy in range(int(BOOK.top), int(RIVER_Y), 10):
                px_, py_, sc = proj(fa["x"], yy)
                w = int(fa["w"] * sc)
                off = math.sin(yy * 0.1 + tnow * 6) * 2
                col = (150 + int(40 * math.sin(yy * 0.2 + tnow * 5)), 200, 225)
                pygame.draw.line(screen, col, (px_ - w / 2 + off, py_), (px_ + w / 2 + off, py_), 3)
            # splash pool at the river
            px_, py_, sc = proj(fa["x"], RIVER_Y)
            for k in range(5):
                rr = int((6 + k * 5) * sc)
                pygame.draw.circle(screen, (200, 230, 240), (int(px_), int(py_)), rr, 1)

def draw_fog(screen, g, tnow):
    """Fog of war: cover the enemy half in shadow except within vision circles
    around the player's own units and towers. Purely visual."""
    fog = pygame.Surface((W, H), pygame.SRCALPHA)
    # cover the top (enemy) half of the projected board
    top = quad(BOOK.left, BOOK.top, BOOK.right, RIVER_Y)
    pygame.draw.polygon(fog, (6, 8, 14, 205), top)
    # punch vision holes for player's units + towers (team 0)
    for e in list(g.units) + list(g.towers):
        if getattr(e, "team", 1) != 0 or not getattr(e, "alive", True):
            continue
        sx, sy, sc = proj(e.x, e.y)
        vr = int((150 if isinstance(e, Tower) else 120) * sc)
        # soft-edged hole: erase alpha with a radial of transparent circles
        for rr in range(vr, 0, -6):
            a = int(205 * (rr / vr))
            pygame.draw.circle(fog, (0, 0, 0, 0), (int(sx), int(sy)), rr)
    screen.blit(fog, (0, 0))

def draw_weather(screen, g, tnow):
    if WEATHER == "lightning":
        if g.flash > 0.02:
            fl = pygame.Surface((W, H), pygame.SRCALPHA)
            fl.fill((180, 200, 240, int(90 * g.flash)))
            screen.blit(fl, (0, 0))
            if g.flash > 0.6:      # draw a jagged bolt on strong flashes
                rr = random.Random(int(tnow * 7))
                bx = rr.randint(int(BOOK.left), int(BOOK.right))
                pts = [(bx, 0)]
                y = 0
                while y < BOOK.top + 120:
                    y += rr.randint(20, 44); bx += rr.randint(-30, 30)
                    pts.append((bx, y))
                pygame.draw.lines(screen, (230, 240, 255), False, pts, 2)
        # rain streaks
        for i in range(70):
            rr = random.Random(i * 3)
            rx = (rr.uniform(0, W) + tnow * 120 * rr.uniform(0.5, 1)) % W
            ry = (rr.uniform(0, H) + tnow * 700 * rr.uniform(0.5, 1)) % H
            pygame.draw.line(screen, (120, 140, 180), (rx, ry), (rx - 3, ry + 14), 1)
    elif WEATHER == "embers":
        for i in range(46):
            rr = random.Random(i * 5)
            ex = (rr.uniform(BOOK.left, BOOK.right) + math.sin(tnow * 0.7 + i) * 20)
            ey = BOOK.bottom - ((tnow * rr.uniform(40, 90) + rr.uniform(0, 600)) % (BOOK.h + 60))
            a = 0.5 + 0.5 * math.sin(tnow * 5 + i)
            pygame.draw.circle(screen, (255, int(140 + 80 * a), 40), (int(ex), int(ey)), rr.randint(1, 2))
    elif WEATHER == "snow":
        for i in range(80):
            rr = random.Random(i * 4)
            sx = (rr.uniform(0, W) + math.sin(tnow * 0.6 + i) * 24) % W
            sy = (rr.uniform(0, H) + tnow * rr.uniform(30, 70)) % H
            pygame.draw.circle(screen, (220, 230, 244), (int(sx), int(sy)), rr.randint(1, 2))
    elif WEATHER == "aurora":
        # rippling ribbons of light across the top of the page
        aur = pygame.Surface((W, H), pygame.SRCALPHA)
        for band in range(3):
            base = BOOK.top + 20 + band * 34
            col = [(90, 220, 170), (150, 120, 220), (90, 180, 230)][band]
            pts = []
            for xx in range(BOOK.left, BOOK.right, 12):
                yy = base + math.sin(xx * 0.02 + tnow * (1.2 + band * 0.4)) * 20
                pts.append((xx, yy))
            for k, (px_, py_) in enumerate(pts):
                a = int(60 + 40 * math.sin(tnow * 2 + k * 0.3 + band))
                pygame.draw.line(aur, (*col, max(0, a)), (px_, py_), (px_, py_ + 60), 3)
        screen.blit(aur, (0, 0))
    elif WEATHER == "sand":
        # low blowing sand streaks drifting sideways
        for i in range(70):
            rr = random.Random(i * 6)
            sx = (rr.uniform(0, W) + tnow * rr.uniform(120, 260)) % W
            sy = BOOK.top + rr.uniform(0, BOOK.h)
            a = 120 + int(80 * math.sin(tnow * 3 + i))
            surf = pygame.Surface((14, 2), pygame.SRCALPHA)
            surf.fill((210, 180, 120, max(0, min(200, a))))
            screen.blit(surf, (sx, sy))
    elif WEATHER == "sandstorm":
        # heavy diagonal sand sheets + a warm haze that pulses across the page
        haze = pygame.Surface((W, H), pygame.SRCALPHA)
        ha = int(40 + 30 * math.sin(tnow * 0.8))
        haze.fill((214, 176, 120, ha))
        screen.blit(haze, (0, 0))
        for i in range(150):
            rr = random.Random(i * 3)
            spd = rr.uniform(300, 620)
            sx = (rr.uniform(0, W * 1.4) - tnow * spd) % (W * 1.4) - W * 0.2
            sy = (rr.uniform(0, H) + tnow * spd * 0.3) % H
            ln = rr.randint(16, 40)
            a = 90 + int(90 * math.sin(tnow * 4 + i))
            s = pygame.Surface((ln, 2), pygame.SRCALPHA)
            s.fill((230, 200, 140, max(0, min(220, a))))
            screen.blit(s, (sx, sy))
    elif WEATHER == "gulls":
        # gentle sunny beach: drifting gulls (V shapes) + sparkle glints
        for i in range(6):
            rr = random.Random(i * 11)
            gx = (rr.uniform(BOOK.left, BOOK.right) + tnow * rr.uniform(20, 50) * (1 if i % 2 else -1)) % (BOOK.right - BOOK.left) + BOOK.left
            gy = BOOK.top + 30 + rr.uniform(0, 120) + math.sin(tnow * 1.2 + i) * 8
            fl = math.sin(tnow * 4 + i) * 4
            pygame.draw.lines(screen, (60, 60, 70), False,
                              [(gx - 8, gy + fl), (gx, gy - 2), (gx + 8, gy + fl)], 2)
        for i in range(24):
            rr = random.Random(i * 7 + 3)
            sx = rr.uniform(BOOK.left, BOOK.right)
            sy = RIVER_Y + rr.uniform(-30, 30)
            if (math.sin(tnow * 3 + i) > 0.6):
                px_, py_, _ = proj(sx, sy)
                pygame.draw.circle(screen, (255, 255, 240), (int(px_), int(py_)), 2)
    elif WEATHER == "mist":
        # cool mountain mist: soft horizontal fog bands rolling upward
        for band in range(4):
            surf = pygame.Surface((W, 60), pygame.SRCALPHA)
            yy = (BOOK.bottom - band * 150 - tnow * 18) % (BOOK.h + 200) + BOOK.top - 100
            a = int(46 + 24 * math.sin(tnow * 0.6 + band))
            surf.fill((210, 224, 228, max(0, a)))
            screen.blit(surf, (0, yy))

def draw_cartoon_smear(screen, u):
    """Traditional-animation 'smear frame': when a unit has moved a lot since the
    last held pose (we render on twos at 12fps), bridge the gap with a tapered
    streak plus a couple of ghost multiples in the unit's paper colour. Slow or
    idle units barely smear; fast ones leave a satisfying whip. Purely visual."""
    crx = getattr(u, "_crx", u.x); cry = getattr(u, "_cry", u.y)
    sx, sy, sc = proj(u.x, u.y)
    sx0, sy0, _ = proj(crx, cry)
    dx, dy = sx - sx0, sy - sy0
    d = math.hypot(dx, dy)
    r = max(6, u.size * sc * unit_vis_scale() * 0.72)
    if d < r * 0.35:        # not enough motion to warrant a smear
        return
    col = shade(getattr(u, "paper", PAGE), 0.92)
    hi = shade(col, 1.35)
    smear = pygame.Surface((W, H), pygame.SRCALPHA)
    nx, ny = (-dy / d, dx / d)          # perpendicular to motion
    # extend the trail slightly past the old pose for a whippier feel
    tail = (sx0 - dx * 0.35, sy0 - dy * 0.35)
    w0 = r * 1.05                        # trailing width (fat)
    poly = [(tail[0] + nx * w0, tail[1] + ny * w0),
            (tail[0] - nx * w0, tail[1] - ny * w0),
            (sx - nx * r * 0.2, sy - ny * r * 0.2),
            (sx + nx * r * 0.2, sy + ny * r * 0.2)]
    pygame.draw.polygon(smear, (*col, 130), poly)
    # ghost multiples along the path — the classic "multiple exposure" look
    ghosts = min(4, int(d / (r * 0.7)) + 2)
    for k in range(1, ghosts + 1):
        tt = k / (ghosts + 1)
        gx = lerp(tail[0], sx, tt); gy = lerp(tail[1], sy, tt)
        a = int(140 * (1 - tt) ** 0.8)
        pygame.draw.circle(smear, (*col, a), (int(gx), int(gy)), int(r * (0.55 + 0.45 * tt)))
    # bright speed lines trailing the leading edge (ink-pen streaks)
    for s in (-1, 1):
        lx0 = tail[0] + nx * r * 0.7 * s; ly0 = tail[1] + ny * r * 0.7 * s
        pygame.draw.line(smear, (*hi, 170),
                         (lx0, ly0), (sx + nx * r * 0.15 * s, sy + ny * r * 0.15 * s), 2)
    pygame.draw.line(smear, (*hi, 130), tail, (sx, sy), 2)
    screen.blit(smear, (0, 0))

def draw_caches(screen, g, tnow):
    """Wax-sealed card caches on the enemy page — a glowing envelope of folded
    paper bound in red wax. Cracks show as they take damage."""
    for cache in getattr(g, "caches", []):
        if cache["cracked"]:
            continue
        cx, cy, sc = proj(cache["x"], cache["y"])
        r = int(22 * sc)
        frac = max(0.0, cache["hp"] / cache["maxhp"])
        # soft glow pulse
        glow = pygame.Surface((r * 6, r * 6), pygame.SRCALPHA)
        ga = int(60 + 40 * math.sin(tnow * 3 + cx))
        pygame.draw.circle(glow, (230, 190, 90, ga), (r * 3, r * 3), r * 2)
        screen.blit(glow, (cx - r * 3, cy - r * 3))
        # folded-paper envelope
        pts = [(cx, cy - r), (cx + r, cy), (cx, cy + r), (cx - r, cy)]
        pygame.draw.polygon(screen, (222, 206, 170), pts)
        pygame.draw.polygon(screen, (150, 120, 70), pts, 2)
        pygame.draw.line(screen, (150, 120, 70), (cx - r, cy), (cx, cy - r * 0.2), 2)
        pygame.draw.line(screen, (150, 120, 70), (cx + r, cy), (cx, cy - r * 0.2), 2)
        # wax seal, cracking as hp drops
        seal = (150, 36, 42) if frac > 0.5 else (110, 24, 30)
        pygame.draw.circle(screen, seal, (cx, cy), int(r * 0.42))
        if frac < 0.75:
            for k in range(3):
                a = tnow + k
                pygame.draw.line(screen, (60, 12, 16),
                                 (cx, cy), (cx + math.cos(a) * r * 0.4, cy + math.sin(a) * r * 0.4), 1)

def draw_table_finds(screen, g, tnow, sfont):
    """Cards drifting at the desk edges the player can tap to collect."""
    HOT["tablefinds"] = []
    for tf in getattr(g, "table_finds", []):
        x, y = int(tf["x"]), int(tf["y"])
        blink = 0.6 + 0.4 * math.sin(tnow * 4 + tf["phase"])
        fade = min(1.0, tf["life"] / 3.0)          # fade out near the end
        card = pygame.Surface((64, 84), pygame.SRCALPHA)
        card.fill((0, 0, 0, 0))
        rect = pygame.Rect(0, 0, 64, 84)
        pygame.draw.rect(card, (*shade(PAGE, 1.0), int(230 * fade)), rect, border_radius=8)
        pygame.draw.rect(card, (*GOLD, int(255 * fade * blink)), rect, 3, border_radius=8)
        q = sfont.render("?", True, INK)
        card.blit(q, (32 - q.get_width() / 2, 30))
        screen.blit(card, (x - 32, y - 42))
        lbl = sfont.render("tap!", True, shade(GOLD, 1.2))
        screen.blit(lbl, (x - lbl.get_width() / 2, y + 46))
        HOT["tablefinds"].append((pygame.Rect(x - 32, y - 42, 64, 84), tf))

def draw_found_toasts(screen, g, font, sfont):
    """Slide-in banners for cards just discovered (cache/table/forest)."""
    finds = getattr(g, "found_cards", [])
    if not finds:
        return
    # only show the newest; give it a lifetime via a timer on the grant dict
    grant = finds[0]
    grant.setdefault("_t", 0.0)
    grant["_t"] += DT if False else 1 / 60.0     # advanced by render cadence
    key = grant["key"]
    card = CARDS.get(key, {})
    rr = carddata.RARITY_ORDER if CARDDATA_OK else ["common"]
    rar = carddata.rarity(key) if CARDDATA_OK else "common"
    rcol = {"common": (170, 160, 150), "uncommon": (110, 170, 120),
            "rare": (90, 140, 210), "epic": (180, 110, 210),
            "legendary": (230, 180, 70)}.get(rar, (180, 170, 150))
    w, h = 320, 92
    x = W - w - 20
    y = 70
    panel = pygame.Surface((w, h), pygame.SRCALPHA)
    pygame.draw.rect(panel, (18, 14, 12, 235), panel.get_rect(), border_radius=12)
    pygame.draw.rect(panel, (*rcol, 255), panel.get_rect(), 3, border_radius=12)
    head = font.render("NEW CARD!" if grant.get("new") else "CARD FOUND", True, GOLD)
    panel.blit(head, (16, 10))
    nm = font.render(card.get("name", key)[:26], True, PAGE)
    panel.blit(nm, (16, 38))
    sub = sfont.render(f"{rar} · via {grant.get('how','found')}", True, rcol)
    panel.blit(sub, (16, 64))
    screen.blit(panel, (x, y))
    # expire the toast after ~2.5s of render time
    if grant["_t"] > 2.5:
        finds.pop(0)

def draw_page_effects(screen, g, tnow):
    """Dynamic paper life: a corner that lifts and flutters in the wind now and
    then, occasional frayed edge wisps, and per-map 'climate' treatment — dry
    maps look parched (warm, curling), cold maps look stiff (bluish, frosted)."""
    dry = CUR_ARENA in ("volcano", "dunes", "sandstorm", "forge")
    cold = CUR_ARENA in ("frost", "aurora", "observatory")
    # ── occasional wind gust lifting the bottom-right page corner ──
    gust = math.sin(tnow * 0.5) * 0.5 + math.sin(tnow * 1.7 + 1) * 0.5
    gust = max(0.0, gust)                      # only lifts, never pushes down
    if dry:
        gust = max(gust, 0.4 + 0.3 * math.sin(tnow * 2.3))   # dry pages curl more
    if cold:
        gust *= 0.35                            # stiff pages barely move
    lift = gust * 26
    cxr, cyr = BOOK.right - 4, BOOK.bottom - 4
    if lift > 1:
        p1 = (cxr, cyr)
        p2 = (cxr - 70 - lift * 0.4, cyr - 6)
        p3 = (cxr - 8, cyr - 60 - lift)
        fold = shade(PAGE, 1.12 + 0.15 * gust)
        pygame.draw.polygon(screen, fold, [p1, p2, p3])
        pygame.draw.polygon(screen, shade(PAGE, 0.7), [p1, p2, p3], 2)
        sh = pygame.Surface((W, H), pygame.SRCALPHA)
        pygame.draw.polygon(sh, (10, 8, 6, int(60 * gust)),
                            [(p2[0], p2[1] + 8), (p3[0], p3[1] + 8), (p1[0], p1[1])])
        screen.blit(sh, (0, 0))
    # ── frayed edge wisps: a few tiny fibers flicking off an edge now and then ──
    fr = random.Random(int(tnow * 2))
    for _ in range(3):
        if fr.random() < 0.5:
            edge = fr.choice("tb")
            ex = fr.randint(BOOK.left + 40, BOOK.right - 40)
            ey = BOOK.top if edge == "t" else BOOK.bottom
            wob = math.sin(tnow * 6 + ex) * 3
            pygame.draw.line(screen, shade(PAGE, 1.15),
                             (ex, ey), (ex + wob, ey + (-4 if edge == "t" else 4)), 1)
    # ── climate wash over the whole page ──
    if dry or cold:
        wash = pygame.Surface((W, H), pygame.SRCALPHA)
        q = quad(BOOK.left, BOOK.top, BOOK.right, BOOK.bottom)
        pygame.draw.polygon(wash, (150, 90, 30, 26) if dry else (120, 150, 190, 30), q)
        screen.blit(wash, (0, 0))
        if cold:
            fr2 = random.Random(7)
            for _ in range(40):
                fx = fr2.randint(BOOK.left, BOOK.right)
                fy = fr2.choice([BOOK.top + fr2.randint(0, 40), BOOK.bottom - fr2.randint(0, 40)])
                a = 60 + int(30 * math.sin(tnow + fx))
                frs = pygame.Surface((3, 3), pygame.SRCALPHA)
                pygame.draw.circle(frs, (210, 230, 245, max(0, min(255, a))), (1, 1), 1)
                screen.blit(frs, (fx, fy))

def apply_shockwaves(screen, g):
    """Cheap radial ripple: for each active shockwave, grab an annular band of the
    frame and redraw it scaled slightly outward, making a lens-like pulse ring.
    Hi-gfx only; purely visual."""
    if not gfx_hi():
        return
    for sw in getattr(g, "shockwaves", []):
        prog = sw["t"] / sw["dur"]
        if prog >= 1.0:
            continue
        cx, cy, _ = proj(sw["x"], sw["y"])
        radius = int(40 + prog * 320)
        mag = sw["mag"] * (1.0 - prog)
        if radius < 6 or mag < 0.5:
            continue
        band = int(28 + mag)
        r_out = radius + band
        x0 = int(max(0, cx - r_out)); y0 = int(max(0, cy - r_out))
        x1 = int(min(W, cx + r_out)); y1 = int(min(H, cy + r_out))
        w, h = x1 - x0, y1 - y0
        if w <= 2 or h <= 2:
            continue
        try:
            patch = screen.subsurface((x0, y0, w, h)).copy()
        except ValueError:
            continue
        # scale the patch up by a hair to push the ring outward, then blit back
        sc = 1.0 + (mag / max(1, radius)) * 1.6
        bw, bh = int(w * sc), int(h * sc)
        big = pygame.transform.smoothscale(patch, (bw, bh))
        # The patch was copied from the DISPLAY surface, which has NO per-pixel
        # alpha. BLEND_RGBA_MULT with the ring's transparent interior (0,0,0,0)
        # would multiply the RGB to black while leaving the pixels opaque — the
        # "black circle on card placement" bug in hi-gfx. Convert to a per-pixel
        # alpha surface first so the masked interior becomes transparent instead.
        if not (big.get_flags() & pygame.SRCALPHA):
            big = big.convert_alpha()
        # mask to a ring so only the annulus is displaced (soft alpha ring)
        ring = pygame.Surface((bw, bh), pygame.SRCALPHA)
        ccx, ccy = bw // 2, bh // 2
        pygame.draw.circle(ring, (255, 255, 255, 200), (ccx, ccy), int((radius + band) * sc))
        pygame.draw.circle(ring, (0, 0, 0, 0), (ccx, ccy), int(radius * sc * 0.86))
        big.blit(ring, (0, 0), special_flags=pygame.BLEND_RGBA_MULT)
        screen.blit(big, (int(cx - bw / 2), int(cy - bh / 2)))
        # a faint bright rim to sell the energy
        pygame.draw.circle(screen, (255, 240, 210), (int(cx), int(cy)), radius, 2)

def _update_event_particles(dt):
    """Spawn & advance the display-only seasonal ambient overlay (snow, leaves,
    bats…). Purely cosmetic — never read by the simulation, so it's safe in net
    play. Density adapts to the battery-saver setting."""
    if not ACTIVE_EVENT or not EVENTS_OK:
        if EVENT_PARTICLES: EVENT_PARTICLES.clear()
        return
    spec = EVENTS_MOD.ambient_spec(ACTIVE_EVENT)
    if not spec:
        return
    target = spec.get("count", 0)
    if BATTERY_SAVER:
        target = target * 2 // 3
    while len(EVENT_PARTICLES) < target:
        _spawn_event_particle(spec)
    for p in EVENT_PARTICLES:
        p["t"] += dt
        p["y"] += p["vy"] * dt
        p["x"] += p["vx"] * dt + (math.sin(p["t"] * 1.6 + p["seed"]) * p["sway"] * dt if p.get("sway") else 0)
        if p.get("spin_v"):
            p["ang"] += p["spin_v"] * dt
        if p["vy"] >= 0 and p["y"] > H + 20:
            _respawn_event_particle(p, spec)
        elif p["vy"] < 0 and p["y"] < -20:
            _respawn_event_particle(p, spec)
        if p["x"] < -30: p["x"] = W + 20
        elif p["x"] > W + 30: p["x"] = -20
    if len(EVENT_PARTICLES) > target and not ACTIVE_EVENT.get("particles") == "fireworks":
        del EVENT_PARTICLES[target:]


def _spawn_event_particle(spec):
    import random as _r
    lo, hi = spec.get("fall", (18, 40))
    rise = spec.get("rise")
    EVENT_PARTICLES.append(dict(
        x=_r.uniform(0, W),
        y=_r.uniform(-40, H) if not rise else _r.uniform(0, H + 40),
        vy=(-_r.uniform(abs(lo), abs(hi)) if rise else _r.uniform(max(1, lo), max(2, hi))),
        vx=_r.uniform(-1, 1) * spec.get("drift", 12),
        sz=_r.uniform(*spec.get("size", (2, 4))),
        ang=_r.uniform(0, 6.28), spin_v=(_r.uniform(-2, 2) if spec.get("spin") else 0),
        sway=spec.get("drift", 12) if spec.get("sway") else 0,
        seed=_r.uniform(0, 6.28), t=0.0,
        col=spec.get("color") or _rainbow_col(),
        shape=spec.get("shape", "dot"), glow=spec.get("glow", False)))


def _respawn_event_particle(p, spec):
    import random as _r
    p["y"] = -20 if not spec.get("rise") else H + 20
    p["x"] = _r.uniform(0, W); p["t"] = 0.0
    if spec.get("rainbow"): p["col"] = _rainbow_col()


def _rainbow_col():
    import random as _r
    return _r.choice([(230, 90, 90), (230, 190, 80), (110, 200, 110),
                      (90, 160, 230), (190, 120, 210)])


def draw_event_overlay(screen):
    """Draw the seasonal ambient particles over the scene. Cheap shapes only."""
    if not ACTIVE_EVENT or not EVENT_PARTICLES:
        return
    for p in EVENT_PARTICLES:
        x, y, s, col = int(p["x"]), int(p["y"]), max(1, int(p["sz"])), p["col"]
        sh = p["shape"]
        if sh in ("dot", "mote"):
            if p.get("glow"):
                gs = pygame.Surface((s * 6, s * 6), pygame.SRCALPHA)
                pygame.draw.circle(gs, (*col, 60), (s * 3, s * 3), s * 3)
                screen.blit(gs, (x - s * 3, y - s * 3))
            pygame.draw.circle(screen, col, (x, y), s)
        elif sh == "rect":
            r = pygame.Surface((s * 2, s), pygame.SRCALPHA)
            r.fill((*col, 220)); r = pygame.transform.rotate(r, math.degrees(p["ang"]))
            screen.blit(r, (x, y))
        elif sh == "leaf":
            lf = pygame.Surface((s * 2, s * 3), pygame.SRCALPHA)
            pygame.draw.ellipse(lf, (*col, 230), (0, 0, s * 2, s * 3))
            lf = pygame.transform.rotate(lf, math.degrees(p["ang"]))
            screen.blit(lf, (x - lf.get_width() // 2, y - lf.get_height() // 2))
        elif sh == "heart":
            _draw_mini_heart(screen, x, y, s, col)
        elif sh == "bat":
            _draw_mini_bat(screen, x, y, s, col, p["t"])
        elif sh == "clover":
            for dx, dy in ((-s, 0), (s, 0), (0, -s), (0, s)):
                pygame.draw.circle(screen, col, (x + dx, y + dy), max(1, s // 2 + 1))
        elif sh == "lantern":
            if p.get("glow"):
                gs = pygame.Surface((s * 5, s * 5), pygame.SRCALPHA)
                pygame.draw.circle(gs, (*col, 70), (int(s * 2.5), int(s * 2.5)), s * 2)
                screen.blit(gs, (x - int(s * 2.5), y - int(s * 2.5)))
            pygame.draw.ellipse(screen, col, (x - s, y - s, s * 2, int(s * 2.4)))
        else:
            pygame.draw.circle(screen, col, (x, y), s)
    if ACTIVE_EVENT.get("particles") == "fireworks" and random.random() < 0.04:
        _spawn_firework()


def _draw_mini_heart(screen, x, y, s, col):
    pygame.draw.circle(screen, col, (x - s // 2, y), max(1, s // 2))
    pygame.draw.circle(screen, col, (x + s // 2, y), max(1, s // 2))
    pygame.draw.polygon(screen, col, [(x - s, y), (x + s, y), (x, y + s)])


def _draw_mini_bat(screen, x, y, s, col, t):
    flap = math.sin(t * 8) * s * 0.4
    pygame.draw.circle(screen, col, (x, y), max(1, int(s * 0.4)))
    pygame.draw.polygon(screen, col, [(x, y), (x - s, int(y - flap)), (x - s, int(y + s * 0.3))])
    pygame.draw.polygon(screen, col, [(x, y), (x + s, int(y - flap)), (x + s, int(y + s * 0.3))])


def _spawn_firework():
    import random as _r
    cx, cy = _r.uniform(W * 0.2, W * 0.8), _r.uniform(H * 0.1, H * 0.4)
    col = _rainbow_col()
    for _ in range(14):
        a = _r.uniform(0, 6.28); v = _r.uniform(40, 110)
        EVENT_PARTICLES.append(dict(
            x=cx, y=cy, vx=math.cos(a) * v, vy=math.sin(a) * v,
            sz=_r.uniform(2, 3), ang=0, spin_v=0, sway=0, seed=0, t=0.0,
            col=col, shape="dot", glow=True))


def _draw_sigil_badge(screen, u):
    """Float a small rounded badge with the unit's ability glyph above it. An
    accessibility aid — see what a creature does without opening its card. Purely
    cosmetic; skips units with no special ability."""
    sig = getattr(u, "sigil", None)
    if not sig or sig == "none":
        return
    sx, sy, sc = proj(u.x, u.y)
    s = max(6, u.size * sc * unit_vis_scale())
    r = max(7, int(s * 0.42))                     # badge radius scales with unit
    bx = int(sx)
    by = int(sy - s * 1.7 - r)                    # sit above the head
    col = SIGIL_COLORS.get(sig, (200, 170, 90))
    # rounded ink-paper badge
    pad = r + 4
    badge = pygame.Surface((pad * 2, pad * 2), pygame.SRCALPHA)
    pygame.draw.circle(badge, (30, 24, 20, 210), (pad, pad), r + 2)
    pygame.draw.circle(badge, (*shade(col, 0.55), 235), (pad, pad), r + 2, 2)
    pygame.draw.circle(badge, (238, 230, 214, 235), (pad, pad), r)
    SIGILS_MOD.draw_glyph(badge, SIGILS_MOD.glyph_for(sig), pad, pad, r * 0.72,
                          shade(col, 0.9))
    screen.blit(badge, (bx - pad, by - pad))
    # a tiny stem connecting badge to the creature
    pygame.draw.line(screen, (30, 24, 20, 180), (bx, by + r), (bx, int(sy - s * 1.5)), 1)


def render(screen, g, font, bfont, sfont):
    global BOARD_STATIC
    if BOARD_STATIC is None:
        BOARD_STATIC = build_board_static()
    tnow = pygame.time.get_ticks() * 0.001
    screen.blit(BOARD_STATIC, (0, 0))

    # dynamic ink: drifting current lines + candle glints
    for i in range(7):
        wy = RIVER_Y - RIVER_H // 2 + 6 + i * 7
        pts = []
        for xx in range(BOOK.left, BOOK.right, 16):
            px_, py_, _ = proj(xx, wy)
            pts.append((px_, py_ + math.sin(xx * 0.03 + tnow * 2 + i) * 2.5))
        pygame.draw.lines(screen, RIVER_HI if i % 2 else shade(RIVER, 1.5), False, pts, 1)
    for i in range(14):
        gx = random.Random(i).uniform(BOOK.left + 30, BOOK.right - 30)
        gy = RIVER_Y + random.Random(i + 50).uniform(-18, 18)
        px_, py_, _ = proj(gx, gy)
        a = 0.5 + 0.5 * math.sin(tnow * 3 + i * 2.1)
        pygame.draw.line(screen, shade((150, 140, 120), 0.6 + 0.5 * a),
                         (px_ - 4, py_), (px_ + 4, py_), 1)

    draw_hazards(screen, g, tnow)
    draw_caches(screen, g, tnow)
    if gfx_hi():
        draw_page_effects(screen, g, tnow)

    if g.selected is not None:
        zq = quad(BOOK.left, RIVER_Y + RIVER_H, BOOK.right, BOOK.bottom)
        zs = pygame.Surface((W, H), pygame.SRCALPHA)
        pygame.draw.polygon(zs, (36, 128, 118, 30), zq)
        screen.blit(zs, (0, 0))

    shadow_layer = pygame.Surface((W, H), pygame.SRCALPHA)
    ents = ([t for t in g.towers if t.alive] + list(g.units))
    # shadows first (under everyone), then bodies by depth
    for e in ents:
        if isinstance(e, Tower):
            sx, sy, sc = proj(e.x, e.y)
            cast_shadow(shadow_layer, sx, sy + e.size * sc * tower_vis_scale() * 0.5, 1.0, e.size * sc * tower_vis_scale() * 1.3,
                        LIGHT_MAIN, stretch=3.0, alpha=115)
    screen.blit(shadow_layer, (0, 0))
    shadow_layer = pygame.Surface((W, H), pygame.SRCALPHA)
    cart = cartoon_on()
    # draw crumpling corpses first (beneath the living), then living entities
    for d in getattr(g, "dying", []):
        _draw_dying(screen, shadow_layer, d)
    _draw_waves(screen, g)          # Tidal Wave crests roll over the lane
    for e in sorted(ents, key=lambda e: e.y):
        if isinstance(e, Tower):
            draw_tower(screen, shadow_layer, e)
        else:
            if cart:
                draw_cartoon_smear(screen, e)      # hand-drawn motion streak
            origami_animal(screen, shadow_layer, e)
            if cart:
                e._crx, e._cry = e.x, e.y          # remember this held pose
    # accessibility: float a sigil badge above each ability-bearing unit
    if SIGIL_BADGES and SIGILS_OK:
        for e in ents:
            if not isinstance(e, Tower):
                _draw_sigil_badge(screen, e)
    # unit shadows composite beneath subsequent light pass
    screen.blit(shadow_layer, (0, 0), special_flags=pygame.BLEND_RGBA_MULT) if False else screen.blit(shadow_layer, (0, 0))

    for p in g.projs:
        px_, py_, _ = proj(p[0], p[1])
        kind = p[4] if len(p) > 4 else "bolt"
        # a soft motion trail behind each projectile in hi mode
        if gfx_hi() and len(p) > 5:
            ox, oy, _ = proj(p[5][0], p[5][1])
            tcol = {"mortar": (255, 170, 80), "splash": (150, 210, 140),
                    "frost": (170, 220, 255)}.get(kind, (120, 130, 170))
            trail = pygame.Surface((W, H), pygame.SRCALPHA)
            pygame.draw.line(trail, (*tcol, 90), (ox, oy), (px_, py_), 4)
            screen.blit(trail, (0, 0))
        if kind in ("mortar",):
            # heavy iron shell with a hot core + fletching
            pygame.draw.circle(screen, (60, 46, 30), (int(px_), int(py_)), 9)
            pygame.draw.circle(screen, (160, 140, 80), (int(px_), int(py_)), 8)
            pygame.draw.circle(screen, (255, 180, 90), (int(px_), int(py_)), 4)
            pygame.draw.circle(screen, (90, 70, 40), (int(px_), int(py_)), 8, 2)
        elif kind == "splash":
            # a wet ink blob with a lighter rim + droplet specks
            pygame.draw.circle(screen, (120, 170, 110), (int(px_), int(py_)), 6)
            pygame.draw.circle(screen, (180, 220, 170), (int(px_ - 2), int(py_ - 2)), 2)
            pygame.draw.circle(screen, PAGE, (int(px_), int(py_)), 6, 1)
        elif kind == "frost":
            # a faceted ice shard: diamond + inner glint
            r = 6
            pygame.draw.polygon(screen, (150, 200, 240),
                [(px_, py_ - r), (px_ + r * 0.7, py_), (px_, py_ + r), (px_ - r * 0.7, py_)])
            pygame.draw.polygon(screen, (230, 245, 255),
                [(px_, py_ - r), (px_ + r * 0.7, py_), (px_, py_ + r), (px_ - r * 0.7, py_)], 1)
            pygame.draw.circle(screen, (255, 255, 255), (int(px_ - 1), int(py_ - 1)), 1)
        else:
            # ink bolt with a subtle highlight
            pygame.draw.circle(screen, INK, (int(px_), int(py_)), 5)
            pygame.draw.circle(screen, RIVER_HI, (int(px_), int(py_)), 5, 1)
            if gfx_hi():
                pygame.draw.circle(screen, (120, 140, 180), (int(px_ - 1), int(py_ - 1)), 2)
    for p in g.parts:
        a = max(0, p.life / p.max)
        px_, py_, psc = proj(p.x, p.y)
        pygame.draw.circle(screen, shade(p.col, 0.6 + 0.4 * a),
                           (int(px_), int(py_)), max(1, int(p.r * a * psc)))
    for ft in g.texts:
        px_, py_, _ = proj(ft.x, ft.y)
        img = bfont.render(ft.txt, True, ft.col)
        screen.blit(img, (px_ - img.get_width() / 2, py_))

    if getattr(g, "mode", "classic") == "fog":
        draw_fog(screen, g, tnow)

    if g.boss == "angler" and g.hook_anim:
        tgt, ht, phase = g.hook_anim
        lx, ly, _ = proj(*g.hook_lure)
        tx_, ty_, _ = proj(tgt.x, tgt.y)
        wob = math.sin(tnow * 14) * 6
        pygame.draw.line(screen, (70, 64, 56), (lx, ly), (tx_ + wob, ty_), 2)
        pygame.draw.circle(screen, (40, 36, 32), (int(lx), int(ly)), 5)
        pygame.draw.circle(screen, WAX, (int(tx_ + wob), int(ty_)), 4)

    # ── collect dynamic colored lights from units, projectiles, spells (hi) ──
    dyn = []
    if gfx_hi():
        for u in g.units:
            if not u.alive:
                continue
            col = SIGIL_LIGHT.get(getattr(u, "sigil", None))
            if col:
                sx, sy, sc = proj(u.x, u.y)
                # pulse a little; attackers flare brighter mid-swing
                pulse = 0.5 + 0.25 * math.sin(tnow * 6 + sx)
                if getattr(u, "cool", 0) > 0.6:      # just fired/attacked
                    pulse += 0.4
                dyn.append(((sx, sy - 8), 70 * sc, col, min(1.0, pulse)))
        for p in g.projs:
            kind = p[4] if len(p) > 4 else "bolt"
            pcol = {"frost": (140, 210, 255), "splash": (120, 200, 120),
                    "mortar": (255, 170, 80)}.get(kind)
            if pcol:
                px_, py_, _ = proj(p[0], p[1])
                dyn.append(((px_, py_), 46, pcol, 0.8))
        for fx in getattr(g, "frenzy_fx", []):
            px_, py_, _ = proj(fx["x"], fx["y"])
            if fx.get("kind") == "fireball":
                dyn.append(((px_, py_), 90, (255, 140, 50), 1.0))
            elif fx.get("kind") == "bolt":
                dyn.append(((px_, py_), 110, (200, 220, 255), 1.0))
        # lava glow / hazard tints
        if HAZARD == "lava" and getattr(g, "lava", 0) > 0:
            ey = BOOK.top + BOOK.h * g.lava
            dyn.append(((DISP_CX, proj(BOOK.centerx, ey)[1]), 260, (255, 90, 30), 0.5 * g.lava + 0.3))

    lm = build_lightmap(tnow, dynamic=dyn)
    screen.blit(lm, (0, 0), special_flags=pygame.BLEND_MULT)

    sway = math.sin(tnow * 0.8) * 10
    rest_tip = proj(BOOK.left + 120, BOOK.top + 55)
    draw_boss_hand(screen, (rest_tip[0] - 60 + sway, -40),
                   (rest_tip[0] + sway * 0.4, rest_tip[1]), spread=1.25)
    if g.hand_reach:
        tx, ty, _, key = g.hand_reach
        ph = g.hand_t
        reach = math.sin(min(1.0, ph / 0.55) * math.pi / 2) if ph < 0.55 else \
                (1.0 - (ph - 0.55) / 0.65)
        reach = max(0, min(1, reach))
        ttip = proj(tx, ty)
        start = (W * 0.72, -60)
        hx = lerp(start[0], ttip[0], reach)
        hy = lerp(start[1], ttip[1] - 8, reach)
        draw_boss_hand(screen, (hx + (hx - ttip[0]) * 0.4, hy - 150),
                       (hx, hy), spread=0.9, holding=key, sfont=sfont)

    if ARENAS[CUR_ARENA]["candles"]:
        snuff = getattr(g, "snuff_candle", None)
        for ci, ((cx, cy), size) in enumerate((((DISP_CANDLE_R), 1.0), ((DISP_CANDLE_L), 0.7))):
            ch2 = int(88 * size)
            pygame.draw.rect(screen, (214, 200, 172), (cx - 13 * size, cy, 26 * size, ch2), border_radius=4)
            pygame.draw.rect(screen, (150, 134, 108), (cx - 13 * size, cy, 26 * size, ch2), 2, border_radius=4)
            rrc = random.Random(int(cx))
            for _ in range(4):
                dx0 = cx - 13 * size + rrc.randint(2, int(22 * size))
                dl = rrc.randint(10, 34)
                pygame.draw.line(screen, (228, 216, 190), (dx0, cy + 2), (dx0, cy + dl), 3)
                pygame.draw.circle(screen, (228, 216, 190), (int(dx0), cy + dl), 2)
            pygame.draw.rect(screen, (60, 40, 30), (cx - 20 * size, cy + ch2, 40 * size, 10))
            # candle index 0 (right) = team 0 / bottom; 1 (left) = team 1 / top.
            snuffed = (snuff == ci)
            if snuffed:
                # a curl of smoke rising from the dead wick
                for k in range(8):
                    sy = cy - 12 - k * 7
                    sx = cx + math.sin(tnow * 2 + k * 0.7) * (3 + k)
                    a = max(0, 120 - k * 15)
                    smk = pygame.Surface((10, 10), pygame.SRCALPHA)
                    pygame.draw.circle(smk, (90, 84, 78, a), (5, 5), max(1, 4 - k // 3))
                    screen.blit(smk, (int(sx - 5), int(sy - 5)))
                pygame.draw.circle(screen, (40, 30, 24), (int(cx), int(cy - 11)), int(3 * size))
            else:
                fl2 = (6 + math.sin(tnow * 9 + cx) * 2 + math.sin(tnow * 17) * 1.2) * size
                pygame.draw.circle(screen, (252, 186, 96), (int(cx), int(cy - 10)), int(fl2 * 1.6))
                pygame.draw.circle(screen, (255, 236, 178), (int(cx), int(cy - 11)), int(fl2))
                pygame.draw.circle(screen, (255, 255, 235), (int(cx), int(cy - 12)), max(1, int(fl2 * 0.4)))
        for i in range(22):
            rr3 = random.Random(i * 7)
            mx = DISP_CANDLE_R[0] - rr3.uniform(40, 420) + math.sin(tnow * 0.6 + i) * 8
            my = DISP_CANDLE_R[1] + rr3.uniform(-60, 300) + math.sin(tnow * 0.4 + i * 2) * 6
            pygame.draw.circle(screen, (200, 184, 150), (int(mx), int(my)), 1)

    draw_weather(screen, g, tnow)
    # low-health chaos bits (fireballs, lightning strikes)
    for fx in getattr(g, "frenzy_fx", []):
        px_, py_, sc = proj(fx["x"], fx["y"])
        if fx.get("kind") == "fireball":
            r = int(14 * sc)
            pygame.draw.circle(screen, (255, 160, 60), (int(px_), int(py_)), r)
            pygame.draw.circle(screen, (255, 230, 150), (int(px_), int(py_)), int(r * 0.5))
            for k in range(4):
                pygame.draw.circle(screen, (200, 80, 30),
                                   (int(px_ + random.uniform(-8, 8)), int(py_ - r - k * 5)), max(1, r - k * 3))
        elif fx.get("kind") == "bolt":
            yy = 0; bx = px_
            pts = [(bx, 0)]
            rr = random.Random(int(fx["x"]))
            while yy < py_:
                yy += rr.randint(18, 40); bx += rr.randint(-24, 24)
                pts.append((bx, yy))
            pygame.draw.lines(screen, (235, 240, 255), False, pts, 2)

    # desk vignette — fades the board's edges into shadow so the table reads as
    # filling the whole screen (esp. on tall phones) rather than ending abruptly.
    battle_vignette(screen)

    title = bfont.render("P A P E R B O U N D", True, shade(GOLD, 0.95))
    screen.blit(title, (W / 2 - title.get_width() / 2, 8))
    ch = sfont.render(ARENAS[CUR_ARENA]["chapter"] + " — " + ARENAS[CUR_ARENA]["name"], True, shade(PAGE, 0.65))
    screen.blit(ch, (W / 2 - ch.get_width() / 2, 34))
    # Arch-Scribe boss banner (with a rare variant title prefix)
    if AIBOSS_OK and getattr(g, "ai_boss_id", None):
        pers = ai_boss.BOSS_PERSONAS.get(g.ai_boss_id)
        if pers:
            tint = shade(tuple(min(255, c + 110) for c in pers.palette), 1.0)
            var = getattr(g, "boss_variant", None)
            name = f"{var.title} {pers.name}" if var else pers.name
            bl = sfont.render(f"☠ {name}, {pers.title} · phase {getattr(g,'boss_phase',1)}", True, tint)
            screen.blit(bl, (W / 2 - bl.get_width() / 2, 54))
    if getattr(g, "netmode", None):
        role = {"host": "HOST", "client": "CLIENT", "spectator": "SPECTATING"}.get(g.netmode, g.netmode.upper())
        col = shade(GOLD, 1.1) if g.netmode == "spectator" else shade(P_ACCENT, 1.3)
        tag = sfont.render(f"◈ LAN · {role}", True, col)
        screen.blit(tag, (16, 12))
    if getattr(g, "mode", "classic") != "classic":
        mt = sfont.render(f"✦ {MODES[g.mode]['name']}", True, shade(GOLD, 1.15))
        screen.blit(mt, (W - mt.get_width() - 16, 12))

    # forfeit / exit button (all modes) — top-right corner
    fbtn = pygame.Rect(W - 132, 34, 116, 30)
    hovf = fbtn.collidepoint(pygame.mouse.get_pos()) if False else False
    pygame.draw.rect(screen, (72, 20, 22), fbtn, border_radius=8)
    pygame.draw.rect(screen, shade(WAX, 1.2), fbtn, 2, border_radius=8)
    flab = sfont.render("Forfeit / Exit", True, shade(PAGE, 0.9))
    screen.blit(flab, (fbtn.centerx - flab.get_width() / 2, fbtn.centery - flab.get_height() / 2))
    HOT["forfeit"] = fbtn

    # The Trickster's darkness overlay (drawn over the board + HUD)
    if getattr(g, "boss", None) == "trickster" and getattr(g, "trick_dark", False):
        _draw_trickster_dark(screen, g, font, bfont, sfont)

    # low-health "frenzy" banner: the page is going wild
    if getattr(g, "frenzy", False):
        pulse = 0.5 + 0.5 * math.sin(tnow * 8)
        band = pygame.Surface((W, 30), pygame.SRCALPHA)
        band.fill((150, 30, 36, int(60 + 60 * pulse)))
        screen.blit(band, (0, 58))
        ftxt = bfont.render("⚠ THE PAGE UNRAVELS ⚠", True, shade((255, 200, 120), 0.9 + 0.3 * pulse))
        screen.blit(ftxt, (W / 2 - ftxt.get_width() / 2, 60))

    cw, chh, gap = 108, 128, 14
    x0 = W / 2 - (4 * cw + 3 * gap) / 2
    # In 2v2 dual-local, the primary strip is seat A's controller; otherwise the
    # normal single hand. This keeps one code path for drawing + affordability.
    if getattr(g, "dual_local", False) and g.seat_ctrl:
        _cA = g.seat_ctrl[list(g.seat_ctrl.keys())[0]]
        p_hand, p_ink, p_sel, p_next = _cA["rot"].hand, _cA["ink"], _cA["selected"], _cA["rot"].next_card
    else:
        p_hand, p_ink, p_sel, p_next = g.hand, g.ink, g.selected, g.next_card
    which_bottom = "seat" if (getattr(g, "dual_local", False) and g.seat_ctrl) else "bottom"
    seat_bottom = list(g.seat_ctrl.keys())[0] if which_bottom == "seat" else None
    for i, key in enumerate(p_hand):
        r = pygame.Rect(x0 + i * (cw + gap), HAND_Y - 30, cw, chh)
        if p_sel == i: r = r.move(0, -12)
        # 'not enough ink' bounce: a quick horizontal shudder on the card
        bt = g.card_bounce.get((which_bottom, i, seat_bottom), 0) if hasattr(g, "card_bounce") else 0
        if bt > 0:
            r = r.move(int(math.sin(bt * 60) * 6), 0)
        _team = getattr(g, "my_team", 0) if g.netmode is not None else 0
        _dc = g.cost_of(key, _team)
        if getattr(g, "trick_hand_shadow", False):
            # the Trickster has hidden your hand — draw shadowed card-backs so you
            # can't read (or trust) what you're about to play.
            pygame.draw.rect(screen, (26, 20, 34), r, border_radius=8)
            pygame.draw.rect(screen, shade((60, 46, 78), 1.0), r, 2, border_radius=8)
            qm = bfont.render("?", True, (120, 96, 150))
            screen.blit(qm, (r.centerx - qm.get_width() / 2, r.centery - qm.get_height() / 2))
        else:
            draw_card(screen, key, r, font, sfont, p_ink >= _dc, cost_override=_dc)
        # tiny keyboard-shortcut hint for this slot (1-4 / a-o-e-u)
        hint = ("1", "2", "3", "4")[i] + "·" + ("a", "o", "e", "u")[i]
        hk = sfont.render(hint, True, shade(PAGE, 0.62))
        screen.blit(hk, (r.centerx - hk.get_width() / 2, r.top - 16))
    nxt = sfont.render("Next: " + CARDS[p_next]["name"], True, shade(PAGE, 0.75))
    screen.blit(nxt, (x0 - nxt.get_width() - 18, HAND_Y + 40))
    bar = pygame.Rect(x0 + 4 * (cw + gap) + 6, HAND_Y - 10, 26, 96)
    pygame.draw.rect(screen, (14, 10, 8), bar.inflate(8, 8), border_radius=6)
    pygame.draw.rect(screen, shade(PAGE, 0.35), bar, border_radius=4)
    _cap = ink_cap(g.mode if g else MODE)
    fill = bar.h * min(1.0, p_ink / _cap)
    pygame.draw.rect(screen, RIVER_HI, (bar.x, bar.bottom - fill, bar.w, fill), border_radius=4)
    # pip lines every ~cap/10 so a 20-cap bar reads without clutter
    _pips = 10
    for i in range(1, _pips):
        yy = bar.bottom - bar.h * i / _pips
        pygame.draw.line(screen, shade(PAGE, 0.55), (bar.x, yy), (bar.right, yy), 1)
    it = font.render(f"{int(p_ink)}", True, PAGE)
    screen.blit(it, (bar.centerx - it.get_width() / 2, bar.bottom + 6))
    screen.blit(sfont.render("INK", True, shade(PAGE, 0.7)), (bar.x - 2, bar.y - 22))

    # ── 2v2 dual-local: this device drives TWO seats on the same team. Seat A
    #    uses the primary bottom hand (drawn above from seat_ctrl); seat B gets a
    #    compact strip just above it. Both deploy onto the shared team's half. ──
    if getattr(g, "dual_local", False) and g.seat_ctrl:
        seats = list(g.seat_ctrl.keys())
        # relabel the primary bottom hand as "P1" and draw P2's compact strip
        lblA = sfont.render(f"P1 · seat {seats[0]}", True, shade(P_ACCENT, 1.3))
        screen.blit(lblA, (x0 - lblA.get_width() - 18, HAND_Y + 62))
        if len(seats) > 1:
            cB = g.seat_ctrl[seats[1]]
            p2 = sfont.render(f"P2 · seat {seats[1]}", True, shade(GOLD, 1.1))
            screen.blit(p2, (16, 12))
            cw2, gap2 = 96, 12
            x02 = W / 2 - (4 * cw2 + 3 * gap2) / 2
            HOT["hand_b"] = []
            for i, key in enumerate(cB["rot"].hand):
                r = pygame.Rect(x02 + i * (cw2 + gap2), 6, cw2, 96)
                if cB["selected"] == i: r = r.move(0, 10)
                aff = cB["ink"] >= CARDS[key]["cost"]
                pygame.draw.rect(screen, shade(PAGE, 1.0 if aff else 0.6), r, border_radius=6)
                pygame.draw.rect(screen, shade(GOLD, 1.0), r, 2, border_radius=6)
                nm = sfont.render(CARDS[key]["name"][:10], True, INK)
                screen.blit(nm, (r.centerx - nm.get_width() / 2, r.bottom - 22))
                pygame.draw.circle(screen, INK, (r.left + 13, r.top + 13), 11)
                cc = sfont.render(str(CARDS[key]["cost"]), True, PAGE)
                screen.blit(cc, (r.left + 13 - cc.get_width() / 2, r.top + 13 - cc.get_height() / 2))
                sc = SIGIL_COLORS.get(CARDS[key]["sigil"], GOLD)
                pygame.draw.circle(screen, sc, (r.right - 13, r.top + 13), 6)
                hint = ("7", "8", "9", "0")[i] + "·" + ("h", "t", "n", "s")[i]
                hk = sfont.render(hint, True, shade(GOLD, 1.0))
                screen.blit(hk, (r.centerx - hk.get_width() / 2, r.bottom + 2))
                HOT["hand_b"].append((r, i))
            # P2 ink pip row
            ib = sfont.render(f"P2 ink {int(cB['ink'])}", True, shade(GOLD, 1.1))
            screen.blit(ib, (x02 + 4 * (cw2 + gap2) + 8, 44))

    # ── hotseat player-2 hand: a compact strip flipped along the top edge ──
    if getattr(g, "local_1v1", False) and g.hand2:
        p2 = sfont.render("PLAYER 2", True, shade(E_ACCENT, 1.4))
        screen.blit(p2, (16, 12))
        cw2, gap2 = 96, 12
        x02 = W / 2 - (4 * cw2 + 3 * gap2) / 2
        for i, key in enumerate(g.hand2):
            r = pygame.Rect(x02 + i * (cw2 + gap2), 6, cw2, 96)
            if g.selected2 == i: r = r.move(0, 10)
            # mini card: cost + name, tinted by affordability
            aff = g.ai_ink2 >= CARDS[key]["cost"]
            pygame.draw.rect(screen, shade(PAGE, 1.0 if aff else 0.6), r, border_radius=6)
            pygame.draw.rect(screen, shade(E_ACCENT, 1.0), r, 2, border_radius=6)
            nm = sfont.render(CARDS[key]["name"][:10], True, INK)
            screen.blit(nm, (r.centerx - nm.get_width() / 2, r.bottom - 22))
            pygame.draw.circle(screen, INK, (r.left + 13, r.top + 13), 11)
            cc = sfont.render(str(CARDS[key]["cost"]), True, PAGE)
            screen.blit(cc, (r.left + 13 - cc.get_width() / 2, r.top + 13 - cc.get_height() / 2))
            sc = SIGIL_COLORS.get(CARDS[key]["sigil"], GOLD)
            pygame.draw.circle(screen, sc, (r.right - 13, r.top + 13), 6)
            hint = ("7", "8", "9", "0")[i] + "·" + ("h", "t", "n", "s")[i]
            hk = sfont.render(hint, True, shade(E_ACCENT, 1.4))
            screen.blit(hk, (r.centerx - hk.get_width() / 2, r.bottom + 2))
        # p2 ink bar on the right of the top strip
        bar2 = pygame.Rect(x02 + 4 * (cw2 + gap2) + 6, 8, 22, 90)
        pygame.draw.rect(screen, (14, 10, 8), bar2.inflate(8, 8), border_radius=6)
        pygame.draw.rect(screen, shade(PAGE, 0.35), bar2, border_radius=4)
        fill2 = bar2.h * g.ai_ink2 / 10
        pygame.draw.rect(screen, E_ACCENT, (bar2.x, bar2.bottom - fill2, bar2.w, fill2), border_radius=4)
        it2 = sfont.render(f"{int(g.ai_ink2)}", True, PAGE)
        screen.blit(it2, (bar2.centerx - it2.get_width() / 2, bar2.bottom + 2))

    vg = pygame.Surface((W, H), pygame.SRCALPHA)
    for i in range(10):
        a = 12 + i * 7
        pygame.draw.rect(vg, (4, 2, 6, a), (i * 3, i * 3, W - i * 6, H - i * 6),
                         width=6, border_radius=30)
    screen.blit(vg, (0, 0))

    draw_table_finds(screen, g, tnow, sfont)
    draw_found_toasts(screen, g, font, sfont)

    apply_shockwaves(screen, g)         # spell/boss screen-distortion ripples
    apply_postfx(screen, tnow)          # photorealistic bloom / grain / chroma

    if g.result:
        ov = pygame.Surface((W, H), pygame.SRCALPHA)
        ov.fill((6, 4, 5, 190))
        screen.blit(ov, (0, 0))
        big = _font(64, bold=True)
        # if the loss came from the *opponent* leaving, the local player WON
        opp_left = getattr(g, "forfeit_notice", None) is not None
        won = (g.result == "win") or opp_left
        msg = "THE PAGE HOLDS" if won else "THE INK RUNS DRY"
        col = shade(GOLD, 1.0) if won else shade(WAX, 1.15)
        img = big.render(msg, True, col)
        screen.blit(img, (W / 2 - img.get_width() / 2, H / 2 - 60))
        if opp_left:
            note = font.render("Your opponent forfeited and left the table.",
                               True, shade(PAGE, 0.9))
            screen.blit(note, (W / 2 - note.get_width() / 2, H / 2 + 8))
            sub = sfont.render("Victory by default  ·  click to continue", True, shade(PAGE, 0.8))
        else:
            sub = sfont.render("click to continue", True, shade(PAGE, 0.8))
        screen.blit(sub, (W / 2 - sub.get_width() / 2, H / 2 + 40))

MENU_BOOK = pygame.Rect(232, 92, 816, 604)
# Tokens (e.g. the Beekeeper's bees) are spawned by other cards, never drafted or
# built directly, so keep them out of the craftable/deck pool.
ALL_CARDS = [k for k in CARDS.keys()
             if not (CARDS[k].get("token") or str(CARDS[k].get("set", "")).endswith("_token"))]
# The set of cards that ship in cards.json — used by the replay recorder to know
# which card defs are "base" (already on disk) vs dynamically created mid-match
# (wildcard/generated/boss) and therefore need snapshotting into the replay.
BASE_CARD_KEYS = frozenset(CARDS.keys())

try:
    import replay as replaymod
    REPLAY_OK = True
except Exception:
    REPLAY_OK = False
    replaymod = None

try:
    import netreliable
    NETREL_OK = True
except Exception:
    NETREL_OK = False
    netreliable = None

try:
    import patchtree
    PATCHTREE_OK = True
except Exception:
    PATCHTREE_OK = False
    patchtree = None

try:
    import trophy as trophymod
    TROPHY_OK = True
except Exception:
    TROPHY_OK = False
    trophymod = None

try:
    import tournament as tournamod
    TOURNEY_OK = True
except Exception:
    TOURNEY_OK = False
    tournamod = None

# ── AI Director: runtime card/map generation (LLM or procedural) ───────────
try:
    import ai_director
    AIDIR_OK = True
except Exception:
    AIDIR_OK = False
    ai_director = None

try:
    import ai_boss
    AIBOSS_OK = True
except Exception:
    AIBOSS_OK = False
    ai_boss = None

try:
    import boss_sim
    BOSSSIM_OK = True
except Exception:
    BOSSSIM_OK = False
    boss_sim = None
SHOW_BALANCE = False           # debug-menu balance-audit view toggle
ECHO_STYLE = "apparition"      # spirit-echo look: "apparition" (shadowed base
                               # model) or "wraith" (bespoke tattered form).
                               # Toggled in the debug menu.

def make_replay_game(meta, cards):
    """Build a Game configured to replay a recorded match: same seed/mode/arena,
    any non-base cards pre-registered, AI/boss/director disabled, and its own
    recorder suppressed. Used by replay.ReplayPlayer."""
    global MODE
    # re-register any dynamically-created cards the replay carries
    for key, entry in (cards or {}).items():
        if key not in CARDS:
            e = dict(entry)
            if isinstance(e.get("paper"), list):
                e["paper"] = tuple(e["paper"])
            CARDS[key] = e
    prev_mode = MODE
    MODE = meta.get("mode", MODE)
    if meta.get("arena"):
        set_arena(meta["arena"])
    g = Game.__new__(Game)          # bypass normal init to set the replay flag first
    g.is_replay = True
    Game.__init__(g, deck=meta.get("deck", DEFAULT_DECK[:]),
                  netmode=None, seed=meta.get("seed"), local_1v1=False)
    MODE = prev_mode
    # a replay never runs the local AI opponent or saves its own recording
    g._recorder = None
    return g


def open_replay(path):
    """Load a replay file and return a ReplayPlayer ready to scrub, or None."""
    if not REPLAY_OK:
        return None
    try:
        doc = replaymod.load(path)
        player = replaymod.ReplayPlayer(doc, make_replay_game)
        player.compat = doc.get("compat", "unknown")
        return player
    except Exception:
        return None


def register_card(card, force_key=None):
    """Install a generated card dict (ai_director schema) into the live game so
    it can be deployed, drawn and rotated exactly like a built-in card. Returns
    the card key, or None if the payload was unusable. Safe to call at runtime.
    `force_key` pins the exact key (used by the wildcard for net-determinism)."""
    if not isinstance(card, dict):
        return None
    if AIDIR_OK:                       # re-validate at the boundary, always
        card = ai_director.sanitize_card(card, taken_ids=tuple(CARDS.keys()))
    if not card or "id" not in card:
        return None
    key = force_key or card["id"]
    ab = card.get("ability", "sharp")
    # runtime flat entry in the exact shape CARDS uses
    entry = {"name": card["name"], "cost": int(card["cost"]), "hp": int(card["hp"]),
             "dmg": int(card["dmg"]), "spd": card["spd"], "rng": int(card["rng"]),
             "size": int(card["size"]), "flying": bool(card["flying"]),
             "paper": tuple(card["paper"]), "sigil": ab}
    if card.get("building"): entry["building"] = True
    if card.get("riverhop"): entry["riverhop"] = True
    if int(card.get("count", 1)) > 1: entry["count"] = int(card["count"])
    if isinstance(card.get("model"), (list, tuple)): entry["model"] = card["model"]
    if isinstance(card.get("model_extra"), (list, tuple)): entry["model_extra"] = card["model_extra"]
    CARDS[key] = entry
    if key not in ALL_CARDS and not key.startswith("wild"):
        ALL_CARDS.append(key)
    # make sure the ability has a colour + a sketch family so it renders
    SIGIL_COLORS.setdefault(ab, tuple(card["paper"]))
    SIGIL_DESC.setdefault(ab, card.get("blurb", ""))
    fam = "monolith" if card.get("building") else ("bird" if card.get("flying") else "feline")
    _CARD_FAMILY.setdefault(key, fam)
    if ORIGAMI_OK:                      # drop any stale cached recipe for this key
        try: origami.clear_cache()
        except Exception: pass
    return key

def register_arena(mp, key=None):
    """Install a generated arena dict and return its key. Does NOT switch to it;
    call set_arena(key) after. Safe at runtime."""
    if not isinstance(mp, dict):
        return None
    if AIDIR_OK:
        mp = ai_director.sanitize_map(mp)
    key = key or ("living_" + "".join(c for c in mp.get("name", "page").lower() if c.isalnum())[:14])
    # coerce the ai_director map schema into the ARENAS entry shape
    ARENAS[key] = dict(
        name=mp["name"], chapter=mp.get("chapter", "Living Page"),
        page=tuple(mp["page"]), page_line=tuple(mp["page_line"]),
        desk=tuple(mp["desk"]), desk_grain=tuple(mp["desk_grain"]),
        cover=tuple(mp["cover"]), river=tuple(mp["river"]),
        river_hi=tuple(mp["river_hi"]), ribbon=tuple(mp["ribbon"]),
        book=tuple(mp["book"]), top_y=mp["top_y"], bot_y=mp["bot_y"],
        ambient=tuple(mp["ambient"]), light_main=tuple(mp["light_main"]),
        lights=[(tuple(p), r, tuple(c)) for (p, r, c) in mp["lights"]],
        candles=bool(mp["candles"]), weather=mp["weather"],
        hazard=mp["hazard"], poi=mp["poi"])
    return key

STARTER = ["sabertooth", "pangolin", "crane", "polarbear", "wolverine",
           "batcolony", "gorilla", "baobab", "frog", "cassowary"]
UNLOCKS = set(STARTER)
# seed the persistent collection with the starter set on first run
if COLLECTION_OK:
    try:
        for _sk in STARTER:
            if not PBCOLLECTION.owns(_sk):
                PBCOLLECTION.grant(_sk, how="starter")
        # restore the chosen defensive troop
        _tp = PBCOLLECTION.get_pref("tower_troop")
        if _tp in TOWER_TROOPS:
            TOWER_TROOP = _tp
    except Exception:
        pass
DEFAULT_DECK = ["sabertooth", "polarbear", "crane", "baobab", "wolverine", "pangolin", "frog", "mammoth"]
# Ten storable deck slots. Slot 0 is the classic starter; the rest start empty
# and fill in from the deck builder. Persisted via the collection backend.
DECK_SLOTS = [DEFAULT_DECK[:] for _ in range(10)]
DECK_SLOT = 0                     # active slot index
DECK = DECK_SLOTS[DECK_SLOT]      # active deck (alias into the slot list)

def _load_deck_slots():
    global DECK_SLOTS, DECK_SLOT, DECK
    if not COLLECTION_OK:
        return
    try:
        saved = PBCOLLECTION.get_pref("deck_slots")
        if isinstance(saved, list) and saved:
            for i in range(min(10, len(saved))):
                if isinstance(saved[i], list):
                    DECK_SLOTS[i] = [k for k in saved[i] if k in CARDS]
        si = PBCOLLECTION.get_pref("deck_slot", 0)
        if isinstance(si, int) and 0 <= si < 10:
            DECK_SLOT = si
        DECK = DECK_SLOTS[DECK_SLOT]
    except Exception:
        pass

def _save_deck_slots():
    if not COLLECTION_OK:
        return
    try:
        PBCOLLECTION.set_pref("deck_slots", DECK_SLOTS)
        PBCOLLECTION.set_pref("deck_slot", DECK_SLOT)
    except Exception:
        pass

def select_deck_slot(i):
    """Switch the active deck slot; DECK aliases into that slot's list."""
    global DECK_SLOT, DECK
    DECK_SLOT = max(0, min(9, i))
    DECK = DECK_SLOTS[DECK_SLOT]
    _save_deck_slots()

DEBUG_ALL = False
HOT = {}          # clickable rects populated during draw, read by event loop
_load_deck_slots()

def unlocked(key):
    if DEBUG_ALL or key in UNLOCKS:
        return True
    # anything the player has found (persisted collection) is playable
    if COLLECTION_OK:
        try:
            return PBCOLLECTION.owns(key)
        except Exception:
            return False
    return False

def draw_desk_and_book(screen, t, title, subtitle):
    """flat (non-perspective) open book used for all menu screens."""
    screen.fill(DESK)
    rng = random.Random(3)
    for i in range(0, W, 90):
        pygame.draw.line(screen, DESK_GRAIN, (i, 0), (i + 40, H), 3)
        pygame.draw.line(screen, shade(DESK, 1.12), (i + 22, 0), (i + 62, H), 1)
    b = MENU_BOOK
    pygame.draw.rect(screen, (10, 7, 6), b.inflate(60, 60).move(10, 14), border_radius=16)
    pygame.draw.rect(screen, COVER, b.inflate(44, 44), border_radius=12)
    pygame.draw.rect(screen, shade(COVER, 1.3), b.inflate(44, 44), 3, border_radius=12)
    for i in range(4):
        pygame.draw.rect(screen, shade(PAGE, 0.55 + i * 0.09), b.inflate(20 - i * 5, 20 - i * 5), border_radius=6)
    # two pages + spine
    pygame.draw.rect(screen, PAGE, b, border_radius=4)
    for yy in range(b.top + 26, b.bottom, 26):
        pygame.draw.line(screen, PAGE_LINE, (b.left + 16, yy), (b.right - 16, yy), 1)
    # spine shadow
    sp = pygame.Surface((60, b.h), pygame.SRCALPHA)
    for i in range(30):
        a = int(60 * (1 - abs(i - 15) / 15))
        pygame.draw.line(sp, (40, 26, 14, a), (i * 2, 0), (i * 2, b.h), 2)
    screen.blit(sp, (b.centerx - 30, b.top))
    # foxing on the pages
    for _ in range(60):
        x, y = rng.randint(b.left + 10, b.right - 10), rng.randint(b.top + 10, b.bottom - 10)
        pygame.draw.circle(screen, (150, 116, 74, 60), (x, y), rng.randint(1, 3))
    # title on a wax banner
    bf = _font(40, bold=True)
    ti = bf.render(title, True, shade(INK, 1.0))
    screen.blit(ti, (b.centerx - ti.get_width() / 2, b.top + 30))
    if subtitle:
        sf = _font(17)
        si = sf.render(subtitle, True, shade(INK, 2.2))
        screen.blit(si, (b.centerx - si.get_width() / 2, b.top + 78))

def button(screen, rect, label, font, mouse, enabled=True, key=None):
    hover = rect.collidepoint(mouse) and enabled
    base = (86, 30, 30) if enabled else (70, 64, 54)
    col = shade(base, 1.3) if hover else base
    pygame.draw.rect(screen, (12, 8, 6), rect.move(2, 3), border_radius=8)
    pygame.draw.rect(screen, col, rect, border_radius=8)
    pygame.draw.rect(screen, shade(col, 1.4), rect, 2, border_radius=8)
    ti = font.render(label, True, shade(PAGE, 1.05) if enabled else shade(PAGE, 0.7))
    screen.blit(ti, (rect.centerx - ti.get_width() / 2, rect.centery - ti.get_height() / 2))
    if key is not None:
        HOT[key] = rect
    return hover

def draw_torn_card(surf, rect):
    """locked card slot: a torn-out page stub."""
    rng = random.Random(rect.x * 7 + rect.y)
    pts = [(rect.left, rect.top)]
    x = rect.left
    while x < rect.right:                     # ragged bottom tear
        x += rng.randint(10, 22)
        pts.append((min(x, rect.right), rect.bottom - rng.randint(0, 26)))
    pts += [(rect.right, rect.top)]
    pygame.draw.polygon(surf, shade(PAGE, 0.66), pts)
    pygame.draw.polygon(surf, shade(PAGE, 0.4), pts, 2)
    f = _font(13, italic=True)
    t1 = f.render("torn", True, shade(INK, 3.0))
    t2 = f.render("out", True, shade(INK, 3.0))
    surf.blit(t1, (rect.centerx - t1.get_width() / 2, rect.centery - 16))
    surf.blit(t2, (rect.centerx - t2.get_width() / 2, rect.centery + 2))

def draw_menu(screen, fonts, t, mouse):
    font, bfont, sfont = fonts
    draw_desk_and_book(screen, t, "PAPERBOUND", "an origami lane-battler")
    b = MENU_BOOK
    HOT.clear()
    # left page: a folded crane emblem
    emblem = Unit("crane", 0, 0, 0); emblem.spawn_t = 0
    emblem.x, emblem.y = unproj(b.left + 200, b.centery + 40)
    sh = pygame.Surface((W, H), pygame.SRCALPHA)
    _sc = proj(emblem.x, emblem.y)[2]; emblem.size = 60 / _sc
    origami_animal(screen, sh, emblem)
    fl = sfont.render("Fold your beasts. Hold the page.", True, shade(INK, 2.4))
    screen.blit(fl, (b.left + 200 - fl.get_width() / 2, b.centery + 150))
    # seasonal event greeting (date-driven; the same for everyone on the day)
    if ACTIVE_EVENT:
        ev = ACTIVE_EVENT
        acc = ev.get("accent", GOLD)
        title = f"{ev.get('emoji','')}  {ev['name']}  {ev.get('emoji','')}"
        et = sfont.render(title, True, acc)
        screen.blit(et, (b.left + 200 - et.get_width() / 2, b.centery + 178))
        bn = _font(15).render(ev.get("banner", ""), True, shade(INK, 2.2))
        screen.blit(bn, (b.left + 200 - bn.get_width() / 2, b.centery + 200))
    # right page: menu buttons
    bx = b.centerx + 70
    ys = b.top + 52
    gap = 39
    bh = 34
    button(screen, pygame.Rect(bx, ys + 0*gap, 300, bh), "Play", bfont, mouse, key="play")
    button(screen, pygame.Rect(bx, ys + 1*gap, 300, bh), "Arch-Scribe Duel", bfont, mouse,
           enabled=AIBOSS_OK, key="aiboss")
    button(screen, pygame.Rect(bx, ys + 2*gap, 300, bh), "Local 1v1", bfont, mouse, key="local1v1")
    button(screen, pygame.Rect(bx, ys + 3*gap, 300, bh), "Foldworks (RTS)", bfont, mouse,
           enabled=RTS_OK, key="rts")
    button(screen, pygame.Rect(bx, ys + 4*gap, 300, bh), "Multiplayer (LAN)", bfont, mouse, key="mp")
    button(screen, pygame.Rect(bx, ys + 5*gap, 300, bh), "Deck Builder", bfont, mouse, key="deck")
    button(screen, pygame.Rect(bx, ys + 6*gap, 300, bh), "Defenses", bfont, mouse, key="towers")
    button(screen, pygame.Rect(bx, ys + 7*gap, 300, bh), "Collection", bfont, mouse, key="collection")
    button(screen, pygame.Rect(bx, ys + 8*gap, 300, bh), "Profile / Replays", bfont, mouse,
           enabled=REPLAY_OK, key="replays")
    button(screen, pygame.Rect(bx, ys + 9*gap, 300, bh), "Choose Arena", bfont, mouse, key="arena")
    button(screen, pygame.Rect(bx, ys + 10*gap, 300, bh), "Options", bfont, mouse, key="options")
    button(screen, pygame.Rect(bx, ys + 11*gap, 300, bh), "Codex / Debug", bfont, mouse, key="debug")
    button(screen, pygame.Rect(bx, ys + 12*gap, 300, bh), "Quit", bfont, mouse, key="quit")
    menu_vignette(screen)

def make_rock_event(rng=None):
    """The Rock of Power: a rare roaming find. Instead of a reward path, you place
    one of YOUR owned cards on the standing stone and it returns Spirit-Divergent —
    on death it splits into shadow echoes. Offers a few of your cards to imbue."""
    rng = rng or random
    owned = []
    if COLLECTION_OK:
        try:
            owned = [carddata.base_of(k) for k in PBCOLLECTION.owned_keys()]
        except Exception:
            owned = []
    owned = [k for k in dict.fromkeys(owned) if k in CARDS] or list(DECK) or ALL_CARDS[:6]
    def already(k):
        vk = carddata.variant_key(k, "spiritdiverge")
        return COLLECTION_OK and PBCOLLECTION.owns(vk)
    choices = [k for k in owned if not already(k)]
    rng.shuffle(choices)
    return {"rock": True, "choices": choices[:6], "chosen": None, "granted": None}

def draw_rock(screen, fonts, t, mouse, ev):
    font, bfont, sfont = fonts
    draw_desk_and_book(screen, t, "THE ROCK OF POWER",
                       "a standing stone hums — place a card upon it")
    b = MENU_BOOK
    HOT.clear(); HOT["rockcards"] = []
    cx, cy = b.centerx, b.top + 150
    glow = pygame.Surface((220, 160), pygame.SRCALPHA)
    for rr in range(70, 0, -6):
        a = int(70 * rr / 70)
        pygame.draw.ellipse(glow, (150, 90, 200, a), (110 - rr, 80 - rr*0.7, rr*2, rr*1.4))
    screen.blit(glow, (cx - 110, cy - 80), special_flags=pygame.BLEND_ADD)
    pygame.draw.polygon(screen, shade((90, 70, 120), 1.0),
                        [(cx-34, cy+50), (cx-24, cy-46), (cx+20, cy-52), (cx+36, cy+50)])
    pygame.draw.polygon(screen, (150, 110, 200),
                        [(cx-34, cy+50), (cx-24, cy-46), (cx+20, cy-52), (cx+36, cy+50)], 2)
    if ev.get("granted"):
        gk = ev["granted"]
        msg = font.render(f"{CARDS.get(gk,{}).get('name','Your card')} is now Spirit-Divergent!",
                          True, (180, 130, 220))
        screen.blit(msg, (cx - msg.get_width()/2, cy + 66))
        sub = sfont.render("On death it splits into two shadow echoes per unit "
                           "(25% damage, 15% health).", True, shade(INK, 1.8))
        screen.blit(sub, (cx - sub.get_width()/2, cy + 92))
        button(screen, pygame.Rect(cx - 90, b.bottom - 66, 180, 46), "Continue", font, mouse, key="rockcontinue")
    else:
        prompt = sfont.render("Choose a card to imbue with Spirit Divergence:", True, shade(INK, 1.6))
        screen.blit(prompt, (cx - prompt.get_width()/2, cy + 64))
        choices = ev.get("choices", [])
        n = len(choices)
        if not n:
            none = sfont.render("You carry no cards to place here yet.", True, shade(INK, 2.2))
            screen.blit(none, (cx - none.get_width()/2, cy + 110))
            button(screen, pygame.Rect(cx - 90, b.bottom - 66, 180, 46), "Leave", font, mouse, key="rockcontinue")
        else:
            cols = min(n, 6); cw = 118; gap = 12
            x0 = cx - (cols * cw + (cols-1) * gap) / 2
            for i, key in enumerate(choices[:6]):
                r = pygame.Rect(x0 + i * (cw + gap), cy + 96, cw, 134)
                hover = r.collidepoint(mouse)
                draw_card(screen, key, r, font, sfont, True)
                if hover:
                    pygame.draw.rect(screen, (170, 110, 210), r, 3, border_radius=8)
                HOT["rockcards"].append((r, key))
    menu_vignette(screen)

def make_forest_event(rng=None):
    """Build a 3-path 'forest' choice. Each path offers a card reward (sometimes a
    rare variant). The chosen path grants its card into the persistent collection."""
    rng = rng or random
    pool = [k for k in ALL_CARDS if not unlocked(k)] or ALL_CARDS
    flavors = [
        ("The Mossy Fork", "Ferns curl over a half-buried page."),
        ("The Ink-Dark Trail", "Something rustles beyond the margin."),
        ("The Sunlit Clearing", "A folded shape rests on warm paper."),
        ("The Thorn Passage", "Brambles guard a torn corner."),
        ("The Quiet Hollow", "A hush, and a glint of wax."),
    ]
    rng.shuffle(flavors)
    paths = []
    for i in range(3):
        key = rng.choice(pool)
        var = carddata.roll_variant() if CARDDATA_OK else "standard"
        title, blurb = flavors[i]
        paths.append(dict(title=title, blurb=blurb, key=key, variant=var))
    return {"paths": paths, "chosen": None}

def draw_forest(screen, fonts, t, mouse, ev):
    font, bfont, sfont = fonts
    draw_desk_and_book(screen, t, "A FORK IN THE PATH", "choose where the story wanders")
    b = MENU_BOOK
    HOT.clear(); HOT["paths"] = []
    n = len(ev["paths"])
    cw = (b.width - 80 - (n - 1) * 20) // n
    for i, p in enumerate(ev["paths"]):
        r = pygame.Rect(b.left + 40 + i * (cw + 20), b.top + 130, cw, 360)
        hover = r.collidepoint(mouse)
        chosen = ev["chosen"] == i
        pygame.draw.rect(screen, shade(PAGE, 0.95 if (hover or chosen) else 0.8), r, border_radius=12)
        pygame.draw.rect(screen, GOLD if chosen else shade(INK, 3.2), r, 3 if chosen else 2, border_radius=12)
        ttl = font.render(p["title"], True, INK)
        screen.blit(ttl, (r.centerx - ttl.get_width() / 2, r.top + 18))
        for j, line in enumerate(_wrap(p["blurb"], sfont, r.w - 30)):
            ln = sfont.render(line, True, shade(INK, 2.0))
            screen.blit(ln, (r.centerx - ln.get_width() / 2, r.top + 50 + j * 18))
        # reward preview: show the card art + a rarity ribbon, hidden until chosen
        key = p["key"]
        vk = carddata.make_variant(key, p["variant"]) if CARDDATA_OK else key
        prev = pygame.Rect(r.centerx - 54, r.top + 110, 108, 124)
        if chosen:
            draw_card(screen, vk, prev, font, sfont, True)
            nm = font.render(CARDS.get(vk, {}).get("name", key)[:18], True, INK)
            screen.blit(nm, (r.centerx - nm.get_width() / 2, prev.bottom + 10))
            rar = carddata.rarity(vk) if CARDDATA_OK else "common"
            rc = {"common": (150,150,150),"uncommon": (110,170,120),"rare": (90,140,210),
                  "epic": (180,110,210),"legendary": (230,180,70)}.get(rar,(160,160,160))
            rl = sfont.render(rar.upper(), True, rc)
            screen.blit(rl, (r.centerx - rl.get_width() / 2, prev.bottom + 34))
        else:
            # a wax-sealed mystery envelope
            pygame.draw.rect(screen, shade(PAGE, 0.7), prev, border_radius=8)
            pygame.draw.rect(screen, shade(INK, 3.0), prev, 2, border_radius=8)
            qm = bfont.render("?", True, shade(INK, 2.4))
            screen.blit(qm, (prev.centerx - qm.get_width() / 2, prev.centery - qm.get_height() / 2))
        HOT["paths"].append((r, i))
    if ev["chosen"] is not None:
        button(screen, pygame.Rect(b.centerx - 90, b.bottom - 62, 180, 46), "Continue", font, mouse, key="fcontinue")
    else:
        hint = sfont.render("Tap a path to take it.", True, shade(INK, 2.2))
        screen.blit(hint, (b.centerx - hint.get_width() / 2, b.bottom - 52))
    menu_vignette(screen)

def draw_towers(screen, fonts, t, mouse):
    font, bfont, sfont = fonts
    draw_desk_and_book(screen, t, "DEFENSES", "swap your flank towers for a troop")
    b = MENU_BOOK
    HOT.clear(); HOT["towers"] = []
    # a preview mini-tower drawn with each card so differences read at a glance
    cols = 2
    cw = (b.width - 80 - (cols - 1) * 20) // cols
    ch = 92
    gx0, gy0 = b.left + 40, b.top + 110
    for i, tk in enumerate(TOWER_ORDER):
        d = TOWER_TROOPS[tk]
        col, row = i % cols, i // cols
        r = pygame.Rect(gx0 + col * (cw + 20), gy0 + row * (ch + 12), cw, ch)
        if r.bottom > b.bottom - 70: continue
        sel = (TOWER_TROOP == tk); hover = r.collidepoint(mouse)
        pygame.draw.rect(screen, shade(PAGE, 0.95 if sel else (0.84 if hover else 0.74)), r, border_radius=10)
        pygame.draw.rect(screen, GOLD if sel else shade(INK, 3.2), r, 3 if sel else 2, border_radius=10)
        # roof-color chip
        pygame.draw.circle(screen, d["color"], (r.left + 28, r.centery), 16)
        pygame.draw.polygon(screen, shade(d["color"], 1.2),
                            [(r.left + 16, r.centery), (r.left + 28, r.centery - 16), (r.left + 40, r.centery)])
        nm = font.render(d["name"], True, INK)
        screen.blit(nm, (r.left + 56, r.top + 12))
        bl = sfont.render(d["blurb"], True, shade(INK, 2.0))
        screen.blit(bl, (r.left + 56, r.top + 38))
        # stat line
        wl = {"bolt":"single","splash":"splash","twin":"twin-shot","frost":"chill",
              "mortar":"mortar","none":"support"}[d["weapon"]]
        stat = sfont.render(f"HP {d['hp']}  ·  DMG {d['dmg']}  ·  RNG {d['rng']}  ·  {wl}",
                            True, shade(INK, 1.8))
        screen.blit(stat, (r.left + 56, r.top + 60))
        if d["special"]:
            sp = sfont.render("★ " + d["special"], True, shade(GOLD, 1.1))
            screen.blit(sp, (r.right - sp.get_width() - 14, r.top + 12))
        HOT["towers"].append((r, tk))
    button(screen, pygame.Rect(b.right - 178, b.bottom - 60, 140, 44), "Back", font, mouse, key="back")
    menu_vignette(screen)

def draw_collection(screen, fonts, t, mouse):
    font, bfont, sfont = fonts
    owned = PBCOLLECTION.owned_keys() if COLLECTION_OK else list(UNLOCKS)
    total = len(ALL_CARDS)
    draw_desk_and_book(screen, t, "COLLECTION",
                       f"{len(owned)} cards found  ·  {total} in rotation")
    b = MENU_BOOK
    HOT.clear(); HOT["colcards"] = []
    # sort: rarity desc, then cost
    def sortkey(k):
        base = carddata.base_of(k) if CARDDATA_OK else k
        rr = carddata.rarity_rank(carddata.rarity(k)) if CARDDATA_OK else 0
        return (-rr, CARDS.get(k, {}).get("cost", 0), base)
    owned_sorted = sorted(owned, key=sortkey)
    cols, cw, ch, gap = 6, None, None, 10
    cw = (b.width - 60 - (cols - 1) * gap) // cols
    ch = int(cw * 1.18)
    gx0, gy0 = b.left + 30, b.top + 116
    rar_col = {"common": (150,150,150),"uncommon": (110,170,120),"rare": (90,140,210),
               "epic": (180,110,210),"legendary": (230,180,70)}
    shown = 0
    for i, key in enumerate(owned_sorted):
        col, row = i % cols, i // cols
        r = pygame.Rect(gx0 + col * (cw + gap), gy0 + row * (ch + 8), cw, ch)
        if r.bottom > b.bottom - 70: break
        shown += 1
        if key in CARDS:
            draw_card(screen, key, r, font, sfont, True)
            rar = carddata.rarity(key) if CARDDATA_OK else "common"
            rc = rar_col.get(rar, (150,150,150))
            pygame.draw.rect(screen, rc, r, 3, border_radius=8)
            # variant ribbon
            if CARDDATA_OK and "#" in key:
                vlab = sfont.render(carddata.variant_of(key).capitalize(), True, PAGE)
                rb = pygame.Rect(r.left, r.top, vlab.get_width() + 12, 18)
                pygame.draw.rect(screen, rc, rb, border_radius=4)
                screen.blit(vlab, (r.left + 6, r.top + 1))
            cnt = PBCOLLECTION.count(key) if COLLECTION_OK else 1
            if cnt > 1:
                ct = sfont.render(f"x{cnt}", True, GOLD)
                screen.blit(ct, (r.right - ct.get_width() - 6, r.bottom - 20))
            HOT["colcards"].append((r, key))
    # discovery log strip
    if COLLECTION_OK:
        log = PBCOLLECTION.recent_log(6)
        ly = b.bottom - 58
        lab = sfont.render("Recently found:", True, shade(INK, 2.2))
        screen.blit(lab, (b.left + 30, ly - 18))
        lx = b.left + 30
        for entry in log:
            nm = CARDS.get(entry["key"], {}).get("name", entry["key"])[:14]
            chip = sfont.render(f"{nm} ({entry['how']})", True, shade(INK, 1.9))
            pygame.draw.rect(screen, shade(PAGE, 0.8), (lx - 4, ly, chip.get_width() + 8, 22), border_radius=5)
            screen.blit(chip, (lx, ly + 2))
            lx += chip.get_width() + 18
            if lx > b.right - 200: break
    button(screen, pygame.Rect(b.right - 178, b.bottom - 60, 140, 44), "Back", font, mouse, key="back")
    menu_vignette(screen)

def draw_options(screen, fonts, t, mouse):
    font, bfont, sfont = fonts
    draw_desk_and_book(screen, t, "OPTIONS", "graphics & mobile")
    b = MENU_BOOK
    HOT.clear(); HOT["gfx"] = []; HOT["toggles"] = []
    # ---- Graphics preset selector (left column)
    head = bfont.render("Graphics", True, INK)
    screen.blit(head, (b.left + 44, b.top + 108))
    labels = {"low": "Low · fast flat paper",
              "photorealistic": "Photoreal · bloom + soft shadows"}
    for i, preset in enumerate(GFX_PRESETS):
        r = pygame.Rect(b.left + 44, b.top + 150 + i * 66, 400, 56)
        sel = (GFX == preset); hover = r.collidepoint(mouse)
        pygame.draw.rect(screen, shade(PAGE, 0.92 if sel else (0.82 if hover else 0.72)), r, border_radius=10)
        pygame.draw.rect(screen, GOLD if sel else shade(INK, 3.2), r, 3 if sel else 2, border_radius=10)
        pygame.draw.circle(screen, shade(INK, 3.0), (r.left + 26, r.centery), 11, 2)
        if sel: pygame.draw.circle(screen, GOLD, (r.left + 26, r.centery), 6)
        screen.blit(font.render(preset.capitalize(), True, INK), (r.left + 50, r.top + 8))
        screen.blit(sfont.render(labels[preset], True, shade(INK, 2.0)), (r.left + 50, r.top + 32))
        chip = pygame.Rect(r.right - 92, r.top + 12, 78, 32)
        _preset_swatch(screen, chip, preset)
        HOT["gfx"].append((r, preset))

    # ---- Mobile & power (right column) — a stack of toggle rows
    head2 = bfont.render("Mobile & Power", True, INK)
    screen.blit(head2, (b.centerx + 30, b.top + 108))
    rows = [
        ("fps",   "Frame rate", {12: "12 fps · cartoon", 30: "30 fps · battery",
                                  60: "60 fps · smooth"}.get(FPS_CAP, f"{FPS_CAP} fps · high-refresh")),
        ("ssample","Supersampling", ("On · crisp (photoreal)" if SUPERSAMPLE else "Off")
                    + ("" if gfx_hi() else " — needs photoreal")),
        ("piecescale","Piece size", f"{PIECE_SCALE_LABELS[PIECE_SCALE_I % len(PIECE_SCALE_STEPS)]} · tap to enlarge units & towers"),
        ("orient","Layout", "Reflows to your screen — portrait & landscape"),
        ("pausebg","Pause in background", "On — saves battery" if PAUSE_BG else "Off"),
        ("battsave","Battery saver", "On — throttles menus to save power" if BATTERY_SAVER else "Off · full framerate everywhere"),
        ("sigilbadges","Ability badges", "On — show sigil icons above units" if SIGIL_BADGES else "Off · cleaner board"),
    ]
    for i, (key, name, val) in enumerate(rows):
        r = pygame.Rect(b.centerx + 30, b.top + 150 + i * 52, b.right - (b.centerx + 30) - 44, 46)
        hover = r.collidepoint(mouse)
        pygame.draw.rect(screen, shade(PAGE, 0.82 if hover else 0.72), r, border_radius=10)
        pygame.draw.rect(screen, shade(INK, 3.2), r, 2, border_radius=10)
        screen.blit(font.render(name, True, INK), (r.left + 16, r.top + 4))
        screen.blit(sfont.render(val, True, shade(INK, 2.0)), (r.left + 16, r.top + 26))
        # a little pill toggle on the right (piecescale shows its % instead)
        on = (key == "fps" and FPS_CAP != 60) or (key == "orient" and ORIENT_LOCK == "landscape") \
             or (key == "pausebg" and PAUSE_BG) or (key == "ssample" and SUPERSAMPLE) \
             or (key == "battsave" and BATTERY_SAVER) \
             or (key == "sigilbadges" and SIGIL_BADGES)
        if key == "piecescale":
            pill = pygame.Rect(r.right - 78, r.centery - 13, 62, 26)
            active = PIECE_SCALE_I != 0
            pygame.draw.rect(screen, GOLD if active else shade(INK, 2.6), pill, border_radius=13)
            lbl = sfont.render(PIECE_SCALE_LABELS[PIECE_SCALE_I % len(PIECE_SCALE_STEPS)],
                               True, shade(PAGE, 0.3) if active else PAGE)
            screen.blit(lbl, (pill.centerx - lbl.get_width() / 2, pill.centery - lbl.get_height() / 2))
            HOT["toggles"].append((r, key))
            continue
        pill = pygame.Rect(r.right - 66, r.centery - 13, 50, 26)
        pygame.draw.rect(screen, GOLD if on else shade(INK, 2.6), pill, border_radius=13)
        knob = (pill.right - 13) if on else (pill.left + 13)
        pygame.draw.circle(screen, PAGE, (knob, pill.centery), 10)
        HOT["toggles"].append((r, key))

    note = sfont.render("Touch, mouse and up to two controllers are always active.",
                        True, shade(INK, 2.3))
    screen.blit(note, (b.left + 44, b.bottom - 116))

    # ── audio: master volume slider + mute (full width along the bottom) ──
    ahead = bfont.render("Audio", True, INK)
    screen.blit(ahead, (b.left + 44, b.bottom - 190))
    if not (SOUND_OK and sound and sound.available):
        na = sfont.render("Audio device unavailable on this build.", True, shade(INK, 2.4))
        screen.blit(na, (b.left + 150, b.bottom - 184))
    # mute pill
    mr = pygame.Rect(b.left + 150, b.bottom - 188, 92, 30)
    hov = mr.collidepoint(mouse)
    pygame.draw.rect(screen, shade(PAGE, 0.9 if hov else 0.74), mr, border_radius=8)
    pygame.draw.rect(screen, GOLD if MUTED else shade(INK, 3.0), mr, 2, border_radius=8)
    screen.blit(sfont.render("Muted" if MUTED else "Sound On", True, INK),
                (mr.left + 10, mr.centery - 8))
    HOT["mute"] = mr
    # slider track
    track = pygame.Rect(b.left + 150, b.bottom - 150, b.right - (b.left + 150) - 60, 10)
    pygame.draw.rect(screen, shade(INK, 2.4), track, border_radius=5)
    fillw = int(track.w * VOLUME)
    pygame.draw.rect(screen, GOLD, (track.x, track.y, fillw, track.h), border_radius=5)
    # knob
    knob_x = track.x + fillw
    pygame.draw.circle(screen, PAGE, (knob_x, track.centery), 11)
    pygame.draw.circle(screen, shade(INK, 2.0), (knob_x, track.centery), 11, 2)
    vlabel = sfont.render(f"Volume  {int(VOLUME * 100)}%", True, shade(INK, 1.8))
    screen.blit(vlabel, (track.x, track.y - 26))
    # a hit rect a bit taller than the visual track for easy grabbing
    HOT["volslider"] = pygame.Rect(track.x, track.y - 12, track.w, 34)

    button(screen, pygame.Rect(b.right - 190, b.bottom - 66, 150, 48), "Back", font, mouse, key="back")
    if EXPANSIONS_OK:
        button(screen, pygame.Rect(b.right - 400, b.bottom - 66, 190, 48),
               "Expansions…", font, mouse, key="expansions")
    menu_vignette(screen)

def draw_expansions(screen, fonts, t, mouse):
    """The set/pack toggle screen — rows are owned by expansions.Scene."""
    font, bfont, sfont = fonts
    draw_desk_and_book(screen, t, "EXPANSIONS", "toggle card sets & content packs")
    b = MENU_BOOK
    HOT.clear()
    global EXPANSIONS_SCENE
    if EXPANSIONS_SCENE is None:
        EXPANSIONS_SCENE = expmod.Scene(b)
    EXPANSIONS_SCENE.draw(screen, fonts, mouse, shade, PAGE, INK, GOLD)
    button(screen, pygame.Rect(b.right - 190, b.bottom - 66, 150, 48), "Back", font, mouse, key="back")
    menu_vignette(screen)

def draw_lobby_waiting(screen, fonts, t, mouse, mp_state):
    """Waiting-room shown after hosting or joining, before the match starts.
    Shows your assigned team, deck-slot picker, and connected players."""
    font, bfont, sfont = fonts
    draw_desk_and_book(screen, t, "WAITING ROOM", "pick your deck and ready up")
    b = MENU_BOOK
    HOT.clear(); HOT["lobbyslots"] = []
    role = mp_state.get("role", "host")
    mm = mp_state.get("match_mode", "1v1")
    peer = mp_state.get("peer")

    # ── your assignment ──
    my_seats = mp_state.get("my_seats") or ([0] if role == "host" else [2])
    team = 0 if (my_seats and my_seats[0] < 2) else 1
    tcol = P_ACCENT if team == 0 else E_ACCENT
    tag = bfont.render(f"You are {'HOST' if role=='host' else 'GUEST'} · Team {team+1}",
                       True, shade(tcol, 1.3))
    screen.blit(tag, (b.left + 60, b.top + 110))
    tinfo = sfont.render(f"Mode: {mm}   ·   seats: {my_seats}", True, shade(INK, 2.0))
    screen.blit(tinfo, (b.left + 60, b.top + 146))

    # ── deck-slot picker: choose which of your 10 decks to bring ──
    dl = sfont.render("Choose your deck:", True, shade(INK, 2.0))
    screen.blit(dl, (b.left + 60, b.top + 186))
    sw = 54; sgap = 8
    sx0 = b.left + 60
    for i in range(10):
        col = i % 5; row = i // 5
        r = pygame.Rect(sx0 + col * (sw + sgap), b.top + 214 + row * 46, sw, 38)
        sel = (DECK_SLOT == i); hover = r.collidepoint(mouse)
        filled = len(DECK_SLOTS[i]) >= 8
        pygame.draw.rect(screen, shade(PAGE, 0.95 if sel else (0.82 if hover else 0.7)), r, border_radius=6)
        pygame.draw.rect(screen, GOLD if sel else shade(INK, 3.3), r, 2 if sel else 1, border_radius=6)
        lab = font.render(str(i + 1), True, INK)
        screen.blit(lab, (r.centerx - lab.get_width() / 2, r.centery - lab.get_height() / 2))
        if filled and not sel:
            pygame.draw.circle(screen, P_ACCENT, (r.right - 7, r.top + 7), 3)
        HOT["lobbyslots"].append((r, i))
    # preview the chosen deck's cards
    py = b.top + 320
    pv = sfont.render(f"Deck slot {DECK_SLOT+1}:  " +
                      ", ".join(CARDS[k]["name"] for k in DECK[:4]), True, shade(INK, 1.8))
    screen.blit(pv, (b.left + 60, py))
    pv2 = sfont.render("            " + ", ".join(CARDS[k]["name"] for k in DECK[4:8]), True, shade(INK, 1.8))
    screen.blit(pv2, (b.left + 60, py + 22))

    # ── connected players / seat fill ──
    rx = b.centerx + 40
    ph = bfont.render("Players", True, INK)
    screen.blit(ph, (rx, b.top + 110))
    yy = b.top + 150
    if role == "host" and peer is not None:
        seats = getattr(peer, "seat_pool", [0, 2])
        for s in seats:
            owner = peer.seat_owner.get(s)
            who = "you (host)" if owner == "host" else ("open…" if owner is None else "guest")
            stm = 0 if s < 2 else 1
            c = P_ACCENT if stm == 0 else E_ACCENT
            pygame.draw.circle(screen, c, (rx + 8, yy + 8), 6)
            line = sfont.render(f"seat {s} · Team {stm+1} · {who}", True,
                                shade(INK, 1.8) if owner else shade(INK, 2.6))
            screen.blit(line, (rx + 24, yy)); yy += 28
        allin = peer.all_seats_filled()
        msg = "All seats filled — starting!" if allin else "Waiting for players to join…"
        screen.blit(sfont.render(msg, True, shade(GOLD, 1.1)), (rx, yy + 12))
    else:
        # a joining client shows whether it's waiting on host approval
        awaiting = getattr(peer, "awaiting", False) if peer else False
        denied = getattr(peer, "denied", False) if peer else False
        if denied:
            screen.blit(sfont.render("The host declined your request.", True, shade(WAX, 1.2)), (rx, yy))
        elif awaiting:
            screen.blit(sfont.render("Waiting for the host to approve you…", True, shade(GOLD, 1.1)), (rx, yy))
        else:
            screen.blit(sfont.render("Connected. Waiting for host to start…", True, shade(GOLD, 1.1)), (rx, yy))

    # ── host trust panel: pending join requests need Approve / Deny ──
    HOT["approve"] = []; HOT["deny"] = []
    if role == "host" and peer is not None and getattr(peer, "pending", None):
        py2 = b.top + 300
        screen.blit(bfont.render("Join requests", True, shade(WAX, 1.15)), (rx, py2)); py2 += 34
        for addr, info in list(peer.pending.items())[:3]:
            row = pygame.Rect(rx, py2, 320, 44)
            pygame.draw.rect(screen, shade(PAGE, 0.86), row, border_radius=8)
            pygame.draw.rect(screen, shade(WAX, 1.0), row, 2, border_radius=8)
            who = sfont.render(f"{info['user']}  ·  {info['ip']}", True, INK)
            screen.blit(who, (row.left + 8, row.top + 4))
            ok = pygame.Rect(row.right - 128, row.top + 22, 58, 18)
            no = pygame.Rect(row.right - 64, row.top + 22, 54, 18)
            pygame.draw.rect(screen, (60, 110, 70), ok, border_radius=5)
            pygame.draw.rect(screen, (120, 50, 46), no, border_radius=5)
            screen.blit(sfont.render("Approve", True, PAGE), (ok.left + 2, ok.top))
            screen.blit(sfont.render("Deny", True, PAGE), (no.left + 8, no.top))
            HOT["approve"].append((ok, addr)); HOT["deny"].append((no, addr))
            py2 += 50

    st = mp_state.get("status", "")
    if st:
        screen.blit(sfont.render(st, True, shade(GOLD, 1.1)), (b.left + 60, b.bottom - 96))
    button(screen, pygame.Rect(b.right - 190, b.bottom - 66, 150, 48), "Leave", font, mouse, key="leaveroom")
    menu_vignette(screen)

def _lan_field(mp_state):
    """Which text field is being edited: 'ip' or 'user' (or None)."""
    return mp_state.get("editing")

def _lan_text_insert(mp_state, text):
    """Insert filtered characters into whichever LAN field is focused. IP field
    accepts digits + dots; username accepts most printable characters."""
    field = mp_state.get("editing")
    if not field or not text:
        return
    if field == "ip":
        for ch in text:
            if (ch.isdigit() or ch == ".") and len(mp_state.get("ip_input", "")) < 21:
                mp_state["ip_input"] = mp_state.get("ip_input", "") + ch
    elif field == "user":
        for ch in text:
            if ch.isprintable() and ch not in '"\\' and len(mp_state.get("user", "")) < 18:
                mp_state["user"] = mp_state.get("user", "") + ch

def _lan_text_backspace(mp_state):
    field = mp_state.get("editing")
    if field == "ip":
        mp_state["ip_input"] = mp_state.get("ip_input", "")[:-1]
    elif field == "user":
        mp_state["user"] = mp_state.get("user", "")[:-1]

def _begin_edit(mp_state, field, rect=None):
    """Focus a LAN text field and summon the on-screen keyboard (Wayland/mobile)."""
    # if we're leaving the username field, remember it
    if mp_state.get("editing") == "user" and field != "user" and COLLECTION_OK:
        try: PBCOLLECTION.set_pref("username", mp_state.get("user", ""))
        except Exception: pass
    mp_state["editing"] = field
    try:
        pygame.key.start_text_input()
        if rect is not None:
            pygame.key.set_text_input_rect(rect)
    except Exception:
        pass

def draw_multiplayer(screen, fonts, t, mouse, mp_state):
    """LAN lobby. mp_state is a dict the main loop owns:
       {role, status, ip_input, editing, peer, local_ips}"""
    font, bfont, sfont = fonts
    draw_desk_and_book(screen, t, "MULTIPLAYER", "share a page across the table (LAN)")
    b = MENU_BOOK
    HOT.clear()
    if not NET_OK:
        warn = font.render("Networking unavailable on this build.", True, WAX)
        screen.blit(warn, (b.centerx - warn.get_width() / 2, b.centery))
        button(screen, pygame.Rect(b.right - 190, b.bottom - 66, 150, 48), "Back", font, mouse, key="back")
        menu_vignette(screen); return

    # ── match mode toggle (1v1 / 2v2) + "two players here" ──
    my = b.top + 76
    mm = mp_state.get("match_mode", "1v1")
    twoHere = mp_state.get("two_here", False)
    lbl = sfont.render("Match:", True, shade(INK, 2.0))
    screen.blit(lbl, (b.left + 60, my + 6))
    HOT["mmodes"] = []
    mxx = b.left + 130
    for mk, txt in (("1v1", "1v1"), ("2v2", "2v2 (teams)")):
        w = 14 + sfont.size(txt)[0]
        r = pygame.Rect(mxx, my, w, 30)
        sel = (mm == mk); hov = r.collidepoint(mouse)
        pygame.draw.rect(screen, shade(PAGE, 0.95 if sel else (0.82 if hov else 0.7)), r, border_radius=15)
        pygame.draw.rect(screen, GOLD if sel else shade(INK, 3.3), r, 2, border_radius=15)
        tt = sfont.render(txt, True, INK)
        screen.blit(tt, (r.centerx - tt.get_width() / 2, r.centery - tt.get_height() / 2))
        HOT["mmodes"].append((r, mk)); mxx += w + 8
    if mm == "2v2":
        # a toggle for running two local players on this device
        tr = pygame.Rect(mxx + 16, my, 210, 30)
        hov = tr.collidepoint(mouse)
        pygame.draw.rect(screen, shade(PAGE, 0.9 if hov else 0.72), tr, border_radius=8)
        pygame.draw.rect(screen, GOLD if twoHere else shade(INK, 3.3), tr, 2, border_radius=8)
        tt = sfont.render(("✓ " if twoHere else "  ") + "two players on this device", True, INK)
        screen.blit(tt, (tr.left + 8, tr.centery - tt.get_height() / 2))
        HOT["twohere"] = tr

    # ── channel (transport) picker: HTTP is firewall-friendly, UDP is fast ──
    chan = mp_state.get("channel", "http")
    clbl = sfont.render("Channel:", True, shade(INK, 2.0))
    screen.blit(clbl, (b.left + 60, my + 40))
    HOT["channels"] = []
    cxx = b.left + 140
    for ck, ctxt in (("http", "HTTP · firewall-friendly"), ("udp", "UDP · fast")):
        w = 14 + sfont.size(ctxt)[0]
        r = pygame.Rect(cxx, my + 34, w, 28)
        sel = (chan == ck); hov = r.collidepoint(mouse)
        pygame.draw.rect(screen, shade(PAGE, 0.95 if sel else (0.82 if hov else 0.7)), r, border_radius=14)
        pygame.draw.rect(screen, GOLD if sel else shade(INK, 3.3), r, 2, border_radius=14)
        tt = sfont.render(ctxt, True, INK)
        screen.blit(tt, (r.centerx - tt.get_width() / 2, r.centery - tt.get_height() / 2))
        HOT["channels"].append((r, ck)); cxx += w + 8

    # left page: HOST
    lx = b.left + 60
    hh = bfont.render("Host a game", True, INK)
    screen.blit(hh, (lx, b.top + 120))
    info = sfont.render("Start a table and wait for a friend to join.", True, shade(INK, 2.2))
    screen.blit(info, (lx, b.top + 160))
    chan = mp_state.get("channel", "http")
    import transport as _tp_mod
    chan_port = (_tp_mod.DEFAULT_HTTP_PORT if chan == "http" else DEFAULT_PORT)
    proto = "TCP" if chan == "http" else "UDP"
    button(screen, pygame.Rect(lx, b.top + 196, 260, 52),
           "Host (%s %d)" % (proto, chan_port), font, mouse, key="host")
    # show our LAN IPs so the other player knows what to type
    ipy = b.top + 268
    il = sfont.render("Your LAN address(es):", True, shade(INK, 2.2))
    screen.blit(il, (lx, ipy)); ipy += 24
    for ip in mp_state.get("local_ips", []):
        chip = sfont.render("  " + ip, True, GOLD)
        screen.blit(chip, (lx, ipy)); ipy += 22

    # right page: JOIN + SCAN
    rx = b.centerx + 40
    jh = bfont.render("Join a game", True, INK)
    screen.blit(jh, (rx, b.top + 120))
    button(screen, pygame.Rect(rx, b.top + 156, 150, 40), "Scan LAN", sfont, mouse, key="scan")
    scanning = mp_state.get("scanning", False)
    if scanning:
        screen.blit(sfont.render("scanning…", True, shade(GOLD, 1.1)), (rx + 164, b.top + 166))
    # discovered hosts list
    HOT["hosts"] = []
    hy = b.top + 206
    hosts = mp_state.get("hosts", [])
    if hosts:
        for h in hosts[:4]:
            r = pygame.Rect(rx, hy, 300, 40)
            hover = r.collidepoint(mouse)
            pygame.draw.rect(screen, shade(PAGE, 0.9 if hover else 0.78), r, border_radius=8)
            pygame.draw.rect(screen, shade(INK, 3.2), r, 2, border_radius=8)
            nm = sfont.render(f"{h['name']} · {h['mode']} · {h['seats_open']} open", True, INK)
            screen.blit(nm, (r.left + 10, r.top + 4))
            ipl = sfont.render(h["ip"], True, shade(INK, 2.2))
            screen.blit(ipl, (r.left + 10, r.top + 22))
            HOT["hosts"].append((r, h))
            hy += 46
    else:
        screen.blit(sfont.render("No tables found yet — Scan, or type an IP below.",
                                 True, shade(INK, 2.4)), (rx, hy)); hy += 30
    # manual IP field
    fld = pygame.Rect(rx, hy + 6, 220, 40)
    editing_ip = (mp_state.get("editing") == "ip")
    pygame.draw.rect(screen, shade(PAGE, 0.9), fld, border_radius=8)
    pygame.draw.rect(screen, GOLD if editing_ip else shade(INK, 3.2), fld, 2, border_radius=8)
    ip_txt = mp_state.get("ip_input", "") or "192.168.___.___"
    show = ip_txt + ("|" if (editing_ip and int(t * 2) % 2 == 0) else "")
    it = sfont.render(show, True, INK if mp_state.get("ip_input") else shade(INK, 2.6))
    screen.blit(it, (fld.left + 10, fld.centery - it.get_height() / 2))
    HOT["ipfield"] = fld
    button(screen, pygame.Rect(fld.right + 10, hy + 6, 90, 40), "Join", sfont, mouse, key="join")

    # ── on-screen numpad: the bulletproof way to enter an IP on a touch phone
    #    where a Wayland virtual keyboard may be missing or blocked. Digits + dot
    #    + backspace, big tap targets. Always available; no OSK required. ──
    HOT["numpad"] = []
    npx, npy = rx, hy + 54
    keys = ["1", "2", "3", "4", "5", "6", "7", "8", "9", ".", "0", "⌫"]
    bw2, bh2, gp2 = 44, 34, 6
    for i, kk in enumerate(keys):
        col, row = i % 3, i // 3
        r = pygame.Rect(npx + col * (bw2 + gp2), npy + row * (bh2 + gp2), bw2, bh2)
        hov = r.collidepoint(mouse)
        pygame.draw.rect(screen, shade(PAGE, 0.92 if hov else 0.8), r, border_radius=6)
        pygame.draw.rect(screen, shade(INK, 3.0), r, 1, border_radius=6)
        kt = sfont.render(kk, True, INK)
        screen.blit(kt, (r.centerx - kt.get_width() / 2, r.centery - kt.get_height() / 2))
        HOT["numpad"].append((r, kk))
    # spectate button beside the numpad
    button(screen, pygame.Rect(npx + 3 * (bw2 + gp2) + 12, npy, 120, 34), "Spectate", sfont, mouse, key="spectate")

    # ── username field (top-right of the page) — who you are at the table ──
    ufld = pygame.Rect(rx, b.top + 120, 230, 34)
    editing_user = (mp_state.get("editing") == "user")
    ulabel = sfont.render("You:", True, shade(INK, 2.0))
    # (drawn later so it sits above the Join column; keep simple: put at top)
    pygame.draw.rect(screen, shade(PAGE, 0.9), ufld, border_radius=8)
    pygame.draw.rect(screen, GOLD if editing_user else shade(INK, 3.2), ufld, 2, border_radius=8)
    uname = mp_state.get("user", "") or "tap to set name"
    ushow = uname + ("|" if (editing_user and int(t * 2) % 2 == 0) else "")
    ut = sfont.render(ushow, True, INK if mp_state.get("user") else shade(INK, 2.6))
    screen.blit(ut, (ufld.left + 10, ufld.centery - ut.get_height() / 2))
    HOT["userfield"] = ufld

    # ── firewall / connectivity sane-defaults hint (host side) ──
    chan2 = mp_state.get("channel", "http")
    _cport = (_tp_mod.DEFAULT_HTTP_PORT if chan2 == "http" else DEFAULT_PORT)
    _cproto = "tcp" if chan2 == "http" else "udp"
    ht = sfont.render("If peers can't connect, allow %s %d in your firewall."
                      % (_cproto.upper(), _cport), True, shade(INK, 2.5))
    screen.blit(ht, (lx, b.bottom - 150))
    if chan2 == "http":
        ht2 = sfont.render("HTTP channel: works through most firewalls; advertises via mDNS/Avahi.",
                           True, shade(INK, 2.7))
        screen.blit(ht2, (lx, b.bottom - 130))

    # status line
    st = mp_state.get("status", "")
    if st:
        stimg = font.render(st, True, shade(GOLD, 1.1))
        screen.blit(stimg, (b.centerx - stimg.get_width() / 2, b.bottom - 116))

    button(screen, pygame.Rect(b.right - 190, b.bottom - 66, 150, 48), "Back", font, mouse, key="back")
    menu_vignette(screen)

def _preset_swatch(screen, rect, preset):
    """A little live preview chip showing the flavour of each preset."""
    surf = pygame.Surface((rect.w, rect.h))
    if preset == "low":
        surf.fill((40, 30, 24))
        pygame.draw.circle(surf, (235, 205, 162), (rect.w - 20, rect.h // 2), 14)
    else:
        # gradient + bloom-ish core
        for x in range(rect.w):
            k = x / rect.w
            col = (int(28 + 40 * k), int(20 + 26 * k), int(16 + 18 * k))
            pygame.draw.line(surf, col, (x, 0), (x, rect.h))
        cx, cy = rect.w - 20, rect.h // 2
        for r in range(16, 0, -1):
            a = (1 - r / 16) ** 1.5
            col = tuple(min(255, int(c * a)) for c in (255, 214, 150))
            pygame.draw.circle(surf, col, (cx, cy), r)
    screen.blit(surf, rect.topleft)
    pygame.draw.rect(screen, shade(INK, 3.0), rect, 2)

DECK_FILTER = "all"      # all | troop | building | flying
DECK_SORT = "cost"       # cost | name
DECK_DETAIL = None       # card key pinned in the detail popup (tap to pin)
DECK_PAGE = 0            # current page of the card grid
DECK_STACK = {}          # base_key -> index into its owned-variant stack
DECK_STACK_ANIM = {}     # base_key -> (from_idx, t0) for the rotate animation
_DECK_DRAG = None        # {key,x0,base} while dragging a stack to cycle it

def _owned_variants(base_key):
    """All owned printings of a base card (standard + any rare variants), as a
    stack ordered standard-first. Falls back to just the base if collection is
    unavailable. This is what the deck grid cycles through.

    While a holiday event is active it also offers that event's seasonal skin for
    EVERY card (built on demand), so players can swipe a card into its themed form
    and back — available to everyone on the day, no unlock needed."""
    variants = [base_key]
    if COLLECTION_OK:
        try:
            for k in PBCOLLECTION.owned_keys():
                if "#" in k and carddata.base_of(k) == base_key:
                    if k not in variants:
                        variants.append(k)
        except Exception:
            pass
    # seasonal skin (event-gated, free for the duration)
    if SEASONAL_VARIANT_ID and CARDDATA_OK:
        try:
            sk = carddata.make_variant(base_key, SEASONAL_VARIANT_ID)
            if sk not in variants:
                variants.append(sk)
        except Exception:
            pass
    return variants

def _deck_matches_filter(key):
    d = CARDS[key]
    if DECK_FILTER == "all": return True
    if DECK_FILTER == "building": return d.get("building", False)
    if DECK_FILTER == "flying": return d.get("flying", False)
    if DECK_FILTER == "troop":
        return not d.get("building", False) and not d.get("flying", False)
    return True

def _deck_sorted_keys():
    keys = [k for k in ALL_CARDS if _deck_matches_filter(k)]
    if DECK_SORT == "cost":
        keys.sort(key=lambda k: (CARDS[k]["cost"], CARDS[k]["name"]))
    else:
        keys.sort(key=lambda k: CARDS[k]["name"])
    return keys

# grid geometry — two rows so the pager/detail controls never overlap the third
# row (which was unreadable on desktop). Bigger cards, cleaner page.
_DB_COLS, _DB_ROWS = 6, 2
_DB_PER_PAGE = _DB_COLS * _DB_ROWS       # 12 cards per page

def _random_deck(rng=None):
    """Build a fresh 8-card deck from the cards you own, with a light hand on the
    cost curve so it's playable rather than eight 9-drops: aim for a couple of
    cheap cards, a mid spine, and at most a couple of expensive finishers.
    Falls back to the whole roster if the collection isn't available."""
    rng = rng or random
    pool = [k for k in ALL_CARDS if unlocked(k)]
    if len(pool) < 8:
        pool = ALL_CARDS[:]
    cheap = [k for k in pool if CARDS[k]["cost"] <= 3]
    mid = [k for k in pool if 4 <= CARDS[k]["cost"] <= 6]
    pricey = [k for k in pool if CARDS[k]["cost"] >= 7]
    deck = []
    def take(bucket, n):
        rng.shuffle(bucket)
        for k in bucket[:n]:
            if k not in deck and len(deck) < 8:
                deck.append(k)
    take(cheap, rng.randint(2, 3))       # a couple of cheap cards to cycle
    take(pricey, rng.randint(1, 2))      # one or two heavy hitters
    take(mid, 8 - len(deck))             # fill the spine with mid-cost cards
    # if buckets were thin, top up from anything owned
    if len(deck) < 8:
        rest = [k for k in pool if k not in deck]
        take(rest, 8 - len(deck))
    return deck[:8]

def draw_deckbuilder(screen, fonts, t, mouse, scroll):
    global DECK_DETAIL, DECK_PAGE
    font, bfont, sfont = fonts
    avg = (sum(CARDS[k]["cost"] for k in DECK) / len(DECK)) if DECK else 0
    keys = _deck_sorted_keys()
    npages = max(1, (len(keys) + _DB_PER_PAGE - 1) // _DB_PER_PAGE)
    DECK_PAGE = max(0, min(DECK_PAGE, npages - 1))
    draw_desk_and_book(screen, t, "DECK BUILDER",
                       f"slot {DECK_SLOT+1}/10 · {len(DECK)}/8 chosen · avg {avg:.1f} · page {DECK_PAGE+1}/{npages}")
    b = MENU_BOOK
    HOT.clear(); HOT["cards"] = []; HOT["filters"] = []; HOT["deckslots"] = []
    HOT["pager"] = []; HOT["slots"] = []

    # ── deck-slot selector: ten numbered chips along the very top ──
    sw = 46; sgap = 6
    sx0 = b.centerx - (10 * sw + 9 * sgap) / 2
    for i in range(10):
        r = pygame.Rect(sx0 + i * (sw + sgap), b.top + 66, sw, 30)
        sel = (DECK_SLOT == i); hover = r.collidepoint(mouse)
        filled = len(DECK_SLOTS[i]) >= 8 and DECK_SLOTS[i] != DEFAULT_DECK
        pygame.draw.rect(screen, shade(PAGE, 0.95 if sel else (0.82 if hover else 0.7)), r, border_radius=6)
        pygame.draw.rect(screen, GOLD if sel else shade(INK, 3.3), r, 2 if sel else 1, border_radius=6)
        lab = sfont.render(str(i + 1), True, INK)
        screen.blit(lab, (r.centerx - lab.get_width() / 2, r.centery - lab.get_height() / 2))
        if filled and not sel:
            pygame.draw.circle(screen, P_ACCENT, (r.right - 7, r.top + 7), 3)
        HOT["slots"].append((r, i))

    # ── top: filter + sort chips in a horizontal row (no side rail) ──
    fx = b.left + 30; fyr = b.top + 104
    for key, lbl in [("all", "All"), ("troop", "Troops"), ("building", "Buildings"),
                     ("flying", "Flying")]:
        r = pygame.Rect(fx, fyr, 104, 32)
        sel = (DECK_FILTER == key); hover = r.collidepoint(mouse)
        pygame.draw.rect(screen, shade(PAGE, 0.95 if sel else (0.82 if hover else 0.7)), r, border_radius=7)
        pygame.draw.rect(screen, GOLD if sel else shade(INK, 3.3), r, 2, border_radius=7)
        tx = sfont.render(lbl, True, INK)
        screen.blit(tx, (r.centerx - tx.get_width() / 2, r.centery - tx.get_height() / 2))
        HOT["filters"].append((r, ("filter", key)))
        fx += 112
    srt = pygame.Rect(fx + 8, fyr, 118, 32)
    hover = srt.collidepoint(mouse)
    pygame.draw.rect(screen, shade(PAGE, 0.82 if hover else 0.7), srt, border_radius=7)
    pygame.draw.rect(screen, shade(INK, 3.3), srt, 2, border_radius=7)
    stx = sfont.render(f"Sort: {DECK_SORT}", True, INK)
    screen.blit(stx, (srt.centerx - stx.get_width() / 2, srt.centery - stx.get_height() / 2))
    HOT["filters"].append((srt, ("sort", None)))
    # ── Random deck: rolls a fresh, curve-balanced 8-card deck from what you own ──
    rnd = pygame.Rect(srt.right + 10, fyr, 130, 32)
    hover = rnd.collidepoint(mouse)
    pygame.draw.rect(screen, shade((150, 120, 70), 1.05 if hover else 0.92), rnd, border_radius=7)
    pygame.draw.rect(screen, GOLD, rnd, 2, border_radius=7)
    rtx = sfont.render("⚄ Random", True, INK)
    screen.blit(rtx, (rnd.centerx - rtx.get_width() / 2, rnd.centery - rtx.get_height() / 2))
    HOT["filters"].append((rnd, ("random", None)))

    # ── card grid: two rows, each cell is a STACK of that card's owned printings
    #    (standard + rare variants). The top card shows; a stack gets a fanned
    #    edge + count, and cycling rotates through with a little hand animation. ──
    gy0 = b.top + 152
    avail_w = b.width - 60
    cw = (avail_w - (_DB_COLS - 1) * 12) // _DB_COLS
    ch = int(cw * 1.28)
    gx0 = b.left + 30
    page_keys = keys[DECK_PAGE * _DB_PER_PAGE:(DECK_PAGE + 1) * _DB_PER_PAGE]
    now = t
    for i, key in enumerate(page_keys):
        col, row = i % _DB_COLS, i // _DB_COLS
        r = pygame.Rect(gx0 + col * (cw + 12), gy0 + row * (ch + 14), cw, ch)
        if not unlocked(key):
            draw_torn_card(screen, r)
            continue
        stack = _owned_variants(key)
        vi = DECK_STACK.get(key, 0) % len(stack)
        shown = stack[vi]
        # fanned edges behind the top card to signal a stack (special finds)
        if len(stack) > 1:
            for d_i in range(min(len(stack) - 1, 3), 0, -1):
                off = d_i * 3
                edge = r.move(off, -off)
                pygame.draw.rect(screen, shade(PAGE, 0.6), edge, border_radius=8)
                pygame.draw.rect(screen, shade(INK, 3.4), edge, 1, border_radius=8)
        # rotate animation: slide/scale the top card briefly when the stack cycles
        anim = DECK_STACK_ANIM.get(key)
        dxr = 0; scale = 1.0
        if anim:
            at = (now - anim[1]) / 0.22
            if at >= 1.0:
                DECK_STACK_ANIM.pop(key, None)
            else:
                dxr = int((1 - at) * cw * 0.5 * anim[0])   # slide in from the side
                scale = 0.9 + 0.1 * at
        drawn = shown if shown in CARDS else key
        if scale != 1.0:
            tmp = pygame.Surface((r.w, r.h), pygame.SRCALPHA)
            draw_card(tmp, drawn, pygame.Rect(0, 0, r.w, r.h), font, sfont, key in DECK)
            sc = pygame.transform.smoothscale(tmp, (int(r.w*scale), int(r.h*scale)))
            screen.blit(sc, (r.x + dxr + (r.w-sc.get_width())//2, r.y + (r.h-sc.get_height())//2))
        else:
            draw_card(screen, drawn, r.move(dxr, 0), font, sfont, key in DECK)
        # in-deck highlight (any printing of this card counts)
        in_deck = any(carddata.base_of(dk) == key if COLLECTION_OK else dk == key for dk in DECK)
        if in_deck:
            pygame.draw.rect(screen, GOLD, r, 3, border_radius=8)
            pygame.draw.circle(screen, GOLD, (r.right - 12, r.top + 12), 9)
            screen.blit(sfont.render("✓", True, INK), (r.right - 17, r.top + 4))
        # stack count badge + variant name ribbon
        if len(stack) > 1:
            bdg = pygame.Rect(r.left + 6, r.bottom - 22, 34, 16)
            pygame.draw.rect(screen, shade(INK, 1.2), bdg, border_radius=8)
            screen.blit(_font(11).render(f"{vi+1}/{len(stack)}", True, PAGE), (bdg.x + 4, bdg.y + 1))
            if "#" in drawn and CARDDATA_OK:
                vlab = _font(11).render(carddata.variant_of(drawn).capitalize(), True, (240, 220, 130))
                screen.blit(vlab, (r.centerx - vlab.get_width() // 2, r.top + 4))
        if key == DECK_DETAIL:
            pygame.draw.rect(screen, shade(GOLD, 1.25), r, 2, border_radius=8)
        HOT["cards"].append((r, key))

    # page arrows (sit in the clear band beneath the two rows)
    if npages > 1:
        prev = pygame.Rect(b.left + 30, b.bottom - 150, 54, 40)
        nxt = pygame.Rect(b.left + 92, b.bottom - 150, 54, 40)
        for rr, lbl, tag in ((prev, "‹", "prev"), (nxt, "›", "next")):
            hover = rr.collidepoint(mouse)
            pygame.draw.rect(screen, shade(PAGE, 0.85 if hover else 0.72), rr, border_radius=8)
            pygame.draw.rect(screen, shade(INK, 3.2), rr, 2, border_radius=8)
            gl = bfont.render(lbl, True, INK)
            screen.blit(gl, (rr.centerx - gl.get_width() / 2, rr.centery - gl.get_height() / 2))
            HOT["pager"].append((rr, tag))

    # ── detail POPUP anchored next to the pinned card (not a bottom strip), so a
    #    dense page keeps its space. Flips to the card's left if it'd run off. ──
    if DECK_DETAIL and DECK_DETAIL in CARDS:
        anchor = next((rr for rr, k in HOT["cards"] if k == DECK_DETAIL), None)
        if anchor:
            stack = _owned_variants(DECK_DETAIL)
            vi = DECK_STACK.get(DECK_DETAIL, 0) % len(stack)
            dk = stack[vi] if stack[vi] in CARDS else DECK_DETAIL
            d = CARDS[dk]
            pw, ph = 220, 132
            px = anchor.right + 8
            if px + pw > b.right - 10:
                px = anchor.left - pw - 8
            py = max(b.top + 150, min(anchor.top, b.bottom - ph - 90))
            pop = pygame.Rect(px, py, pw, ph)
            pygame.draw.rect(screen, shade(PAGE, 0.82), pop, border_radius=10)
            pygame.draw.rect(screen, GOLD, pop, 2, border_radius=10)
            screen.blit(font.render(d["name"], True, INK), (pop.left + 12, pop.top + 8))
            yy = pop.top + 34
            for line in (f"cost {d['cost']}   hp {d['hp']}   dmg {d['dmg']}"
                         + (f"   x{d['count']}" if d.get("count", 1) > 1 else ""),):
                screen.blit(sfont.render(line, True, shade(INK, 1.8)), (pop.left + 12, yy)); yy += 22
            sc = SIGIL_COLORS.get(d["sigil"], GOLD)
            pygame.draw.circle(screen, sc, (pop.left + 18, yy + 8), 7)
            desc = f"{d['sigil'].capitalize()}: {SIGIL_DESC.get(d['sigil'],'')}"
            for wline in _wrap(desc, sfont, pw - 40):
                screen.blit(sfont.render(wline, True, shade(INK, 2.0)), (pop.left + 32, yy)); yy += 18
            if len(stack) > 1:
                screen.blit(_font(11).render("scroll / drag card to flip variants", True,
                            shade(INK, 2.4)), (pop.left + 12, pop.bottom - 18))

    # ── bottom: current deck strip with clickable slots (tap to remove) ──
    dy = b.bottom - 80
    lab = sfont.render("Your deck (tap a slot to remove):", True, shade(INK, 2.2))
    screen.blit(lab, (b.left + 30, dy - 22))
    for i in range(8):
        r = pygame.Rect(b.left + 30 + i * 76, dy, 68, 58)
        filled = i < len(DECK)
        pygame.draw.rect(screen, shade(PAGE, 0.72 if filled else 0.5), r, border_radius=6)
        pygame.draw.rect(screen, GOLD if filled else shade(INK, 3.5), r, 2, border_radius=6)
        if filled:
            d = CARDS[DECK[i]]
            nm = sfont.render(d["name"][:8], True, INK)
            screen.blit(nm, (r.centerx - nm.get_width() / 2, r.centery + 6))
            pygame.draw.circle(screen, INK, (r.left + 11, r.top + 11), 9)
            cc = sfont.render(str(d["cost"]), True, PAGE)
            screen.blit(cc, (r.left + 11 - cc.get_width() / 2, r.top + 11 - cc.get_height() / 2))
            sc = SIGIL_COLORS.get(d["sigil"], GOLD)
            pygame.draw.circle(screen, sc, (r.right - 11, r.top + 11), 5)
            HOT["deckslots"].append((r, i))

    # cost-curve histogram on the right of the deck row
    curve_x = b.left + 30 + 8 * 76 + 20
    if curve_x < b.right - 210:
        buckets = [0] * 11
        for k in DECK: buckets[min(10, CARDS[k]["cost"])] += 1
        cmax = max(buckets) or 1
        screen.blit(sfont.render("curve", True, shade(INK, 2.2)), (curve_x, dy - 22))
        for c in range(1, 9):
            bh = int(38 * buckets[c] / cmax)
            bar = pygame.Rect(curve_x + (c - 1) * 15, dy + 40 - bh, 11, bh)
            pygame.draw.rect(screen, RIVER_HI, bar, border_radius=2)
            nn = _font(10).render(str(c), True, shade(INK, 2.2))
            screen.blit(nn, (bar.centerx - nn.get_width() / 2, dy + 42))

    button(screen, pygame.Rect(b.right - 178, dy - 2, 140, 42), "Back", font, mouse, key="back")
    menu_vignette(screen)

def _wrap(text, fontobj, maxw):
    words = text.split()
    lines, cur = [], ""
    for w in words:
        trial = (cur + " " + w).strip()
        if fontobj.size(trial)[0] <= maxw:
            cur = trial
        else:
            if cur: lines.append(cur)
            cur = w
    if cur: lines.append(cur)
    return lines

def draw_arena_select(screen, fonts, t, mouse, sel):
    font, bfont, sfont = fonts
    draw_desk_and_book(screen, t, "CHOOSE ARENA", f"mode: {MODES[MODE]['name']} — {MODES[MODE]['blurb']}")
    b = MENU_BOOK
    HOT.clear(); HOT["arenas"] = []; HOT["modes"] = []

    # ── mode selector chips across the top (wraps to a second row) ──
    mx = b.left + 30; myr = b.top + 100
    for mk in MODE_ORDER:
        mc = MODES[mk]
        w = 8 + sfont.size(mc["name"])[0] + 16
        if mx + w > b.right - 30:          # wrap to the next row
            mx = b.left + 30; myr += 38
        r = pygame.Rect(mx, myr, w, 32)
        sel_m = (MODE == mk); hover = r.collidepoint(mouse)
        pygame.draw.rect(screen, shade(PAGE, 0.95 if sel_m else (0.82 if hover else 0.7)), r, border_radius=16)
        pygame.draw.rect(screen, GOLD if sel_m else shade(INK, 3.3), r, 2, border_radius=16)
        tx = sfont.render(mc["name"], True, INK)
        screen.blit(tx, (r.centerx - tx.get_width() / 2, r.centery - tx.get_height() / 2))
        HOT["modes"].append((r, mk))
        mx += w + 8
    mode_bottom = myr + 38

    # ── arena grid: 4 columns, compact tiles, fits the whole pool ──
    keys = list(ARENAS.keys())
    cols = 4
    gx0, gy0 = b.left + 30, mode_bottom + 8
    tw = (b.width - 60 - (cols - 1) * 12) // cols
    th = 74
    for i, k in enumerate(keys):
        a = ARENAS[k]
        col, row = i % cols, i // cols
        r = pygame.Rect(gx0 + col * (tw + 12), gy0 + row * (th + 10), tw, th)
        if r.bottom > b.bottom - 80: continue
        hover = r.collidepoint(mouse)
        pygame.draw.rect(screen, shade(a["page"], 1.08 if hover else 1.0), r, border_radius=8)
        pygame.draw.rect(screen, a["cover"], r, 3, border_radius=8)
        pygame.draw.circle(screen, a["lights"][0][2], (r.left + 22, r.top + 22), 11)
        nm = sfont.render(a["name"][:20], True, INK)
        screen.blit(nm, (r.left + 40, r.top + 10))
        tags = [x for x in (a.get("weather"), a.get("hazard"), a.get("poi")) if x]
        tg = _font(11).render(
            " · ".join(tags) if tags else "calm", True, shade(INK, 2.2))
        screen.blit(tg, (r.left + 40, r.top + 32))
        if boss_for(k):
            bt = _font(11, bold=True).render(
                "☠ " + boss_for(k), True, WAX)
            screen.blit(bt, (r.left + 12, r.bottom - 20))
        if k == sel:
            pygame.draw.rect(screen, GOLD, r, 3, border_radius=8)
        HOT["arenas"].append((r, k))

    button(screen, pygame.Rect(b.centerx - 220, b.bottom - 62, 200, 46), "Play This Arena", font, mouse, key="playarena")
    button(screen, pygame.Rect(b.centerx + 20, b.bottom - 62, 200, 46), "Back", font, mouse, key="back")
    menu_vignette(screen)

def draw_debug(screen, fonts, t, mouse):
    font, bfont, sfont = fonts
    draw_desk_and_book(screen, t, "CODEX / DEBUG", "testing tools")
    b = MENU_BOOK
    HOT.clear()
    status = "ON — all cards unlocked" if DEBUG_ALL else f"OFF — {len(UNLOCKS)}/{len(ALL_CARDS)} unlocked"
    st = font.render("Unlock all cards: " + status, True, INK)
    screen.blit(st, (b.left + 60, b.top + 140))
    button(screen, pygame.Rect(b.left + 60, b.top + 180, 320, 52),
           "Toggle Unlock-All", font, mouse, key="toggle")
    # a toggle for the balance-audit view (uses the boss forward-sim)
    audit_on = globals().get("SHOW_BALANCE", False)
    button(screen, pygame.Rect(b.left + 60, b.top + 244, 320, 46),
           ("Balance Audit: ON" if audit_on else "Balance Audit: OFF"),
           font, mouse, key="balance")
    # spirit-echo look toggle
    est = globals().get("ECHO_STYLE", "apparition")
    button(screen, pygame.Rect(b.left + 396, b.top + 244, 300, 46),
           f"Spirit Echoes: {est.title()}", font, mouse, key="echostyle")

    if audit_on and BOSSSIM_OK:
        _draw_balance_audit(screen, fonts, b, mouse)
    else:
        # default codex: every card's sigil
        sf = _font(13)
        for i, key in enumerate(ALL_CARDS):
            d = CARDS[key]
            col = INK if unlocked(key) else shade(INK, 3.2)
            x = b.left + 60 + (i // 11) * 300
            y = b.top + 320 + (i % 11) * 26
            if y > b.bottom - 90: continue
            pygame.draw.circle(screen, SIGIL_COLORS.get(d["sigil"], GOLD), (x, y + 7), 7)
            nm = sf.render(f"{d['name']} — {SIGIL_DESC.get(d['sigil'],'')}", True, col)
            screen.blit(nm, (x + 16, y))
    button(screen, pygame.Rect(b.right - 190, b.bottom - 66, 150, 48), "Back", font, mouse, key="back")
    menu_vignette(screen)

_VERDICT_COL = {"oppressive": (206, 92, 78), "weak": (150, 150, 170),
                "fair": (110, 180, 120), "?": (150, 150, 150)}
def _draw_balance_audit(screen, fonts, b, mouse):
    """A visual roster balance sheet from the boss forward-sim: each card's
    value-per-ink, survival, and a strong/fair/weak verdict, colour-coded."""
    font, bfont, sfont = fonts
    sf = _font(12)
    rows = boss_sim.audit_roster(CARDS, horizon=5.0)
    # summary counts
    counts = {"oppressive": 0, "fair": 0, "weak": 0}
    for r in rows: counts[r["verdict"]] = counts.get(r["verdict"], 0) + 1
    hdr = sfont.render(
        f"Forward-sim roster audit — {counts.get('oppressive',0)} oppressive · "
        f"{counts.get('fair',0)} fair · {counts.get('weak',0)} weak   "
        f"(sorted least-balanced first)", True, shade(INK, 1.4))
    screen.blit(hdr, (b.left + 60, b.top + 300))
    # column headers
    cy0 = b.top + 326
    for cx, lbl in ((0, "CARD"), (168, "INK"), (210, "VALUE/INK"),
                    (300, "SURVIVE"), (378, "VERDICT")):
        screen.blit(sf.render(lbl, True, shade(INK, 2.2)), (b.left + 60 + cx, cy0))
    # two columns of rows
    per_col = 15
    for i, r in enumerate(rows):
        col = i // per_col
        if col > 1: break
        row = i % per_col
        x = b.left + 60 + col * 430
        y = cy0 + 22 + row * 22
        vcol = _VERDICT_COL.get(r["verdict"], (150, 150, 150))
        pygame.draw.circle(screen, vcol, (x + 4, y + 7), 5)
        screen.blit(sf.render(r["name"][:20], True, INK), (x + 14, y))
        screen.blit(sf.render(str(r["cost"]), True, shade(INK, 1.8)), (x + 168, y))
        # value bar
        v = max(-20, min(60, r["value"]))
        bw = int((v + 20) / 80 * 70)
        pygame.draw.rect(screen, shade(INK, 3.4), (x + 210, y + 3, 70, 9), border_radius=3)
        pygame.draw.rect(screen, vcol, (x + 210, y + 3, max(2, bw), 9), border_radius=3)
        # survival %
        screen.blit(sf.render(f"{int(r['survival']*100)}%", True, shade(INK, 1.8)), (x + 300, y))
        screen.blit(sf.render(r["verdict"], True, vcol), (x + 378, y))


# ── Trophy rendering: a folded-paper award minted from a battle report ──────
_TROPHY_CACHE = {}
def render_trophy(spec, size=180):
    """Render a trophy spec (from trophy.py) as a paper sculpture: a folded
    silhouette in its material's palette, the ink-blot identicon on its face, gem
    accents, and a soft effect glow. Cached by the spec's stamp+size."""
    key = (spec.get("stamp", ""), size)
    if key in _TROPHY_CACHE:
        return _TROPHY_CACHE[key]
    surf = pygame.Surface((size, size), pygame.SRCALPHA)
    cx, cy = size // 2, size // 2
    pal = spec.get("palette", {"base": (200, 180, 120), "sheen": 0.3})
    base = tuple(pal["base"])
    rare = trophymod.rarity_color(spec.get("rarity", "silver")) if TROPHY_OK else GOLD
    shape = spec.get("shape", "cup")
    s = size * 0.32

    eff = spec.get("effect", "aura")
    if DIFFUSION_OK:
        try:
            glow = vfx_sprite({"fire": "ember", "water": "aura", "frost": "frost",
                               "smoke": "smoke", "ember": "ember", "aura": "aura"}.get(eff, "aura"),
                              size=int(size * 0.9), seed=abs(hash(spec.get("stamp", ""))) % 999)
            if glow.get_width():
                surf.blit(glow, (cx - glow.get_width() // 2, cy - glow.get_height() // 2),
                          special_flags=pygame.BLEND_ADD)
        except Exception:
            pass

    def paper(poly, tone=1.0):
        col = shade(base, tone)
        pygame.draw.polygon(surf, col, poly)
        pygame.draw.polygon(surf, shade(col, 0.7), poly, 2)

    if shape == "cup":
        paper([(cx - s, cy - s), (cx + s, cy - s), (cx + s*0.6, cy + s*0.3), (cx - s*0.6, cy + s*0.3)], 1.05)
        paper([(cx - s*0.4, cy + s*0.3), (cx + s*0.4, cy + s*0.3), (cx + s*0.3, cy + s*0.7), (cx - s*0.3, cy + s*0.7)], 0.9)
        paper([(cx - s*0.7, cy + s*0.7), (cx + s*0.7, cy + s*0.7), (cx + s*0.8, cy + s*0.9), (cx - s*0.8, cy + s*0.9)], 0.8)
        pygame.draw.arc(surf, shade(base, 0.8), (cx - s*1.5, cy - s, s*0.9, s*0.9), 1.2, 4.2, 4)
        pygame.draw.arc(surf, shade(base, 0.8), (cx + s*0.6, cy - s, s*0.9, s*0.9), -1.1, 1.9, 4)
    elif shape == "obelisk":
        paper([(cx, cy - s*1.2), (cx + s*0.5, cy + s*0.6), (cx - s*0.5, cy + s*0.6)], 1.05)
        paper([(cx - s*0.7, cy + s*0.6), (cx + s*0.7, cy + s*0.6), (cx + s*0.8, cy + s*0.9), (cx - s*0.8, cy + s*0.9)], 0.8)
    elif shape == "star":
        pts = []
        for i in range(10):
            ang = -math.pi/2 + i * math.pi/5
            rr = s*1.1 if i % 2 == 0 else s*0.5
            pts.append((cx + math.cos(ang)*rr, cy + math.sin(ang)*rr - s*0.2))
        paper(pts, 1.05)
        paper([(cx - s*0.6, cy + s*0.7), (cx + s*0.6, cy + s*0.7), (cx + s*0.7, cy + s*0.95), (cx - s*0.7, cy + s*0.95)], 0.8)
    elif shape == "shield":
        paper([(cx - s, cy - s), (cx + s, cy - s), (cx + s, cy + s*0.2), (cx, cy + s*1.1), (cx - s, cy + s*0.2)], 1.05)
    else:
        paper([(cx - s, cy + s*0.4), (cx - s, cy - s*0.4), (cx - s*0.5, cy + s*0.1),
               (cx, cy - s*0.7), (cx + s*0.5, cy + s*0.1), (cx + s, cy - s*0.4),
               (cx + s, cy + s*0.4)], 1.05)
        for gx in (-0.7, 0, 0.7):
            pygame.draw.circle(surf, rare, (int(cx + gx*s), int(cy - s*0.4)), max(3, int(s*0.12)))

    sig = spec.get("sigil", [])
    if sig:
        n = len(sig); cell = s * 1.1 / n
        ox, oy = cx - (n*cell)/2, cy - (n*cell)/2 + s*0.1
        blot = shade(base, 0.45)
        for r in range(n):
            for c in range(len(sig[r])):
                if sig[r][c]:
                    bx, by = ox + c*cell + cell/2, oy + r*cell + cell/2
                    pygame.draw.circle(surf, blot, (int(bx), int(by)), max(2, int(cell*0.42)))

    for i in range(spec.get("accents", 1)):
        ang = -math.pi/2 + (i - (spec.get("accents", 1)-1)/2) * 0.5
        gx, gy = cx + math.cos(ang)*s*1.05, cy - s*0.9 + math.sin(ang)*s*0.2
        pygame.draw.circle(surf, rare, (int(gx), int(gy)), max(3, int(s*0.13)))
        pygame.draw.circle(surf, PAGE, (int(gx), int(gy)), max(1, int(s*0.05)))

    pygame.draw.circle(surf, (*rare, 90), (cx, cy), int(size*0.46), 3)
    _TROPHY_CACHE[key] = surf
    return surf


# ── Profile / Replays ──────────────────────────────────────────────────────
REPLAY_LIST = None                 # cached list of saved replays
REPLAY_CASTER = None               # live auto-caster for the current replay
REPLAY_CASTER_FOR = None           # which player the caster is bound to
SHOW_CAST_STORY = True             # show the narration overlay during replays
try:
    import caster as castermod
    CASTER_OK = True
except Exception:
    castermod = None
    CASTER_OK = False


def _draw_story_panel(screen, sfont, entries):
    """Draw the last few auto-caster lines as a story ticker, bottom-left."""
    lines = entries[-5:]
    if not lines:
        return
    pad = 10
    wrapped = []
    for e in lines:
        txt = e["text"]
        # simple wrap at ~54 chars
        while len(txt) > 58:
            cut = txt.rfind(" ", 0, 58)
            if cut <= 0: cut = 58
            wrapped.append((txt[:cut], e["kind"]))
            txt = txt[cut + 1:]
        wrapped.append((txt, e["kind"]))
    wrapped = wrapped[-7:]
    lh = 18
    panel_h = lh * len(wrapped) + pad * 2
    panel_w = 430
    surf = pygame.Surface((panel_w, panel_h), pygame.SRCALPHA)
    surf.fill((18, 14, 12, 180))
    pygame.draw.rect(surf, (200, 168, 96, 160), surf.get_rect(), 1)
    kindcol = {"summon": (150, 200, 235), "engage": (235, 200, 150),
               "fall": (230, 150, 140), "open": (200, 200, 200),
               "close": (200, 180, 120)}
    for i, (line, kind) in enumerate(wrapped):
        col = kindcol.get(kind, (222, 214, 198))
        surf.blit(sfont.render(line, True, col), (pad, pad + i * lh))
    screen.blit(surf, (24, H - panel_h - 70))
REPLAY_PLAYER = None               # active ReplayPlayer while viewing
REPLAY_PLAYING = False             # auto-advance toggle
REPLAY_SPEED = 1                   # wall-clock playback multiplier (1x/2x/4x/8x)
REPLAY_ACC = 0.0                   # real-time accumulator (fixed-timestep sim)
REPLAY_LAST_T = None               # last wall-clock t seen, for frame delta

def _fmt_time(ts):
    try:
        return time.strftime("%b %d, %H:%M", time.localtime(ts))
    except Exception:
        return "?"

def draw_replays(screen, fonts, t, mouse):
    """Profile page: a list of saved replays to review, newest first."""
    global REPLAY_LIST
    font, bfont, sfont = fonts
    draw_desk_and_book(screen, t, "PROFILE / REPLAYS", "review your matches to improve")
    b = MENU_BOOK
    HOT.clear()
    if REPLAY_LIST is None and REPLAY_OK:
        try: REPLAY_LIST = replaymod.list_replays()
        except Exception: REPLAY_LIST = []
    lst = REPLAY_LIST or []
    if not lst:
        msg = font.render("No replays yet — play a match and it'll appear here.", True, INK)
        screen.blit(msg, (b.centerx - msg.get_width() / 2, b.top + 200))
    else:
        # summary line: win rate
        wins = sum(1 for r in lst if r.get("result") == "win")
        total_kb = sum(r.get("size", 0) for r in lst) / 1024.0
        summ = sfont.render(f"{len(lst)} replays · {wins} wins · {wins*100//max(1,len(lst))}% "
                            f"win rate · {total_kb:.1f} KB total", True, shade(INK, 1.4))
        screen.blit(summ, (b.left + 60, b.top + 130))
        rows = HOT.setdefault("replay_rows", [])
        rows.clear()
        for i, r in enumerate(lst[:14]):
            y = b.top + 165 + i * 34
            rr = pygame.Rect(b.left + 60, y, b.w - 120, 30)
            hover = rr.collidepoint(mouse)
            pygame.draw.rect(screen, shade(PAGE, 0.94 if hover else 1.0), rr, border_radius=6)
            pygame.draw.rect(screen, shade(INK, 3.0), rr, 1, border_radius=6)
            m = r.get("meta", {})
            res = r.get("result", "?")
            rescol = (110, 180, 120) if res == "win" else (206, 92, 78) if res == "lose" else INK
            pygame.draw.circle(screen, rescol, (rr.x + 16, rr.centery), 6)
            secs = int(r.get("ticks", 0) / 20)
            label = (f"{_fmt_time(m.get('started', 0))}   ·   {m.get('mode','match')}"
                     f"   ·   {m.get('arena','?')}   ·   {secs//60}:{secs%60:02d}")
            screen.blit(sfont.render(label, True, INK), (rr.x + 32, rr.y + 6))
            wl = sfont.render(res.upper(), True, rescol)
            screen.blit(wl, (rr.right - wl.get_width() - 14, rr.y + 6))
            # version badge: does this replay's patch set match the current build?
            compat = r.get("compat", "unknown")
            cc = {"exact": (110, 180, 120), "same-base": (210, 170, 90),
                  "incompatible": (206, 92, 78)}.get(compat, (150, 150, 150))
            ct = {"exact": "✓ match", "same-base": "~ patch", "incompatible": "✗ ver",
                  "unknown": "?"}.get(compat, "?")
            cb = _font(11).render(ct, True, cc)
            screen.blit(cb, (rr.right - wl.get_width() - 30 - cb.get_width(), rr.y + 8))
            rows.append((rr, r["path"]))
    button(screen, pygame.Rect(b.right - 190, b.bottom - 66, 150, 48), "Back", font, mouse, key="back")
    menu_vignette(screen)

def draw_replay_view(screen, fonts, t, mouse, g_container):
    """Scrub viewer: renders the replayed battle with a timeline scrubber and
    play/pause + speed controls so a player can study any moment."""
    global REPLAY_PLAYER, REPLAY_PLAYING, REPLAY_SPEED, REPLAY_ACC, REPLAY_LAST_T
    font, bfont, sfont = fonts
    HOT.clear()
    player = REPLAY_PLAYER
    if player is None:
        REPLAY_LAST_T = None
        return
    # advance on a real-time accumulator so playback speed is independent of how
    # heavy the current frame is to render. Without this, busy moments (many
    # units/particles) render slowly and the replay drags, while quiet moments
    # race ahead — the sim would be paced by frame rate, not wall-clock time.
    if REPLAY_LAST_T is None:
        REPLAY_LAST_T = t
    frame_dt = t - REPLAY_LAST_T
    REPLAY_LAST_T = t
    # ignore absurd gaps (first frame after load, tab switch, breakpoints)
    if frame_dt < 0 or frame_dt > 0.5:
        frame_dt = 0.0
    if REPLAY_PLAYING and player.cur < player.total:
        # REPLAY_SPEED is a wall-clock multiplier (1x/2x/4x/8x). Ticks advance at
        # the same TPS the match was recorded at, paced by GAME_SPEED, so 1x here
        # equals the original match speed exactly.
        REPLAY_ACC = min(REPLAY_ACC + frame_dt * GAME_SPEED * REPLAY_SPEED, 0.5)
        steps = 0
        while REPLAY_ACC >= DT and player.cur < player.total and steps < 240:
            player.step(1); REPLAY_ACC -= DT; steps += 1
        if player.cur >= player.total:
            REPLAY_PLAYING = False
    else:
        REPLAY_ACC = 0.0
    # auto-caster: narrate the match as it plays. The caster only READS the game,
    # so it never disturbs playback. Rebuilt when a new player is attached.
    global REPLAY_CASTER, REPLAY_CASTER_FOR
    if CASTER_OK and getattr(player, "g", None) is not None:
        if REPLAY_CASTER_FOR is not player:
            try:
                REPLAY_CASTER = castermod.Caster(
                    seed=getattr(player.g, "match_seed", 1),
                    name_of=lambda k: CARDS.get(k, {}).get("name", str(k).title()))
            except Exception:
                REPLAY_CASTER = None
            REPLAY_CASTER_FOR = player
        if REPLAY_CASTER is not None:
            try: REPLAY_CASTER.observe(player.g)
            except Exception: pass
    # render the battle frame from the replayed game
    render(screen, player.g, font, bfont, sfont)
    # story panel: the last few narration lines, bottom-left, if enabled
    if SHOW_CAST_STORY and REPLAY_CASTER is not None and REPLAY_CASTER.entries:
        _draw_story_panel(screen, sfont, REPLAY_CASTER.entries)
    # version banner: if this replay was recorded on a different patch set, warn
    # (an incompatible replay may not reproduce faithfully on this build)
    compat = getattr(player, "compat", "exact")
    if compat and compat != "exact":
        import patchtree as _pt
        msg = "Recorded on a " + _pt.describe(compat)
        col = (210, 170, 90) if compat == "same-base" else (206, 92, 78)
        band = sfont.render(msg, True, PAGE)
        bw = band.get_width() + 28
        br = pygame.Rect(W // 2 - bw // 2, 8, bw, 26)
        pygame.draw.rect(screen, col, br, border_radius=8)
        screen.blit(band, (br.centerx - band.get_width() // 2, br.y + 5))
    # ── scrub bar ──
    bar = pygame.Rect(160, H - 54, W - 320, 10)
    pygame.draw.rect(screen, shade(PAGE, 0.5), bar, border_radius=5)
    prog = player.progress
    pygame.draw.rect(screen, GOLD, (bar.x, bar.y, int(bar.w * prog), bar.h), border_radius=5)
    knob = (bar.x + int(bar.w * prog), bar.centery)
    pygame.draw.circle(screen, PAGE, knob, 9); pygame.draw.circle(screen, INK, knob, 9, 2)
    HOT["scrub"] = bar
    secs = player.cur // 20; total_s = player.total // 20
    tlabel = sfont.render(f"{secs//60}:{secs%60:02d} / {total_s//60}:{total_s%60:02d}", True, PAGE)
    screen.blit(tlabel, (bar.x, bar.y - 22))
    # play/pause + speed + exit
    pp = pygame.Rect(bar.x - 120, H - 66, 96, 34)
    button(screen, pp, "Pause" if REPLAY_PLAYING else "Play", sfont, mouse, key="playpause")
    sp = pygame.Rect(bar.right + 20, H - 66, 70, 34)
    button(screen, sp, f"{REPLAY_SPEED}x", sfont, mouse, key="speed")
    ex = pygame.Rect(W - 150, H - 66, 110, 34)
    button(screen, ex, "Exit", sfont, mouse, key="rexit")

def menu_vignette(screen):
    vg = pygame.Surface((W, H), pygame.SRCALPHA)
    for i in range(10):
        a = 12 + i * 8
        pygame.draw.rect(vg, (4, 2, 6, a), (i * 3, i * 3, W - i * 6, H - i * 6), width=6, border_radius=30)
    screen.blit(vg, (0, 0))

_BATTLE_VIGNETTE = {"key": None, "surf": None}
def _draw_trickster_dark(screen, g, font, bfont, sfont):
    """The blackout overlay: the room goes dark, the Trickster lurks (or doesn't),
    and a flint-&-steel button lets you gamble on relighting. The only tell is
    subtle — read it and pick your moment."""
    tnow = pygame.time.get_ticks() * 0.001
    # heavy darkness that deepens the longer you sit in it
    murk = min(232, 150 + int(getattr(g, "trick_dark_t", 0) * 12))
    veil = pygame.Surface((W, H), pygame.SRCALPHA)
    veil.fill((6, 4, 12, murk))
    screen.blit(veil, (0, 0))
    # PRESENCE TELL: when he's present, two faint eye-glimmers hang over the board
    # and a violet unease creeps in; when absent, the dark is calmer/cooler. This
    # is the cue the player waits for.
    present = getattr(g, "trick_present", False)
    if present:
        pulse = 0.5 + 0.5 * math.sin(tnow * 6)
        ex, ey = BOOK.centerx, BOOK.top + 130
        for dx in (-26, 26):
            glow = pygame.Surface((60, 60), pygame.SRCALPHA)
            pygame.draw.circle(glow, (200, 70, 90, int(120 * pulse)), (30, 30), 14)
            screen.blit(glow, (ex + dx - 30, ey - 30), special_flags=pygame.BLEND_ADD)
            pygame.draw.circle(screen, (240, 120, 130), (ex + dx, ey), max(2, int(4 + pulse * 3)))
        tell = sfont.render("something is here…", True, (180, 90, 110))
        screen.blit(tell, (BOOK.centerx - tell.get_width() / 2, ey + 26))
    else:
        # a cooler, quieter dark — the safe window
        tell = sfont.render("the room feels empty…", True, (110, 120, 160))
        screen.blit(tell, (BOOK.centerx - tell.get_width() / 2, BOOK.top + 140))
    # a laugh flash
    if getattr(g, "trick_laughed", 0) > 0:
        la = sfont.render("heheheh…", True, (170, 120, 210))
        jx = math.sin(tnow * 30) * 4
        screen.blit(la, (BOOK.centerx - la.get_width() / 2 + jx, BOOK.top + 60))
    # ── flint & steel relight button (bottom-centre, above the shadowed hand) ──
    fb = pygame.Rect(W // 2 - 120, HAND_Y - 96, 240, 46)
    hov = fb.collidepoint(pygame.mouse.get_pos())
    pygame.draw.rect(screen, (52, 40, 30) if not hov else (72, 56, 40), fb, border_radius=10)
    pygame.draw.rect(screen, shade(WAX, 1.2), fb, 2, border_radius=10)
    # a couple of spark flecks off the flint
    for k in range(3):
        sx = fb.left + 30 + (math.sin(tnow * 9 + k) * 6)
        sy = fb.centery + math.cos(tnow * 12 + k) * 5
        pygame.draw.circle(screen, (255, 200, 120), (int(sx), int(sy)), 1)
    lab = sfont.render("✦ strike flint & steel — relight", True, shade(PAGE, 0.95))
    screen.blit(lab, (fb.centerx - lab.get_width() / 2, fb.centery - lab.get_height() / 2))
    HOT["relight"] = fb
    hint = sfont.render("wait for the room to feel empty, then relight — "
                        "catch him here and he screeches.", True, shade(PAGE, 0.6))
    screen.blit(hint, (W // 2 - hint.get_width() / 2, fb.bottom + 6))


def battle_vignette(screen):
    """A soft radial darkening at the screen edges so the desk fades into shadow
    top/bottom/side — makes the table read as filling the whole screen instead of
    ending in a hard letterbox edge. Cached by (W,H); works in every gfx preset."""
    key = (W, H)
    if _BATTLE_VIGNETTE["key"] != key:
        vg = pygame.Surface((W, H), pygame.SRCALPHA)
        cx, cy = W / 2, H * 0.46
        maxd = math.hypot(cx, cy) or 1
        # build at low res then upscale — cheap and smooth
        sw, sh = max(2, W // 8), max(2, H // 8)
        small = pygame.Surface((sw, sh), pygame.SRCALPHA)
        for yy in range(sh):
            for xx in range(sw):
                fx, fy = xx / sw * W, yy / sh * H
                d = math.hypot(fx - cx, fy - cy) / maxd
                a = int(max(0, min(150, (d - 0.55) * 320)))   # transparent centre → dark edge
                if a:
                    small.set_at((xx, yy), (6, 3, 8, a))
        vg = pygame.transform.smoothscale(small, (W, H))
        _BATTLE_VIGNETTE["surf"] = vg
        _BATTLE_VIGNETTE["key"] = key
    screen.blit(_BATTLE_VIGNETTE["surf"], (0, 0))

def _hand_slot_at(mx, my, which="bottom"):
    """Return the hand slot index (0..3) under a point, or None. `which` selects
    the bottom (main) strip or the top (player-2 / seat-B) strip layout.

    Coordinates come from win_to_canvas and can be fractional (scaled display),
    so we round before the modular slot math — a sub-pixel x sitting a hair inside
    a slot's left gap used to floor into the previous slot on occasion."""
    mx = round(mx); my = round(my)
    if which == "bottom":
        if my <= HAND_Y - 45:
            return None
        cw, gp = 108, 14
        x0 = W / 2 - (4 * cw + 3 * gp) / 2
        rel = mx - x0
        idx = int(rel // (cw + gp))
        if 0 <= idx < 4 and rel - idx * (cw + gp) <= cw:
            return idx
    else:  # top strip (compact cards along the top edge)
        if my >= 120:
            return None
        cw2, gp2 = 96, 12
        x02 = W / 2 - (4 * cw2 + 3 * gp2) / 2
        rel = mx - x02
        idx = int(rel // (cw2 + gp2))
        if 0 <= idx < 4 and rel - idx * (cw2 + gp2) <= cw2:
            return idx
    return None

def _drag_who_for(g, which):
    """Resolve a drag side ("bottom"/"top") to the player/seat token used by the
    deploy helpers for the current game mode, or None if that side has no player."""
    if getattr(g, "dual_local", False) and g.seat_ctrl:
        seats = list(g.seat_ctrl.keys())
        if which == "bottom":
            return ("seat", seats[0])
        elif len(seats) > 1:
            return ("seat", seats[1])
        return None
    if getattr(g, "local_1v1", False):
        return which          # "bottom"→P1, "top"→P2
    if which == "bottom":
        return "bottom"       # single-player / net own side
    return None

def _drag_hand_key(g, who, idx):
    if isinstance(who, tuple) and who[0] == "seat":
        ctrl = g.seat_ctrl.get(who[1])
        return ctrl["rot"].hand[idx] if ctrl and idx < len(ctrl["rot"].hand) else None
    if who == "top" and getattr(g, "local_1v1", False):
        return g.hand2[idx] if g.hand2 else None
    return g.hand[idx]

def _drag_can_afford(g, who, idx):
    key = _drag_hand_key(g, who, idx)
    if key is None:
        return False
    cost = CARDS[key]["cost"]
    if isinstance(who, tuple) and who[0] == "seat":
        return g.seat_ctrl[who[1]]["ink"] >= cost
    if who == "top" and getattr(g, "local_1v1", False):
        return g.ai_ink2 >= cost
    return g.ink >= cost

def _drag_deploy(g, who, idx, lx, ly):
    """Deploy the dragged card at board point (lx, ly), routing to the correct
    per-mode play method. On insufficient ink the card bounces and the selection
    is kept. Returns the play status ('played' | 'noink' | None)."""
    if isinstance(who, tuple) and who[0] == "seat":
        res = g.net_seat_play(who[1], idx, (lx, ly))
        if res != "noink":
            g.seat_ctrl[who[1]]["selected"] = None
        return res
    if who == "top" and getattr(g, "local_1v1", False):
        res = g.try_play2(idx, (lx, ly))
        if res != "noink":
            g.selected2 = None
        return res
    if g.netmode:
        res = g.net_local_play(idx, (lx, ly))
    else:
        res = g.try_play(idx, (lx, ly))
    if res != "noink":
        g.selected = None
    return res

def draw_drag_ghost(screen, g, drag, tnow):
    """Render the dragged card following the pointer, plus a placement preview
    (a translucent unit + a valid/invalid drop halo) on the board."""
    if not drag:
        return
    px, py = drag["pos"]
    key = drag["key"]
    who = drag["who"]
    idx = drag["idx"]
    lx, ly = unproj(px, py)
    on_board = BOOK.collidepoint(lx, ly)
    team = 0
    if isinstance(who, tuple) and who[0] == "seat":
        team = seat_team_local(who[1])
    elif who == "top":
        team = 1
    afford = _drag_can_afford(g, who, idx)
    if on_board:
        gy = ly
        if team == 0 and gy < RIVER_Y + RIVER_H: gy = RIVER_Y + RIVER_H + 10
        if team == 1 and gy > RIVER_Y - RIVER_H: gy = RIVER_Y - RIVER_H - 10
        halo = (90, 200, 120) if afford else (200, 80, 70)
        sxp, syp, scp = proj(lx, gy)
        ring = pygame.Surface((W, H), pygame.SRCALPHA)
        pygame.draw.circle(ring, (*halo, 80), (int(sxp), int(syp)), int(34 * scp))
        pygame.draw.circle(ring, (*halo, 150), (int(sxp), int(syp)), int(34 * scp), 3)
        screen.blit(ring, (0, 0))
        try:
            gd = Unit(key, 0, 0, team); gd.spawn_t = 0
            gd.x, gd.y = lx, gy
            gsh = pygame.Surface((W, H), pygame.SRCALPHA)
            ghost = pygame.Surface((W, H), pygame.SRCALPHA)
            origami_animal(ghost, gsh, gd)
            # NOTE: do NOT use ghost.set_alpha() here — a per-surface alpha on a
            # per-pixel-alpha (SRCALPHA) surface makes the transparent background
            # composite as BLACK on some SDL/mobile backends (the "black box under
            # the dragged card" bug). Instead scale the per-pixel alpha directly.
            ghost.fill((255, 255, 255, 130), special_flags=pygame.BLEND_RGBA_MULT)
            screen.blit(ghost, (0, 0))
        except Exception:
            pass
    # the card itself trailing the pointer
    cw, chh = 84, 100
    cr = pygame.Rect(int(px - cw / 2), int(py - chh - 8), cw, chh)
    sfl = _font(15)
    ffl = _font(18, bold=True)
    shadow = pygame.Surface((W, H), pygame.SRCALPHA)
    pygame.draw.ellipse(shadow, (0, 0, 0, 80), (px - 30, py - 6, 60, 16))
    screen.blit(shadow, (0, 0))
    draw_card(screen, key, cr, ffl, sfl, afford)

def _hit_any_hot(m):
    """True if a point lands on any registered clickable UI rect this frame —
    used to decide whether a menu press should emit a UI click sound."""
    for v in HOT.values():
        if isinstance(v, pygame.Rect):
            if v.collidepoint(m):
                return True
        elif isinstance(v, (list, tuple)):
            for item in v:
                r = item[0] if isinstance(item, (list, tuple)) and item else item
                if isinstance(r, pygame.Rect) and r.collidepoint(m):
                    return True
    return False

def _boss_state(g):
    """A JSON-able read-out of the battle from the boss's point of view, rich
    enough for it to counter the player's composition (units + lanes + phase)."""
    try:
        lane_x = (BOOK.left + BOOK.w * 0.33, BOOK.left + BOOK.w * 0.66)
        def lane_of(x):
            return 0 if x < lane_x[0] else (1 if x < lane_x[1] else 2)
        player_units = []
        for u in g.units:
            if u.alive and u.team == 0:
                player_units.append({
                    "key": u.key, "hp": round(u.hp), "dmg": round(getattr(u, "dmg", 0)),
                    "flying": bool(getattr(u, "flying", False)),
                    "building": bool(getattr(u, "building", False)),
                    "lane": lane_of(u.x), "size": getattr(u, "size", 20)})
        e_king = next((t for t in g.towers if t.team == 1 and t.king), None)
        return {
            "phase": getattr(g, "boss_phase", 1),
            "elapsed": round(getattr(g, "time", 0), 1),
            "arena": CUR_ARENA, "weather": WEATHER, "hazard": HAZARD,
            "player_units": player_units[:24],
            "player_unit_count": len(player_units),
            "boss_king_hp": round(e_king.hp / e_king.maxhp, 2) if e_king else 1.0,
            "card_ids": list(CARDS.keys())[:80],
        }
    except Exception:
        return {"phase": 1, "player_units": []}

def _spawn_boss_entourage(g, mind):
    """A boss variant opens the fight with a small escort of themed minions,
    placed by its placement style. Deterministic (seeded from the match)."""
    var = getattr(g, "boss_variant", None)
    if not var or not AIBOSS_OK:
        return
    taken = tuple(CARDS.keys())
    n = getattr(var, "escort", 2)
    prng = random.Random((g.match_seed ^ 0xe5c0a7) & 0xffffffff)
    dense = _boss_densest_lane(g)
    for i in range(n):
        try:
            card = mind._minion(taken)            # already entourage-biased + tinted
            key = register_card(card)
            if not key:
                continue
        except Exception:
            continue
        gx = ai_boss.variant_lane_x(var.placement, dense, BOOK.left, BOOK.w, prng)
        gx = max(BOOK.left + 40, min(BOOK.right - 40, gx))
        if var.placement == "front":
            gy = RIVER_Y - RIVER_H - prng.randint(30, 80)
        elif var.placement == "back":
            gy = BOOK.top + prng.randint(60, 100)
        else:
            gy = BOOK.top + prng.randint(90, 150)
        g.deploy(key, gx, gy, 1)
    g.boss_notes.append([f"The {var.title} {mind.persona.name} — {var.blurb}.", 5.5])


def apply_boss_action(g, act):
    """Apply one BossMind action to the live battle from the enemy side (team 1).
    Returns an in-voice announcement string (or None). All effects reuse existing,
    validated engine mechanics, so a bad payload can't corrupt the sim."""
    if not isinstance(act, dict):
        return None
    kind = act.get("kind")
    text = act.get("text")
    lane_x = (BOOK.left + BOOK.w * 0.2, BOOK.left + BOOK.w * 0.5, BOOK.left + BOOK.w * 0.8)

    if kind == "summon":
        key = register_card(act.get("card"))
        if not key:
            return None
        # placement style (from a boss variant) decides WHERE it drops; else the
        # forward-sim's chosen lane; else board centre.
        placement = act.get("placement")
        lane = act.get("lane")
        prng = random.Random((g.match_seed ^ (g.tick * 2246822519)) & 0xffffffff)
        if placement and AIBOSS_OK:
            dense = _boss_densest_lane(g)
            gx = ai_boss.variant_lane_x(placement, dense, BOOK.left, BOOK.w, prng)
        elif isinstance(lane, int) and 0 <= lane <= 2:
            gx = lane_x[lane] + prng.randint(-40, 40)
        else:
            gx = BOOK.centerx + prng.randint(-160, 160)
        gx = max(BOOK.left + 40, min(BOOK.right - 40, gx))
        # front-line variants drop near the river; backline ones hug the boss king
        if placement == "front":
            gy = RIVER_Y - RIVER_H - prng.randint(30, 70)
        elif placement == "back":
            gy = BOOK.top + prng.randint(70, 110)
        else:
            gy = BOOK.top + prng.randint(90, 150)
        g.deploy(key, gx, gy, 1)
        sfx("deploy", 0.7)
        return text or f"{CARDS[key]['name']} unfolds."

    if kind == "counter":
        c = act.get("counter")
        foes = [u for u in g.units if u.team == 0 and u.alive and not u.building]
        if c == "freeze_biggest" and foes:
            tgt = max(foes, key=lambda u: u.hp)
            tgt.stunned = max(getattr(tgt, "stunned", 0), 2.2)
            g.boss_fx.append(dict(kind="tick", x=tgt.x, y=tgt.y, t=0.0))
            g.add_shockwave(tgt.x, tgt.y, mag=16, dur=0.6); sfx("freeze")
            return text or "Halted mid-stroke."
        if c == "blot_densest_lane":
            # ink puddles across the player's busiest third
            lane = _boss_densest_lane(g)
            cx = lane_x[lane]
            for _ in range(3 + g.boss_phase):
                px = cx + random.uniform(-70, 70)
                py = random.uniform(RIVER_Y + 40, BOOK.bottom - 60)
                g.ink_puddles.append(dict(x=px, y=py, timer=4.0, team=1))
                g.boss_fx.append(dict(kind="blot", x=px, y=py, t=0.0))
            sfx("inksplash") if _sfx_has("inksplash") else sfx("error")
            return text or "A blot spreads across your lane."
        if c == "knockback_front" and foes:
            for u in sorted(foes, key=lambda u: u.y)[:5]:
                u.y = min(BOOK.bottom - 20, u.y + 90)   # shove back toward player
                u.stunned = max(getattr(u, "stunned", 0), 0.5)
            g.add_shockwave(BOOK.centerx, RIVER_Y + 60, mag=18, dur=0.6); sfx("whoosh")
            return text or "Back to the margin with you."
        if c == "summon_hardcounter":
            return apply_boss_action(g, {"kind": "summon", "card": None,
                                         "text": text}) or (text or "A counter takes shape.")
        if c == "swarm_flanks":
            for bx in BRIDGE_XS:
                for _ in range(2 + g.boss_phase):
                    g.deploy("frog", bx + random.randint(-40, 40), BOOK.top + 120, 1)
            sfx("deploy", 0.6)
            return text or "The flanks crawl with us."
        return text or "…"

    if kind == "zone":
        lane = act.get("lane", 1)
        lane = lane if lane in (0, 1, 2) else 1
        cx = lane_x[lane]
        for _ in range(3 + g.boss_phase):
            px = cx + random.uniform(-70, 70)
            py = random.uniform(RIVER_Y + 40, BOOK.bottom - 60)
            g.ink_puddles.append(dict(x=px, y=py, timer=4.5, team=1))
            g.boss_fx.append(dict(kind="blot", x=px, y=py, t=0.0))
        g.add_shockwave(cx, RIVER_Y + 80, mag=14, dur=0.6)
        return text or "This lane is closed to you."

    if kind == "mirror":
        # copy the player's strongest (or targeted) card back onto the boss side
        tgt_key = act.get("target")
        if tgt_key not in CARDS:
            foes = [u for u in g.units if u.team == 0 and u.alive]
            tgt_key = max(foes, key=lambda u: u.hp).key if foes else None
        if tgt_key and tgt_key in CARDS:
            g.deploy(tgt_key, BOOK.centerx + random.randint(-100, 100),
                     BOOK.top + 130, 1)
            sfx("deploy", 0.7)
            return text or f"Meet your {CARDS[tgt_key]['name']}, folded backward."
        return text or "Nothing worth copying… yet."

    if kind == "buff":
        try:
            mult = float(act.get("value", 1.3))
        except Exception:
            mult = 1.3
        for u in g.units:
            if u.team == 1 and u.alive:
                u.dmg = getattr(u, "dmg", 0) * mult
        g.boss_fx.append(dict(kind="tick", x=BOOK.centerx, y=BOOK.top + 120, t=0.0))
        return text or "The ink thickens in my favour."

    if kind == "taunt":
        return text or "…"
    return None

def _boss_densest_lane(g):
    lane_x = (BOOK.left + BOOK.w * 0.33, BOOK.left + BOOK.w * 0.66)
    lanes = [0, 0, 0]
    for u in g.units:
        if u.team == 0 and u.alive:
            lanes[0 if u.x < lane_x[0] else (1 if u.x < lane_x[1] else 2)] += 1
    return max(range(3), key=lambda i: lanes[i])

def _sfx_has(name):
    return SOUND_OK and sound is not None and name in getattr(sound, "SFX", ())

def _director_state(g):
    """A small JSON-able snapshot of the battle for the AI Director to reason on."""
    try:
        allies = sum(1 for u in g.units if u.alive and u.team == 0)
        foes = sum(1 for u in g.units if u.alive and u.team == 1)
        king0 = next((t for t in g.towers if t.team == 0 and t.king), None)
        king1 = next((t for t in g.towers if t.team == 1 and t.king), None)
        return {
            "elapsed": round(getattr(g, "time", 0), 1),
            "arena": CUR_ARENA, "weather": WEATHER, "hazard": HAZARD,
            "units_team0": allies, "units_team1": foes,
            "king0_hp": round(king0.hp / king0.maxhp, 2) if king0 else 1.0,
            "king1_hp": round(king1.hp / king1.maxhp, 2) if king1 else 1.0,
            "card_ids": list(CARDS.keys())[:80],
            "ink": round(getattr(g, "ink", 0), 1),
        }
    except Exception:
        return {"elapsed": 0}

def apply_director_event(g, ev, notes):
    """Apply one Director event to the live battle. Returns an announcement str."""
    if not isinstance(ev, dict):
        return None
    kind = ev.get("kind")
    text = ev.get("text")
    if kind == "card":
        key = register_card(ev.get("card"))
        if not key:
            return None
        # drop the fresh figure onto the page for a team (or a neutral flourish)
        team = ev.get("team")
        if team in (0, 1):
            side = 1 if team == 0 else -1
            gx = BOOK.centerx + random.randint(-120, 120)
            gy = RIVER_Y + side * random.randint(120, 220)
            g.deploy(key, gx, gy, team)
        sfx("coin")
        return text or f"A new figure inks itself: {CARDS[key]['name']}"
    if kind == "map":
        akey = register_arena(ev.get("map"))
        if not akey:
            return None
        set_arena(akey); globals()["BOARD_STATIC"] = None
        set_ambient(akey)
        sfx("whoosh")
        return text or f"The page turns: {ARENAS[akey]['name']}"
    if kind == "twist":
        tw = ev.get("twist"); val = ev.get("value")
        if tw == "weather":
            globals()["WEATHER"] = val; sfx("thunder", 0.5)
            return text or f"The weather turns: {val}"
        if tw == "hazard":
            globals()["HAZARD"] = val; globals()["BOARD_STATIC"] = None
            return text or f"A hazard spreads: {val}"
        if tw == "gift_ink":
            try:
                _c = ink_cap(getattr(g, "mode", MODE))
                g.ink = min(_c, g.ink + float(val)); g.ai_ink = min(_c, g.ai_ink + float(val))
            except Exception:
                pass
            sfx("coin")
            return text or "The inkwell brims — bonus ink."
        if tw == "frenzy":
            g.frenzy = True
            return text or "⚠ The page unravels ⚠"
    return None

def _kbd_select(g, side, idx):
    """Select hand slot `idx` (0..3) for the player on the given `side`
    ("bottom" or "top"), mapping to whatever player structure this mode uses.
    Only selects a card here; the actual deploy still happens on a board click/
    tap, consistent with the pointer flow. Each player's selection is kept
    independent so two people can have cards queued at the same time."""
    if idx < 0 or idx > 3:
        return
    sfx("select")
    # 2v2 dual-local: two seats on this device. seat A (first) is the bottom
    # strip, seat B (second) the top strip.
    if getattr(g, "dual_local", False) and g.seat_ctrl:
        seats = list(g.seat_ctrl.keys())
        if side == "bottom":
            g.seat_ctrl[seats[0]]["selected"] = idx
        elif len(seats) > 1:
            g.seat_ctrl[seats[1]]["selected"] = idx
        return
    # hotseat 1v1: player 1 bottom (selected), player 2 top (selected2)
    if getattr(g, "local_1v1", False):
        if side == "bottom":
            g.selected = idx
        else:
            g.selected2 = idx
        return
    # single-player or networked own-side play: only the bottom bank applies
    # (a spectator has no hand). "top" keys are ignored so a stray press does
    # nothing rather than selecting the opponent's card.
    if getattr(g, "netmode", None) == "spectator":
        return
    if side == "bottom":
        g.selected = idx

def main():
    pygame.init()
    # gamepads: open every connected stick so JOY* events flow (the Foldworks
    # RTS has Halo Wars-style console controls; harmless without a pad)
    try:
        pygame.joystick.init()
        pads = [pygame.joystick.Joystick(i)
                for i in range(pygame.joystick.get_count())]
        for p in pads:
            p.init()
    except Exception:
        pass
    if not _FONT_OK:
        # fonts couldn't initialise (see the guidance printed at import). Don't
        # limp into a cryptic mid-render crash — exit cleanly with the fix shown.
        print("Paperbound: aborting — pygame font module unavailable "
              "(run `pip install -U pygame-ce`).", file=sys.stderr)
        return
    pygame.display.set_caption("Paperbound")
    font = _font(18, bold=True)
    bfont = _font(24, bold=True)
    sfont = _font(15)
    fonts = (font, bfont, sfont)
    set_arena("study")

    # seasonal nudge: if a holiday event is active, offer its themed mode as the
    # default (the player can still switch to anything). Purely a menu default.
    if ACTIVE_EVENT and ACTIVE_EVENT.get("default_mode") in MODES:
        globals()["MODE"] = ACTIVE_EVENT["default_mode"]
        try:
            print(f"[event] {ACTIVE_EVENT['name']} {ACTIVE_EVENT.get('emoji','')} — "
                  f"themed mode: {MODES[MODE]['name']}")
        except Exception:
            pass
    # register the active event's card skin (if any) so the deck builder can offer
    # swapping cards to/from their seasonal variant
    globals()["SEASONAL_VARIANT_ID"] = _register_seasonal_variant()
    if SEASONAL_VARIANT_ID:
        try:
            print(f"[event] seasonal card skin available: {SEASONAL_VARIANT_ID}")
        except Exception:
            pass

    if HEADLESS:
        # headless screenshot mode needs a display surface but no interactivity
        screen = pygame.display.set_mode((W, H))
        _headless_shots(screen, fonts); return

    # ── audio: bring up the procedural sound engine + restore saved volume ──
    global VOLUME, MUTED
    if SOUND_OK:
        if COLLECTION_OK:
            try:
                v = PBCOLLECTION.get_pref("volume")
                if isinstance(v, (int, float)): VOLUME = max(0.0, min(1.0, float(v)))
                m = PBCOLLECTION.get_pref("muted")
                if isinstance(m, bool): MUTED = m
            except Exception:
                pass
        sound.init()
        sound.set_master(VOLUME)
        sound.set_muted(MUTED)
    # restore accessibility / display preferences
    if COLLECTION_OK:
        try:
            sb = PBCOLLECTION.get_pref("sigil_badges")
            if isinstance(sb, bool): globals()["SIGIL_BADGES"] = sb
            bs = PBCOLLECTION.get_pref("battery_saver")
            if isinstance(bs, bool): globals()["BATTERY_SAVER"] = bs
            ps = PBCOLLECTION.get_pref("piece_scale_i")
            if isinstance(ps, int) and 0 <= ps < len(PIECE_SCALE_STEPS):
                globals()["PIECE_SCALE_I"] = ps
                globals()["BIG_UNITS"] = ps != 0
        except Exception:
            pass
    set_ambient("study")            # menu ambience (the binder's study)

    clock = pygame.time.Clock()
    # A plain RESIZABLE window (NOT pygame.SCALED). SCALED keeps a fixed logical
    # surface and re-emits VIDEORESIZE when we touch it, which on some Linux/
    # Wayland mobile compositors ping-pongs into an endless resize loop. With
    # plain RESIZABLE the surface simply tracks the window and we reflow the UI
    # ourselves from the live size each frame.
    try:
        window = pygame.display.set_mode((W, H), pygame.RESIZABLE)
    except pygame.error:
        window = pygame.display.set_mode((W, H))
    relayout(*window.get_size())        # reflow to the real window aspect
    set_arena(CUR_ARENA)                # rebuild board geometry for this layout
    canvas = pygame.Surface((W, H))
    _canvas_size = (W, H)
    _win_size_seen = window.get_size()
    acc = 0.0; running = True; t = 0.0
    # focus/visibility event types that exist in this pygame build (SDL2 names
    # vary; we collect whatever's present so backgrounding works everywhere).
    _BG_EVENTS = set()
    _FG_EVENTS = set()
    for nm in ("APP_DIDENTERBACKGROUND", "APP_WILLENTERBACKGROUND",
               "WINDOWFOCUSLOST", "WINDOWMINIMIZED", "WINDOWHIDDEN"):
        if hasattr(pygame, nm): _BG_EVENTS.add(getattr(pygame, nm))
    for nm in ("APP_DIDENTERFOREGROUND", "APP_WILLENTERFOREGROUND",
               "WINDOWFOCUSGAINED", "WINDOWRESTORED", "WINDOWSHOWN", "WINDOWEXPOSED"):
        if hasattr(pygame, nm): _FG_EVENTS.add(getattr(pygame, nm))
    scene = "menu"; sel_arena = "study"; g = None
    flip = None                # page-flip transition {img, t, to}
    forest_ev = None           # active forest-path choice event
    # active card drag (mouse / touch / controller). None when not dragging.
    #   {"who": "bottom"|"top"|("seat", seat_id), "idx": int, "key": str,
    #    "pos": (canvas_x, canvas_y), "src": "mouse"|"touch"|"pad"}
    drag = None
    dragging_vol = False        # true while the options volume knob is grabbed
    _cur_ambient = "study"      # currently playing ambient key (audio)
    _last_result = None         # last battle result seen (for victory/defeat sfx)
    # ── AI Director (Living Page mode) ──
    director = None             # ai_director.Director while a living battle runs
    director_cfg = ai_director.DirectorConfig.from_env() if AIDIR_OK else None
    director_notes = []         # transient on-screen announcements [(text, t_left)]
    # ── AI Boss (Arch-Scribe encounters) ──
    boss_mind = None           # ai_boss.BossMind while an AI-boss battle runs
    boss_fed = None            # last player play we've fed the boss mind
    _saved_user = ""
    _saved_trusted = []
    if COLLECTION_OK:
        try:
            _saved_user = PBCOLLECTION.get_pref("username") or ""
            _saved_trusted = PBCOLLECTION.get_pref("trusted_users") or []
        except Exception:
            pass
    mp_state = {"role": None, "status": "", "ip_input": "", "editing": None,
                "peer": None, "local_ips": (netplay._local_ips() if NET_OK else []),
                "spectator": False, "user": _saved_user, "trusted": list(_saved_trusted),
                "channel": "http"}

    # unified input (mouse + touch + up to 2 controllers for hotseat 1v1)
    hub = None
    if INPUTS_OK:
        hub = inputs.InputHub(lambda p: win_to_canvas(p), W, H, players=2)

    def net_start_battle(role, peer, seed, seats=None, match_mode="1v1"):
        """Spin up a deterministic networked battle for host/client."""
        nonlocal g, scene
        set_arena("study")
        gg = Game(deck=DECK[:], netmode=role, seed=seed,
                  seats=seats or [0], match_mode=match_mode)
        gg.net = peer
        return gg

    def win_to_canvas(pos):
        surf = pygame.display.get_surface()
        ww, wh = surf.get_size()
        return _win_to_canvas_geom(pos, ww, wh)

    def start_flip(to):
        nonlocal flip
        flip = {"img": canvas.copy(), "t": 0.0, "to": to}
        sfx("card_draw", 0.6)     # a soft page-turn as the scene flips

    while running:
        cap = FPS_CAP if FPS_CAP in FPS_CYCLE else 60
        # ── battery-saver adaptive framerate ──────────────────────────────────
        # Menus and other static scenes don't need the full refresh rate; on a
        # phone, rendering a still menu at 60fps burns battery for nothing. When
        # the scene is static (and no flip animation is mid-play), cap the frame
        # rate low. A battle always runs at the full cap so play stays smooth.
        # Recent input briefly restores full rate so menu taps feel responsive.
        if BATTERY_SAVER and scene != "battle" and flip is None:
            since_input = t - _last_input_t
            idle_cap = 30 if since_input < 1.2 else 10   # snappy right after a tap
            eff_cap = min(cap, idle_cap)
        else:
            eff_cap = cap
        dt = clock.tick(eff_cap) / 1000.0
        # feed the SIM accumulator at GAME_SPEED (0.85 → 15% slower battles). The
        # real-time clock `t` below is left un-scaled so menus/animations/UI stay
        # snappy; only the battle simulation is paced down.
        acc = min(acc + dt * GAME_SPEED, 0.25)   # clamp to avoid spiral-of-death
        t += dt
        # ── reflow the UI if the window size changed (portrait ⇄ landscape) ──
        surf0 = pygame.display.get_surface()
        cur_size = surf0.get_size() if surf0 else (W, H)
        if cur_size != _win_size_seen and cur_size[0] > 1 and cur_size[1] > 1:
            _win_size_seen = cur_size
            old_ar = _LAYOUT_AR
            relayout(*cur_size)
            # only rebuild surfaces/board when the canvas actually resized
            if (W, H) != _canvas_size:
                _canvas_size = (W, H)
                canvas = pygame.Surface((W, H))
                set_arena(CUR_ARENA)               # rebuild board for new geometry
                globals()["BOARD_STATIC"] = None
                if hub:                            # keep input hub's canvas size current
                    try: hub.W, hub.H = W, H
                    except Exception: pass
        # keep the view-flip in sync before input maps through unproj()
        globals()["VIEW_FLIP"] = bool(scene == "battle" and g and getattr(g, "view_flip", False))
        # a card drag only lives during active battle play
        if drag and not (scene == "battle" and g and not g.result):
            drag = None
        mouse = win_to_canvas(pygame.mouse.get_pos())
        # translate raw input into pointer events (mouse+touch+pads)
        raw_events = pygame.event.get()
        pointer_events = hub.process(raw_events, dt) if hub else []
        # note any real user input so the battery-saver can briefly restore full
        # frame rate for responsive menu taps/scrolls.
        if any(e.type in (pygame.MOUSEBUTTONDOWN, pygame.MOUSEBUTTONUP, pygame.MOUSEMOTION,
                          pygame.KEYDOWN, pygame.FINGERDOWN, pygame.FINGERMOTION, pygame.MOUSEWHEEL)
               for e in raw_events):
            globals()["_last_input_t"] = t
        for e in raw_events:
            if e.type == pygame.QUIT: running = False
            elif scene == "rts" and RTS_GAME is not None and flip is None:
                # Foldworks RTS owns its own input. Translate pointer coords from
                # the window into canvas space so clicks land where they're drawn.
                if e.type in (pygame.MOUSEBUTTONDOWN, pygame.MOUSEBUTTONUP, pygame.MOUSEMOTION):
                    ce = pygame.event.Event(e.type, {**e.dict, "pos": win_to_canvas(e.pos)})
                    RTS_GAME.handle_event(ce)
                elif e.type == pygame.MOUSEWHEEL:
                    RTS_GAME.handle_event(e)
                elif e.type in (pygame.JOYAXISMOTION, pygame.JOYBUTTONDOWN,
                                pygame.JOYBUTTONUP, pygame.JOYHATMOTION):
                    RTS_GAME.handle_event(e)
                    if getattr(RTS_GAME, "exit_requested", False):
                        globals()["RTS_GAME"] = None; scene = "menu"; start_flip("menu")
                elif e.type in (pygame.FINGERDOWN, pygame.FINGERUP, pygame.FINGERMOTION):
                    # convert normalized finger coords → window px → canvas space
                    ww, wh = screen.get_size()
                    cpos = win_to_canvas((e.x * ww, e.y * wh))
                    fid = getattr(e, "finger_id", getattr(e, "fingerId", 0))
                    ce = pygame.event.Event(e.type, {**e.dict, "pos": cpos, "finger_id": fid})
                    RTS_GAME.handle_event(ce)
                elif e.type == pygame.KEYDOWN:
                    if e.key == pygame.K_ESCAPE and RTS_GAME.done:
                        globals()["RTS_GAME"] = None; scene = "menu"; start_flip("menu")
                    else:
                        RTS_GAME.handle_event(e)
                        # ESC-to-menu: the RTS raises exit_requested when ESC is
                        # pressed with nothing selected (or on the setup screen).
                        if getattr(RTS_GAME, "exit_requested", False):
                            globals()["RTS_GAME"] = None; scene = "menu"; start_flip("menu")
                elif e.type == pygame.QUIT:
                    running = False
            elif e.type == pygame.VIDEORESIZE:
                # DO NOT call set_mode() here — doing so re-emits VIDEORESIZE on
                # several compositors and spins an infinite resize loop (the bug
                # that made the app unusable). With plain RESIZABLE the surface
                # already tracks the window; we just reflow the layout below when
                # we notice the size changed.
                pass
            elif e.type == pygame.ACTIVEEVENT:
                # legacy focus event (still emitted on some platforms)
                if hasattr(e, "gain") and hasattr(e, "state"):
                    globals()["_APP_ACTIVE"] = bool(e.gain)
            elif e.type in _BG_EVENTS:
                globals()["_APP_ACTIVE"] = False
            elif e.type in _FG_EVENTS:
                globals()["_APP_ACTIVE"] = True
            elif e.type == pygame.TEXTINPUT and scene == "mp" and mp_state.get("editing"):
                # Wayland/mobile virtual keyboards deliver composed characters as
                # TEXTINPUT, NOT KEYDOWN — this is the branch that makes the OSK
                # actually work on postmarketOS/phosh/squeekboard.
                _lan_text_insert(mp_state, e.text)
            elif e.type == pygame.KEYDOWN and scene == "mp" and mp_state.get("editing"):
                if e.key == pygame.K_BACKSPACE:
                    _lan_text_backspace(mp_state)
                elif e.key in (pygame.K_RETURN, pygame.K_KP_ENTER, pygame.K_ESCAPE):
                    if mp_state.get("editing") == "user" and COLLECTION_OK:
                        try: PBCOLLECTION.set_pref("username", mp_state.get("user", ""))
                        except Exception: pass
                    mp_state["editing"] = None
                    try: pygame.key.stop_text_input()
                    except Exception: pass
                elif e.unicode:
                    # hardware-keyboard fallback (some backends only send KEYDOWN)
                    _lan_text_insert(mp_state, e.unicode)
            elif e.type == pygame.KEYDOWN and scene == "battle" and g and not g.result:
                # ── keyboard card-select. Two key banks so two players can share
                #    one keyboard, mapped by their position on the map:
                #      bottom/first player  : 1 2 3 4   or the home-row a o e u
                #      top/second player    : 7 8 9 0   or the home-row h t n s
                #    Each bank picks hand slot 0..3. Works in every applicable mode
                #    (single-player, hotseat 1v1, 2v2 dual-local, and net play).
                ch = (e.unicode or "").lower()
                bottom_keys = {"1": 0, "2": 1, "3": 2, "4": 3,
                               "a": 0, "o": 1, "e": 2, "u": 3}
                top_keys = {"7": 0, "8": 1, "9": 2, "0": 3,
                            "h": 0, "t": 1, "n": 2, "s": 3}
                if ch in bottom_keys:
                    _kbd_select(g, "bottom", bottom_keys[ch])
                elif ch in top_keys:
                    _kbd_select(g, "top", top_keys[ch])
            elif e.type == pygame.MOUSEBUTTONDOWN and flip is None:
                m = win_to_canvas(e.pos)
                # mouse wheel over the deck builder cycles a card's variant stack
                if scene == "deck" and e.button in (4, 5):
                    for r, key in HOT.get("cards", []):
                        if r.collidepoint(m):
                            stack = _owned_variants(key)
                            if len(stack) > 1:
                                d = 1 if e.button == 5 else -1
                                DECK_STACK[key] = (DECK_STACK.get(key, 0) + d) % len(stack)
                                DECK_STACK_ANIM[key] = (d, t)
                                globals()["DECK_DETAIL"] = key
                                sfx("click")
                            break
                    continue
                # a soft UI click whenever a menu-scene press lands on a control
                if scene in ("menu", "deck", "arena", "options", "mp", "lobby",
                             "collection", "towers", "debug", "forest"):
                    if _hit_any_hot(m):
                        sfx("click")
                if scene == "menu":
                    if HOT.get("play") and HOT["play"].collidepoint(m):
                        set_arena(sel_arena); g = Game(boss=boss_for(sel_arena), deck=DECK[:]); scene = "battle"; start_flip("battle")
                    elif HOT.get("aiboss") and HOT["aiboss"].collidepoint(m) and AIBOSS_OK:
                        # an LLM/heuristic-driven Arch-Scribe reads your board and
                        # adapts. Random persona each time for variety.
                        set_arena(sel_arena)
                        persona = random.choice(ai_boss.list_personas())
                        g = Game(boss="ai:" + persona, deck=DECK[:])
                        scene = "battle"; start_flip("battle")
                    elif HOT.get("local1v1") and HOT["local1v1"].collidepoint(m):
                        set_arena(sel_arena); g = Game(deck=DECK[:], local_1v1=True); scene = "battle"; start_flip("battle")
                    elif HOT.get("mp") and HOT["mp"].collidepoint(m):
                        mp_state.update(status="", role=None); scene = "mp"; start_flip("mp")
                    elif HOT.get("rts") and HOT["rts"].collidepoint(m) and RTS_OK:
                        if EXPANSIONS_OK:
                            try: expmod.apply_rts(rtsmod)   # merge enabled packs' maps
                            except Exception: pass
                        globals()["RTS_GAME"] = rtsmod.RTS(dict(
                            W=W, H=H, font=font, sfont=sfont, bfont=bfont,
                            draw_unit=None, cards=CARDS, origami=origami,
                            sfx=sfx))  # opens picker
                        scene = "rts"; start_flip("rts")
                    elif HOT.get("deck") and HOT["deck"].collidepoint(m): scene = "deck"; start_flip("deck")
                    elif HOT.get("collection") and HOT["collection"].collidepoint(m): scene = "collection"; start_flip("collection")
                    elif HOT.get("towers") and HOT["towers"].collidepoint(m): scene = "towers"; start_flip("towers")
                    elif HOT.get("arena") and HOT["arena"].collidepoint(m): scene = "arena"; start_flip("arena")
                    elif HOT.get("options") and HOT["options"].collidepoint(m): scene = "options"; start_flip("options")
                    elif HOT.get("debug") and HOT["debug"].collidepoint(m): scene = "debug"; start_flip("debug")
                    elif HOT.get("replays") and HOT["replays"].collidepoint(m) and REPLAY_OK:
                        globals()["REPLAY_LIST"] = None      # refresh list
                        scene = "replays"; start_flip("replays")
                    elif HOT.get("quit") and HOT["quit"].collidepoint(m): running = False
                elif scene == "deck":
                    # hit-test in integer canvas space: win_to_canvas returns floats
                    # (the display is scaled), and a sub-pixel x sitting just inside
                    # a gap/boundary could resolve to the neighbouring chip. Rounding
                    # to the nearest pixel makes clicks land on the slot you see.
                    m = (int(round(m[0])), int(round(m[1])))
                    if HOT.get("back") and HOT["back"].collidepoint(m):
                        _save_deck_slots(); scene = "menu"; start_flip("menu")
                    # deck-slot chips: snap to the NEAREST chip within the row's band
                    # rather than requiring an exact hit. The chips have small (6px)
                    # gaps, and on a scaled/touch display a tap aimed at slot 4 could
                    # land in the gap or a pixel into slot 3; nearest-centre snapping
                    # makes the tap select the chip you were aiming at.
                    slots = HOT.get("slots", [])
                    if slots:
                        band = slots[0][0]
                        row_top = min(r.top for r, _ in slots) - 6
                        row_bot = max(r.bottom for r, _ in slots) + 6
                        row_lo = min(r.left for r, _ in slots) - 8
                        row_hi = max(r.right for r, _ in slots) + 8
                        if row_top <= m[1] <= row_bot and row_lo <= m[0] <= row_hi:
                            nearest = min(slots, key=lambda ri: abs(ri[0].centerx - m[0]))
                            _save_deck_slots(); select_deck_slot(nearest[1])
                            globals()["DECK_PAGE"] = 0
                    for r, tag in HOT.get("filters", []):
                        if r.collidepoint(m):
                            kind, val = tag
                            if kind == "filter": globals()["DECK_FILTER"] = val
                            elif kind == "random":
                                # roll a fresh curve-balanced deck into this slot
                                DECK[:] = _random_deck()
                                _save_deck_slots()
                                globals()["DECK_DETAIL"] = None
                                sfx("coin")
                            else: globals()["DECK_SORT"] = "name" if DECK_SORT == "cost" else "cost"
                            globals()["DECK_PAGE"] = 0     # reset to first page on re-filter
                    for r, tag in HOT.get("pager", []):
                        if r.collidepoint(m):
                            globals()["DECK_PAGE"] = DECK_PAGE + (1 if tag == "next" else -1)
                    for r, key in HOT.get("cards", []):
                        if r.collidepoint(m):
                            # first tap pins the detail; tapping the pinned card toggles it
                            if DECK_DETAIL != key:
                                globals()["DECK_DETAIL"] = key
                            else:
                                # add/remove the currently-shown printing (variant)
                                stack = _owned_variants(key)
                                shown = stack[DECK_STACK.get(key, 0) % len(stack)]
                                shown = shown if shown in CARDS else key
                                present = next((dk for dk in DECK
                                                if (carddata.base_of(dk) if COLLECTION_OK else dk) == key), None)
                                if present is not None:
                                    DECK.remove(present)
                                elif len(DECK) < 8:
                                    DECK.append(shown)
                                _save_deck_slots()
                            # begin a possible drag-to-cycle on a stacked card
                            if len(_owned_variants(key)) > 1:
                                globals()["_DECK_DRAG"] = {"key": key, "x0": m[0],
                                                           "base": DECK_STACK.get(key, 0)}
                            break
                    for r, i in HOT.get("deckslots", []):
                        if r.collidepoint(m) and i < len(DECK):
                            DECK.pop(i); _save_deck_slots()
                            break
                elif scene == "arena":
                    if HOT.get("back") and HOT["back"].collidepoint(m): scene = "menu"; start_flip("menu")
                    elif HOT.get("playarena") and HOT["playarena"].collidepoint(m):
                        set_arena(sel_arena); g = Game(boss=boss_for(sel_arena), deck=DECK[:]); scene = "battle"; start_flip("battle")
                    for r, mk in HOT.get("modes", []):
                        if r.collidepoint(m): globals()["MODE"] = mk
                    for r, key in HOT.get("arenas", []):
                        if r.collidepoint(m): sel_arena = key
                elif scene == "options":
                    if HOT.get("back") and HOT["back"].collidepoint(m):
                        sfx("back"); scene = "menu"; start_flip("menu")
                    if HOT.get("expansions") and HOT["expansions"].collidepoint(m) and EXPANSIONS_OK:
                        sfx("select"); scene = "expansions"; start_flip("expansions")
                    for r, preset in HOT.get("gfx", []):
                        if r.collidepoint(m):
                            globals()["GFX"] = preset
                            globals()["BOARD_STATIC"] = None   # rebuild board texture
                            sfx("select")
                    for r, key in HOT.get("toggles", []):
                        if r.collidepoint(m):
                            sfx("toggle")
                            if key == "fps":
                                i = FPS_CYCLE.index(FPS_CAP) if FPS_CAP in FPS_CYCLE else 0
                                globals()["FPS_CAP"] = FPS_CYCLE[(i + 1) % len(FPS_CYCLE)]
                            elif key == "orient": globals()["ORIENT_LOCK"] = "free" if ORIENT_LOCK == "landscape" else "landscape"
                            elif key == "pausebg": globals()["PAUSE_BG"] = not PAUSE_BG
                            elif key == "ssample": globals()["SUPERSAMPLE"] = not SUPERSAMPLE
                            elif key == "battsave":
                                globals()["BATTERY_SAVER"] = not BATTERY_SAVER
                                if COLLECTION_OK:
                                    try: PBCOLLECTION.set_pref("battery_saver", BATTERY_SAVER)
                                    except Exception: pass
                            elif key == "sigilbadges":
                                globals()["SIGIL_BADGES"] = not SIGIL_BADGES
                                if COLLECTION_OK:
                                    try: PBCOLLECTION.set_pref("sigil_badges", SIGIL_BADGES)
                                    except Exception: pass
                            elif key == "piecescale":
                                globals()["PIECE_SCALE_I"] = (PIECE_SCALE_I + 1) % len(PIECE_SCALE_STEPS)
                                globals()["BIG_UNITS"] = PIECE_SCALE_I != 0   # keep compat flag
                                if COLLECTION_OK:
                                    try: PBCOLLECTION.set_pref("piece_scale_i", PIECE_SCALE_I)
                                    except Exception: pass
                    if HOT.get("mute") and HOT["mute"].collidepoint(m):
                        globals()["MUTED"] = not MUTED
                        if SOUND_OK and sound: sound.set_muted(MUTED)
                        if COLLECTION_OK:
                            try: PBCOLLECTION.set_pref("muted", MUTED)
                            except Exception: pass
                        sfx("toggle")
                    if HOT.get("volslider") and HOT["volslider"].collidepoint(m):
                        tr = HOT["volslider"]
                        v = max(0.0, min(1.0, (m[0] - tr.x) / max(1, tr.w)))
                        globals()["VOLUME"] = v
                        if SOUND_OK and sound: sound.set_master(v)
                        dragging_vol = True         # begin dragging the knob
                        sfx("click")
                elif scene == "expansions":
                    if HOT.get("back") and HOT["back"].collidepoint(m):
                        sfx("back"); scene = "options"; start_flip("options")
                    elif EXPANSIONS_SCENE is not None and EXPANSIONS_SCENE.clicked(m):
                        sfx("toggle")
                elif scene == "mp":
                    if HOT.get("back") and HOT["back"].collidepoint(m):
                        if mp_state["peer"]: mp_state["peer"].stop(); mp_state["peer"] = None
                        try: pygame.key.stop_text_input()
                        except Exception: pass
                        mp_state["editing"] = None
                        scene = "menu"; start_flip("menu")
                    elif HOT.get("ipfield") and HOT["ipfield"].collidepoint(m):
                        _begin_edit(mp_state, "ip", HOT["ipfield"])
                    elif HOT.get("userfield") and HOT["userfield"].collidepoint(m):
                        _begin_edit(mp_state, "user", HOT["userfield"])
                    elif any(r.collidepoint(m) for r, _ in HOT.get("numpad", [])):
                        for r, kk in HOT.get("numpad", []):
                            if r.collidepoint(m):
                                sfx("click")
                                if kk == "⌫":
                                    mp_state["ip_input"] = mp_state.get("ip_input", "")[:-1]
                                elif len(mp_state.get("ip_input", "")) < 21:
                                    mp_state["ip_input"] = mp_state.get("ip_input", "") + kk
                                break
                    elif HOT.get("scan") and HOT["scan"].collidepoint(m) and NET_OK:
                        # Run the LAN scan on a background thread. A blocking scan
                        # on the main loop would stop pumping events for its whole
                        # timeout, which some mobile/Wayland compositors treat as a
                        # hung window and freeze or kill it.
                        if not mp_state.get("scanning"):
                            mp_state["scanning"] = True
                            mp_state["status"] = "Scanning the LAN…"
                            def _do_scan(state=mp_state):
                                try:
                                    hosts = netplay.scan_for_hosts(port=DEFAULT_PORT, timeout=0.6)
                                    state["hosts"] = hosts
                                    state["status"] = f"Found {len(hosts)} table(s)."
                                except Exception as ex:
                                    state["status"] = f"Scan failed: {ex}"
                                finally:
                                    state["scanning"] = False
                            import threading as _thr
                            _thr.Thread(target=_do_scan, daemon=True).start()
                    else:
                        # click a discovered host row to join it
                        joined_host = False
                        for r, h in HOT.get("hosts", []):
                            if r.collidepoint(m) and NET_OK:
                                try:
                                    want = 2 if (h.get("mode") == "2v2" and mp_state.get("two_here")) else 1
                                    peer = netplay.Client(h["ip"], port=h.get("port", DEFAULT_PORT), want_seats=want,
                                                          user=(mp_state.get("user") or "Player"),
                                                          transport=h.get("transport", mp_state.get("channel", "http")),
                                                          http_port=h.get("port"))
                                    mp_state.update(role="client", peer=peer, editing=False,
                                                    match_mode=h.get("mode", "1v1"),
                                                    status=f"Joining {h['name']}…")
                                    scene = "lobby"; start_flip("lobby")
                                except Exception as ex:
                                    mp_state["status"] = f"Join failed: {ex}"
                                joined_host = True; break
                        if joined_host:
                            pass
                        else:
                            for r, mk in HOT.get("mmodes", []):
                                if r.collidepoint(m): mp_state["match_mode"] = mk
                            for r, ck in HOT.get("channels", []):
                                if r.collidepoint(m): mp_state["channel"] = ck; sfx("toggle")
                            if HOT.get("twohere") and HOT["twohere"].collidepoint(m):
                                mp_state["two_here"] = not mp_state.get("two_here", False)
                            if HOT.get("host") and HOT["host"].collidepoint(m) and NET_OK:
                                try:
                                    mm = mp_state.get("match_mode", "1v1")
                                    hs = 2 if (mm == "2v2" and mp_state.get("two_here")) else 1
                                    peer = netplay.Host(port=DEFAULT_PORT, mode=mm, host_seats=hs,
                                                        host_name=(mp_state.get("user") or "Paperbound Table"),
                                                        require_approval=True,
                                                        trusted=mp_state.get("trusted", []),
                                                        transport=mp_state.get("channel", "http"))
                                    mp_state.update(role="host", peer=peer, my_seats=peer.host_seats,
                                                    status=f"Hosting {mm}… waiting for players.")
                                    # share the deterministic card index so deploys
                                    # travel as 2-byte ids, not strings.
                                    if NET_OK:
                                        try: netplay.set_card_index(list(BASE_CARD_KEYS))
                                        except Exception: pass
                                    # tag the host with our ruleset stamp so joiners
                                    # can confirm they're on the same patch set.
                                    if CARDDATA_OK and PATCHTREE_OK:
                                        try:
                                            ap = getattr(carddata, "ACTIVE_PATCH", None) or {}
                                            peer.ruleset_stamp = patchtree.ruleset_stamp(
                                                carddata.data_stamp(),
                                                ap.get("hash", "") if isinstance(ap, dict) else "")
                                        except Exception:
                                            peer.ruleset_stamp = ""
                                    scene = "lobby"; start_flip("lobby")
                                except Exception as ex:
                                    mp_state["status"] = f"Host failed: {ex}"
                            elif HOT.get("join") and HOT["join"].collidepoint(m) and NET_OK:
                                ip = mp_state["ip_input"].strip() or "127.0.0.1"
                                try:
                                    want = 2 if (mp_state.get("match_mode") == "2v2" and mp_state.get("two_here")) else 1
                                    peer = netplay.Client(ip, port=DEFAULT_PORT, want_seats=want,
                                                      user=(mp_state.get("user") or "Player"),
                                                      transport=mp_state.get("channel", "http"))
                                    mp_state.update(role="client", peer=peer, editing=False,
                                                    status=f"Connecting to {ip}…")
                                    scene = "lobby"; start_flip("lobby")
                                except Exception as ex:
                                    mp_state["status"] = f"Join failed: {ex}"
                            elif HOT.get("spectate") and HOT["spectate"].collidepoint(m) and NET_OK:
                                ip = mp_state["ip_input"].strip() or "127.0.0.1"
                                try:
                                    peer = netplay.Client(ip, port=DEFAULT_PORT, spectator=True,
                                                      user=(mp_state.get("user") or "Player"),
                                                      transport=mp_state.get("channel", "http"))
                                    mp_state.update(role="spectator", peer=peer, editing=False,
                                                    status=f"Spectating {ip}…")
                                except Exception as ex:
                                    mp_state["status"] = f"Spectate failed: {ex}"
                elif scene == "lobby":
                    if HOT.get("leaveroom") and HOT["leaveroom"].collidepoint(m):
                        if mp_state["peer"]: mp_state["peer"].stop(); mp_state["peer"] = None
                        mp_state["role"] = None
                        scene = "mp"; start_flip("mp")
                    for r, addr in HOT.get("approve", []):
                        if r.collidepoint(m):
                            peer = mp_state.get("peer")
                            if peer:
                                info = peer.approve(addr, remember=True)
                                # remember the trusted username across sessions
                                if info:
                                    tl = mp_state.setdefault("trusted", [])
                                    if info["user"] not in tl: tl.append(info["user"])
                                    if COLLECTION_OK:
                                        try: PBCOLLECTION.set_pref("trusted_users", tl)
                                        except Exception: pass
                                    # seat them now
                                    want = info.get("want", 1)
                                    seats = peer.assign_seats(addr, want)
                                    peer.register(addr, info.get("role", "client"))
                                    peer.start_match_for(addr, info.get("role", "client"), seats)
                                sfx("coin")
                    for r, addr in HOT.get("deny", []):
                        if r.collidepoint(m):
                            peer = mp_state.get("peer")
                            if peer: peer.deny(addr); sfx("back")
                    for r, i in HOT.get("lobbyslots", []):
                        if r.collidepoint(m): select_deck_slot(i)
                elif scene == "debug":
                    if HOT.get("back") and HOT["back"].collidepoint(m):
                        scene = "menu"; start_flip("menu")
                    elif HOT.get("toggle") and HOT["toggle"].collidepoint(m):
                        globals()["DEBUG_ALL"] = not DEBUG_ALL
                    elif HOT.get("balance") and HOT["balance"].collidepoint(m):
                        globals()["SHOW_BALANCE"] = not globals().get("SHOW_BALANCE", False)
                    elif HOT.get("echostyle") and HOT["echostyle"].collidepoint(m):
                        globals()["ECHO_STYLE"] = "wraith" if globals().get("ECHO_STYLE", "apparition") == "apparition" else "apparition"
                elif scene == "replays":
                    if HOT.get("back") and HOT["back"].collidepoint(m):
                        scene = "menu"; start_flip("menu")
                    else:
                        for rr, path in HOT.get("replay_rows", []):
                            if rr.collidepoint(m):
                                player = open_replay(path)
                                if player is not None:
                                    globals()["REPLAY_PLAYER"] = player
                                    globals()["REPLAY_PLAYING"] = True
                                    globals()["REPLAY_SPEED"] = 1
                                    globals()["REPLAY_ACC"] = 0.0
                                    globals()["REPLAY_LAST_T"] = None
                                    scene = "replayview"; start_flip("replayview")
                                break
                elif scene == "replayview":
                    if HOT.get("rexit") and HOT["rexit"].collidepoint(m):
                        globals()["REPLAY_PLAYER"] = None
                        globals()["REPLAY_CASTER"] = None
                        globals()["REPLAY_CASTER_FOR"] = None
                        scene = "replays"; start_flip("replays")
                    elif HOT.get("playpause") and HOT["playpause"].collidepoint(m):
                        globals()["REPLAY_PLAYING"] = not globals().get("REPLAY_PLAYING", False)
                    elif HOT.get("speed") and HOT["speed"].collidepoint(m):
                        globals()["REPLAY_SPEED"] = {1: 2, 2: 4, 4: 8, 8: 1}.get(globals().get("REPLAY_SPEED", 1), 1)
                    elif HOT.get("scrub") and HOT["scrub"].collidepoint(m) and REPLAY_PLAYER:
                        frac = (m[0] - HOT["scrub"].x) / max(1, HOT["scrub"].w)
                        REPLAY_PLAYER.seek(int(frac * REPLAY_PLAYER.total))
                        globals()["REPLAY_PLAYING"] = False
                        globals()["REPLAY_ACC"] = 0.0
                elif scene == "collection":
                    if HOT.get("back") and HOT["back"].collidepoint(m): scene = "menu"; start_flip("menu")
                elif scene == "towers":
                    if HOT.get("back") and HOT["back"].collidepoint(m): scene = "menu"; start_flip("menu")
                    for r, tk in HOT.get("towers", []):
                        if r.collidepoint(m):
                            globals()["TOWER_TROOP"] = tk
                            if COLLECTION_OK:
                                try: PBCOLLECTION.set_pref("tower_troop", tk)
                                except Exception: pass
                elif scene == "forest":
                    if forest_ev is not None and forest_ev.get("rock"):
                        # the Rock of Power: tap a card to imbue it with divergence
                        if forest_ev.get("granted") is None:
                            for r, key in HOT.get("rockcards", []):
                                if r.collidepoint(m):
                                    vk = carddata.make_variant(key, "spiritdiverge") if CARDDATA_OK else key
                                    if COLLECTION_OK:
                                        PBCOLLECTION.grant(vk, how="rock")
                                    forest_ev["granted"] = key
                                    sfx("coin")
                                    break
                        if HOT.get("rockcontinue") and HOT["rockcontinue"].collidepoint(m):
                            forest_ev = None; scene = "menu"; start_flip("menu")
                    elif forest_ev is not None:
                        if forest_ev["chosen"] is None:
                            for r, i in HOT.get("paths", []):
                                if r.collidepoint(m):
                                    forest_ev["chosen"] = i
                                    p = forest_ev["paths"][i]
                                    vk = carddata.make_variant(p["key"], p["variant"]) if CARDDATA_OK else p["key"]
                                    if COLLECTION_OK:
                                        PBCOLLECTION.grant(vk, how="forest")
                        elif HOT.get("fcontinue") and HOT["fcontinue"].collidepoint(m):
                            forest_ev = None; scene = "menu"; start_flip("menu")
                elif scene == "battle":
                    if g.result:
                        was_local = (g.netmode is None and not g.local_1v1)
                        if g.net: g.net.stop()
                        if COLLECTION_OK:
                            try: PBCOLLECTION.record_match()
                            except Exception: pass
                        # after a single-player match, sometimes a roaming fork
                        # appears — and rarely it's the Rock of Power instead.
                        if was_local and COLLECTION_OK and PBCOLLECTION.event_ready("forest", 45):
                            if random.random() < 0.14:
                                forest_ev = make_rock_event()
                            else:
                                forest_ev = make_forest_event()
                            PBCOLLECTION.mark_event("forest")
                            scene = "forest"; start_flip("forest"); g = None
                        else:
                            scene = "menu"; start_flip("menu"); g = None
                    elif g.netmode == "spectator":
                        pass                            # spectators only watch
                    else:
                        # the Trickster's flint-&-steel relight (during blackout)
                        if getattr(g, "trick_dark", False) and HOT.get("relight") and HOT["relight"].collidepoint(m):
                            g._trickster_relight()
                            continue
                        # forfeit / exit button works in every mode
                        if HOT.get("forfeit") and HOT["forfeit"].collidepoint(m):
                            sfx("forfeit")
                            # in a net game, tell everyone we've left so the
                            # remaining player gets a proper victory
                            if g.netmode and g.net:
                                try: g.net.send({"t": "forfeit", "team": g.my_team})
                                except Exception: pass
                            g.forfeited = True
                            g.update()          # let the FSM resolve to ENDED
                        # ── begin a card drag if the press landed on a hand card ──
                        # (works for the bottom strip in every mode, and the top
                        #  strip in hotseat / 2v2 dual-local). Selection is also set
                        #  so a plain tap-then-tap still works as before.
                        elif (bi := _hand_slot_at(m[0], m[1], "bottom")) is not None \
                                and (bwho := _drag_who_for(g, "bottom")) is not None:
                            key = _drag_hand_key(g, bwho, bi)
                            if key:
                                drag = {"who": bwho, "idx": bi, "key": key,
                                        "pos": m, "src": "mouse", "moved": False}
                                _kbd_select(g, "bottom", bi)
                        elif (ti := _hand_slot_at(m[0], m[1], "top")) is not None \
                                and (twho := _drag_who_for(g, "top")) is not None:
                            key = _drag_hand_key(g, twho, ti)
                            if key:
                                drag = {"who": twho, "idx": ti, "key": key,
                                        "pos": m, "src": "mouse", "moved": False}
                                _kbd_select(g, "top", ti)
                        elif getattr(g, "dual_local", False) and g.seat_ctrl:
                            # 2v2, two players on this device: seat A = bottom hand,
                            # seat B = top strip. Board taps deploy for the last-
                            # selected seat's controller.
                            seats = list(g.seat_ctrl.keys())
                            cA = g.seat_ctrl[seats[0]]
                            cB = g.seat_ctrl[seats[1]] if len(seats) > 1 else None
                            handledB = False
                            for r, i in HOT.get("hand_b", []):
                                if r.collidepoint(m):
                                    cB["selected"] = i
                                    cA["selected"] = None   # focus goes to seat B
                                    handledB = True; break
                            if handledB:
                                pass
                            elif m[1] > HAND_Y - 45:        # bottom hand → seat A
                                cwid, gp = 108, 14; x0 = W / 2 - (4 * cwid + 3 * gp) / 2
                                idx = int((m[0] - x0) // (cwid + gp))
                                if 0 <= idx < 4:
                                    cA["selected"] = idx
                                    if cB: cB["selected"] = None
                            else:
                                lx, ly = unproj(*m)
                                if BOOK.collidepoint(lx, ly):
                                    if cB and cB["selected"] is not None:
                                        if g.net_seat_play(seats[1], cB["selected"], (lx, ly)) != "noink":
                                            cB["selected"] = None
                                    elif cA["selected"] is not None:
                                        if g.net_seat_play(seats[0], cA["selected"], (lx, ly)) != "noink":
                                            cA["selected"] = None
                        else:
                            # tap a drifting table-find first (they sit at desk edges)
                            claimed = False
                            for r, tf in HOT.get("tablefinds", []):
                                if r.collidepoint(m):
                                    g.collect_table_find(tf); claimed = True; break
                            if claimed:
                                pass
                            elif m[1] > HAND_Y - 45:
                                cwid, gp = 108, 14; x0 = W / 2 - (4 * cwid + 3 * gp) / 2
                                idx = int((m[0] - x0) // (cwid + gp))
                                if 0 <= idx < 4: g.selected = idx
                            elif g.selected is not None:
                                lx, ly = unproj(*m)
                                if BOOK.collidepoint(lx, ly):
                                    if g.netmode:
                                        res = g.net_local_play(g.selected, (lx, ly))
                                    else:
                                        res = g.try_play(g.selected, (lx, ly))
                                    if res != "noink":
                                        g.selected = None
            elif e.type == pygame.MOUSEMOTION and scene == "deck" and _DECK_DRAG:
                # horizontal drag over a stacked card flips through its variants,
                # one step per ~40px, with the little rotate animation.
                mc = win_to_canvas(e.pos)
                dd = _DECK_DRAG
                stack = _owned_variants(dd["key"])
                if len(stack) > 1:
                    steps = int((mc[0] - dd["x0"]) / 40)
                    ni = (dd["base"] + steps) % len(stack)
                    if ni != DECK_STACK.get(dd["key"], 0):
                        DECK_STACK_ANIM[dd["key"]] = (1 if steps >= 0 else -1, t)
                        DECK_STACK[dd["key"]] = ni
            elif e.type == pygame.MOUSEMOTION and dragging_vol and scene == "options":
                tr = HOT.get("volslider")
                if tr:
                    mc = win_to_canvas(e.pos)
                    v = max(0.0, min(1.0, (mc[0] - tr.x) / max(1, tr.w)))
                    globals()["VOLUME"] = v
                    if SOUND_OK and sound: sound.set_master(v)
            elif e.type == pygame.MOUSEMOTION and drag and drag.get("src") == "mouse":
                drag["pos"] = win_to_canvas(e.pos)
                drag["moved"] = True
            elif e.type == pygame.MOUSEBUTTONUP and scene == "deck" and _DECK_DRAG:
                globals()["_DECK_DRAG"] = None
            elif e.type == pygame.MOUSEBUTTONUP and dragging_vol:
                dragging_vol = False
                if COLLECTION_OK:
                    try: PBCOLLECTION.set_pref("volume", VOLUME)
                    except Exception: pass
            elif e.type == pygame.MOUSEBUTTONUP and drag and drag.get("src") == "mouse":
                mp = win_to_canvas(e.pos)
                lx, ly = unproj(*mp)
                if scene == "battle" and g and not g.result and BOOK.collidepoint(lx, ly):
                    _drag_deploy(g, drag["who"], drag["idx"], lx, ly)
                # a press-with-no-drag stays selected (tap-to-place still works)
                drag = None

        # ── pointer-driven battle input (touch / controllers / hotseat P2) ──
        if scene == "battle" and g and not g.result:
            for pe in pointer_events:
                mx, my = pe.x, pe.y
                # ── unified drag-and-drop for touch & controller pointers ──
                # A press on a hand card starts a drag; moving updates the ghost;
                # releasing over the board deploys. Mouse drag is handled in the
                # raw-event loop above; here we cover touch + pad cursors.
                if pe.source in ("touch", "pad"):
                    # figure which strip (top/bottom) this pointer's player uses
                    which = "top" if (pe.player == 1) else "bottom"
                    if pe.kind == "down":
                        slot = _hand_slot_at(mx, my, which)
                        who = _drag_who_for(g, which) if slot is not None else None
                        # fall back to bottom strip hit if the top didn't match
                        if slot is None and which == "top":
                            slot = _hand_slot_at(mx, my, "bottom")
                            who = _drag_who_for(g, "bottom") if slot is not None else None
                        if slot is not None and who is not None:
                            key = _drag_hand_key(g, who, slot)
                            if key:
                                drag = {"who": who, "idx": slot, "key": key,
                                        "pos": (mx, my), "src": pe.source, "moved": False}
                                sfx("select")
                                continue
                    elif pe.kind == "move" and drag and drag.get("src") == pe.source:
                        drag["pos"] = (mx, my); drag["moved"] = True
                        continue
                    elif pe.kind == "up" and drag and drag.get("src") == pe.source:
                        lx, ly = unproj(mx, my)
                        if BOOK.collidepoint(lx, ly):
                            _drag_deploy(g, drag["who"], drag["idx"], lx, ly)
                        drag = None
                        continue
                if pe.kind != "down":
                    continue
                # 2v2 dual-local: seat A (bottom) + seat B (top strip), same team
                if getattr(g, "dual_local", False) and g.seat_ctrl:
                    seats = list(g.seat_ctrl.keys())
                    cA = g.seat_ctrl[seats[0]]
                    cB = g.seat_ctrl[seats[1]] if len(seats) > 1 else None
                    if cB and my < 120:               # top strip → seat B
                        cwid, gp = 96, 12; x0 = W / 2 - (4 * cwid + 3 * gp) / 2
                        idx = int((mx - x0) // (cwid + gp))
                        if 0 <= idx < 4: cB["selected"] = idx; cA["selected"] = None
                        continue
                    if my > HAND_Y - 45:              # bottom hand → seat A
                        cwid, gp = 108, 14; x0 = W / 2 - (4 * cwid + 3 * gp) / 2
                        idx = int((mx - x0) // (cwid + gp))
                        if 0 <= idx < 4:
                            cA["selected"] = idx
                            if cB: cB["selected"] = None
                        continue
                    lx, ly = unproj(mx, my)
                    if BOOK.collidepoint(lx, ly):
                        if cB and cB["selected"] is not None:
                            if g.net_seat_play(seats[1], cB["selected"], (lx, ly)) != "noink":
                                cB["selected"] = None
                        elif cA["selected"] is not None:
                            if g.net_seat_play(seats[0], cA["selected"], (lx, ly)) != "noink":
                                cA["selected"] = None
                    continue
                # player index: pad 1 or a tap on the top hand → player 2 (team 1)
                p2 = g.local_1v1 and (pe.player == 1 or my < H - HAND_Y + 40 and my < 120)
                if g.local_1v1:
                    # P2 hand sits along the very top; P1 hand along the bottom
                    if my < 120:                     # top hand strip → player 2 cards
                        cwid, gp = 96, 12; x0 = W / 2 - (4 * cwid + 3 * gp) / 2
                        idx = int((mx - x0) // (cwid + gp))
                        if 0 <= idx < 4: g.selected2 = idx
                        continue
                    if my > HAND_Y - 45:             # bottom hand → player 1 cards
                        cwid, gp = 108, 14; x0 = W / 2 - (4 * cwid + 3 * gp) / 2
                        idx = int((mx - x0) // (cwid + gp))
                        if 0 <= idx < 4: g.selected = idx
                        continue
                    lx, ly = unproj(mx, my)
                    if BOOK.collidepoint(lx, ly):
                        if pe.player == 1 or ly < RIVER_Y:      # top half or pad2 → P2
                            if g.selected2 is not None:
                                if g.try_play2(g.selected2, (lx, ly)) != "noink":
                                    g.selected2 = None
                        else:
                            if g.selected is not None:
                                if g.try_play(g.selected, (lx, ly)) != "noink":
                                    g.selected = None
                    continue
                # single-player / net: pad + touch cursor drive the normal flow
                if pe.source in ("pad", "touch"):
                    if g.netmode == "spectator":
                        continue                       # spectators only watch
                    if my > HAND_Y - 45:
                        cwid, gp = 108, 14; x0 = W / 2 - (4 * cwid + 3 * gp) / 2
                        idx = int((mx - x0) // (cwid + gp))
                        if 0 <= idx < 4: g.selected = idx
                    elif g.selected is not None:
                        lx, ly = unproj(mx, my)
                        if BOOK.collidepoint(lx, ly):
                            if g.netmode: res = g.net_local_play(g.selected, (lx, ly))
                            else: res = g.try_play(g.selected, (lx, ly))
                            if res != "noink":
                                g.selected = None

        # ── network message pump (lobby + in-battle) ──
        if NET_OK and mp_state["peer"] is not None:
            peer = mp_state["peer"]
            for msg in peer.poll():
                mt = msg.get("t")
                if scene in ("mp", "lobby"):
                    if mt == "hello" and mp_state["role"] == "host":
                        role = msg.get("role", "client")
                        addr = msg["_from"]
                        # ── trust gate: an untrusted joiner becomes a pending
                        #    request the host must approve in the lobby. Trusted
                        #    (remembered) users and spectators of a public game are
                        #    admitted right away. ──
                        admit = peer.note_join_request(addr, msg)
                        if not admit and addr not in peer.addr_seats and role != "spectator":
                            pass                       # waiting on host's decision
                        elif role == "spectator":
                            peer.register(addr, role)
                            peer.start_match_for(addr, role)
                        else:
                            # assign seats to this joining (approved) device
                            if addr not in peer.addr_seats:
                                want = msg.get("want", 1)
                                seats = peer.assign_seats(addr, want)
                                peer.register(addr, role)
                                peer.start_match_for(addr, role, seats)
                            # start once every seat is claimed
                            if peer.all_seats_filled() and g is None:
                                my_seats = peer.host_seats
                                g = net_start_battle("host", peer, peer.seed, seats=my_seats,
                                                     match_mode=peer.match_mode)
                                mp_state["status"] = "All players in!"
                                scene = "battle"; start_flip("battle")
                    elif mt == "start" and mp_state["role"] in ("client", "spectator"):
                        # verify we're on the same patch set as the host before we
                        # commit to a lockstep sim — a mismatch would desync.
                        host_stamp = msg.get("stamp", "")
                        if host_stamp and CARDDATA_OK and PATCHTREE_OK:
                            my_stamp = patchtree.ruleset_stamp(carddata.data_stamp(),
                                getattr(getattr(carddata, "ACTIVE_PATCH", None) or {}, "get", lambda *_: "")("hash", ""))
                            if host_stamp != my_stamp:
                                mp_state["status"] = ("Version mismatch with host — "
                                                      "you're on a different patch set.")
                        set_arena(msg.get("arena", "study"))
                        nm = "spectator" if mp_state["role"] == "spectator" else msg.get("role", "client")
                        mp_state["my_seats"] = msg.get("seats", [])
                        if NET_OK:
                            try: netplay.set_card_index(list(BASE_CARD_KEYS))
                            except Exception: pass
                        g = Game(deck=DECK[:], netmode=nm, seed=msg["seed"],
                                 seats=msg.get("seats", []), match_mode=msg.get("mode", "1v1"))
                        g.net = peer
                        scene = "battle"; start_flip("battle")
                elif scene == "battle" and g is not None and g.netmode:
                    if mt in ("deploy", "inputs", "bundle"):
                        # dedupe by seq (a resent packet must not double-apply),
                        # schedule new inputs, and absorb any piggybacked acks.
                        g.net_recv_deploy(msg)
                        # host relays every deploy/bundle on to all other peers
                        if mp_state["role"] == "host":
                            peer.broadcast(msg, exclude=msg.get("_from"))
                    elif mt == "ack":
                        g.net_note_ack(msg)
                    elif mt == "forfeit":
                        # an opponent forfeit/left: mark their team defeated so the
                        # remaining player gets a victory screen + their candle snuffs
                        fteam = msg.get("team")
                        g.opponent_forfeited(fteam)
                        if mp_state["role"] == "host":
                            peer.broadcast(msg, exclude=msg.get("_from"))
            # client/spectator: keep announcing until the host answers
            if scene in ("mp", "lobby") and mp_state["role"] in ("client", "spectator"):
                if int(t * 2) % 2 == 0:
                    peer.say_hello()
            # in-battle keep-alive + auto-forfeit if the opponent's link drops
            if scene == "battle" and g is not None and g.netmode and not g.result:
                g.net_pump()                  # flush batched deploys, resend, ack
                peer.heartbeat()
                if not getattr(g, "spectator", False) and peer.link_dead(timeout=6.0):
                    # opponent closed their app / left the network → they forfeit,
                    # this player wins. Resolve locally (no ack needed — they're gone).
                    other = 1 if g.my_team == 0 else 0
                    g.opponent_forfeited(other)
                    mp_state["status"] = "Opponent disconnected."

        # ── background pause: when the window loses focus we stop simulating and
        #    drawing to save battery, BUT we keep pumping events and flipping a
        #    static frame so the compositor never sees a frozen/hung window (which
        #    it might otherwise kill). ──
        if PAUSE_BG and not _APP_ACTIVE:
            pygame.time.wait(60)          # idle without spinning the CPU
            surf = pygame.display.get_surface()
            if surf:
                pygame.display.flip()     # keep the window "alive" for the WM
            continue

        # sim
        if scene == "battle" and g:
            while acc >= DT: g.update(); acc -= DT
            # ── AI Director (Living Page): start it, drain + apply its events ──
            if AIDIR_OK and getattr(g, "mode", "classic") == "living" and not g.result:
                if director is None:
                    director = ai_director.Director(director_cfg)
                    director.start(lambda gg=g: _director_state(gg))
                for ev in director.drain():
                    note = apply_director_event(g, ev, director_notes)
                    if note:
                        director_notes.append([note, 4.5])
            elif director is not None:
                director.stop(); director = None
            # age out on-screen director announcements
            if director_notes:
                for n in director_notes: n[1] -= dt
                director_notes = [n for n in director_notes if n[1] > 0]
            # ── AI Boss (Arch-Scribe): read the board, answer the player ──
            if AIBOSS_OK and getattr(g, "ai_boss_id", None) and not g.result:
                if boss_mind is None:
                    persona = ai_boss.BOSS_PERSONAS.get(g.ai_boss_id)
                    if persona:
                        boss_mind = ai_boss.BossMind(persona, director_cfg,
                                                     variant=getattr(g, "boss_variant", None))
                        boss_mind.start(lambda gg=g: _boss_state(gg))
                        # a variant opens the fight with a themed troop entourage
                        if getattr(g, "boss_variant", None) and not getattr(g, "_entourage_spawned", False):
                            _spawn_boss_entourage(g, boss_mind)
                            g._entourage_spawned = True
                if boss_mind is not None:
                    # feed the boss the player's latest play so it reacts to style
                    if g.last_player_play and g.last_player_play != boss_fed:
                        boss_fed = g.last_player_play
                        boss_mind.on_player_move(boss_fed)
                    for act in boss_mind.drain():
                        note = apply_boss_action(g, act)
                        if note:
                            g.boss_notes.append([note, 4.5])
            elif boss_mind is not None:
                boss_mind.stop(); boss_mind = None
            # age out boss announcements
            if getattr(g, "boss_notes", None):
                for n in g.boss_notes: n[1] -= dt
                g.boss_notes = [n for n in g.boss_notes if n[1] > 0]
        else:
            acc = 0.0
            if director is not None:
                director.stop(); director = None
                director_notes = []
            if boss_mind is not None:
                boss_mind.stop(); boss_mind = None; boss_fed = None

        # ── audio state tracking: ambient bed per scene/arena + result stingers ──
        if scene == "battle":
            amb_key = CUR_ARENA
        elif scene in ("menu", "deck", "arena", "options", "collection",
                       "towers", "mp", "lobby", "debug", "forest"):
            amb_key = "study"                 # calm study ambience for menus
        else:
            amb_key = None
        if amb_key != _cur_ambient:
            _cur_ambient = amb_key
            set_ambient(amb_key)
        # play a one-shot when a battle resolves
        cur_result = g.result if (scene == "battle" and g) else None
        if cur_result and cur_result != _last_result:
            won = (cur_result == "win") or (g and getattr(g, "forfeit_notice", None) is not None)
            sfx("victory" if won else "defeat")
        _last_result = cur_result

        # draw current scene
        if scene == "menu": draw_menu(canvas, fonts, t, mouse)
        elif scene == "deck": draw_deckbuilder(canvas, fonts, t, mouse, 0)
        elif scene == "arena": draw_arena_select(canvas, fonts, t, mouse, sel_arena)
        elif scene == "options": draw_options(canvas, fonts, t, mouse)
        elif scene == "expansions" and EXPANSIONS_OK: draw_expansions(canvas, fonts, t, mouse)
        elif scene == "collection": draw_collection(canvas, fonts, t, mouse)
        elif scene == "towers": draw_towers(canvas, fonts, t, mouse)
        elif scene == "mp": draw_multiplayer(canvas, fonts, t, mouse, mp_state)
        elif scene == "lobby": draw_lobby_waiting(canvas, fonts, t, mouse, mp_state)
        elif scene == "debug": draw_debug(canvas, fonts, t, mouse)
        elif scene == "replays": draw_replays(canvas, fonts, t, mouse)
        elif scene == "replayview": draw_replay_view(canvas, fonts, t, mouse, None)
        elif scene == "rts" and RTS_GAME is not None:
            RTS_GAME.update(dt)
            RTS_GAME.draw(canvas)
        elif scene == "forest" and forest_ev:
            if forest_ev.get("rock"): draw_rock(canvas, fonts, t, mouse, forest_ev)
            else: draw_forest(canvas, fonts, t, mouse, forest_ev)
        elif scene == "battle" and g:
            if gfx_hi() and SUPERSAMPLE and SS_FACTOR > 1.0:
                # true supersampling: render the whole battle at SS_FACTOR then
                # smooth-downscale into the canvas for anti-aliased origami + paper
                snap = _ss_snapshot()
                base_board = BOARD_STATIC
                try:
                    _ss_apply(SS_FACTOR, snap)
                    globals()["BOARD_STATIC"] = _ss_board_cache()   # hi-res board (built once)
                    big = pygame.Surface((W, H))
                    render(big, g, font, bfont, sfont)
                finally:
                    _ss_restore(snap)
                    globals()["BOARD_STATIC"] = base_board          # restore base cache
                pygame.transform.smoothscale(big, (W, H), canvas)
                # the HUD's clickable rects were built at SS scale — bring them
                # back to base canvas coords so input hit-testing still matches
                _ss_unscale_hot(SS_FACTOR)
            else:
                render(canvas, g, font, bfont, sfont)
            # dragged card + placement preview draws on top of the finished frame
            if drag and not g.result:
                draw_drag_ghost(canvas, g, drag, t)
            # AI Director announcements — a stack of fading ribbons near the top
            if director_notes:
                for i, (txt, tleft) in enumerate(director_notes[-3:]):
                    a = int(min(1.0, tleft / 1.0) * 255)
                    yy = 120 + i * 34
                    surf = sfont.render("✦ " + txt, True, shade(GOLD, 1.1))
                    bg = pygame.Surface((surf.get_width() + 28, 26), pygame.SRCALPHA)
                    bg.fill((20, 14, 10, min(180, a)))
                    canvas.blit(bg, (W // 2 - bg.get_width() // 2, yy))
                    surf.set_alpha(a)
                    canvas.blit(surf, (W // 2 - surf.get_width() // 2, yy + 4))
            # AI Boss speech — the Arch-Scribe's voice, tinted to its persona
            bn = getattr(g, "boss_notes", None)
            if bn:
                pers = ai_boss.BOSS_PERSONAS.get(getattr(g, "ai_boss_id", None)) if AIBOSS_OK else None
                tint = tuple(pers.palette) if pers else (150, 60, 160)
                speaker = pers.name if pers else "The Arch-Scribe"
                for i, (txt, tleft) in enumerate(bn[-2:]):
                    a = int(min(1.0, tleft / 1.2) * 255)
                    yy = H - HAND_H - 70 - i * 30
                    line = sfont.render(f"{speaker}: “{txt}”", True, shade(tuple(min(255, c + 90) for c in tint), 1.0))
                    bg = pygame.Surface((line.get_width() + 30, 26), pygame.SRCALPHA)
                    bg.fill((*[c // 3 for c in tint], min(190, a)))
                    canvas.blit(bg, (W // 2 - bg.get_width() // 2, yy))
                    line.set_alpha(a)
                    canvas.blit(line, (W // 2 - line.get_width() // 2, yy + 4))

        # page-flip overlay: old screen slides off to the left with a curl shadow
        if flip is not None:
            flip["t"] += dt
            p = min(1.0, flip["t"] / 0.42)
            wpx = max(1, int(W * (1 - p)))
            strip = pygame.transform.scale(flip["img"], (wpx, H))
            canvas.blit(strip, (0, 0))
            shsurf = pygame.Surface((30, H), pygame.SRCALPHA)
            shsurf.fill((0, 0, 0, 120))
            canvas.blit(shsurf, (wpx, 0))
            if p >= 1.0: flip = None

        # seasonal ambient overlay (snow/leaves/bats…) drawn atop every scene —
        # display-only, updated on the real-time clock, never touches the sim
        _update_event_particles(dt)
        draw_event_overlay(canvas)

        # present the reflowed canvas — a plain fit-and-centre; the canvas aspect
        # already matches the window so this fills the screen with no real bars.
        surf = pygame.display.get_surface()
        ww, wh = surf.get_size()
        tf = _present_geom(ww, wh)
        surf.fill((6, 4, 5))
        dw, dh = max(1, int(tf["dw"])), max(1, int(tf["dh"]))
        frame = canvas if (dw, dh) == (W, H) else pygame.transform.smoothscale(canvas, (dw, dh))
        surf.blit(frame, (int(tf["ox"]), int(tf["oy"])))
        pygame.display.flip()
    pygame.quit()

def _headless_shots(screen, fonts):
    global GFX, BOARD_STATIC
    font, bfont, sfont = fonts
    # battle shot in the volcano arena to show weather+hazard
    set_arena("volcano")
    g = Game(deck=DECK[:])
    random.seed(7)
    for i in range(10):
        g.deploy(random.choice(ALL_CARDS), random.uniform(BOOK.left + 70, BOOK.right - 70),
                 random.uniform(RIVER_Y - 200, RIVER_Y + 200), i % 2)
    for _ in range(80): g.update()
    g.lava = 0.26
    # render the same frame in BOTH presets, side by side comparison files
    GFX = "low"; BOARD_STATIC = None
    render(screen, g, font, bfont, sfont)
    pygame.image.save(screen, "/mnt/user-data/outputs/paperbound_volcano.png")
    pygame.image.save(screen, "/mnt/user-data/outputs/paperbound_gfx_low.png")
    GFX = "photorealistic"; BOARD_STATIC = None
    render(screen, g, font, bfont, sfont)
    pygame.image.save(screen, "/mnt/user-data/outputs/paperbound_gfx_photoreal.png")
    GFX = "low"; BOARD_STATIC = None
    # menu + deck builder + arena select shots
    set_arena("study")
    draw_menu(screen, fonts, 0.0, (-1, -1))
    pygame.image.save(screen, "/mnt/user-data/outputs/paperbound_menu.png")
    global DEBUG_ALL
    draw_deckbuilder(screen, fonts, 0.0, (-1, -1), 0)
    pygame.image.save(screen, "/mnt/user-data/outputs/paperbound_deckbuilder.png")
    draw_arena_select(screen, fonts, 0.0, (-1, -1), "volcano")
    pygame.image.save(screen, "/mnt/user-data/outputs/paperbound_arenaselect.png")
    draw_options(screen, fonts, 0.0, (-1, -1))
    pygame.image.save(screen, "/mnt/user-data/outputs/paperbound_options.png")
    mp_state = {"role": None, "status": "", "ip_input": "", "editing": False,
                "peer": None, "local_ips": ["192.168.1.42"]}
    draw_multiplayer(screen, fonts, 0.0, (-1, -1), mp_state)
    pygame.image.save(screen, "/mnt/user-data/outputs/paperbound_multiplayer.png")

    # ── new biome arenas in photorealistic ──
    GFX = "photorealistic"
    for ak in ("sandstorm", "sandbar", "outcrop"):
        set_arena(ak); BOARD_STATIC = None
        random.seed(4)
        gb = Game(boss=boss_for(ak), deck=DECK[:])
        for i in range(10):
            gb.deploy(random.choice(ALL_CARDS),
                      random.uniform(BOOK.left + 80, BOOK.right - 80),
                      random.uniform(RIVER_Y - 190, RIVER_Y + 190), i % 2)
        for _ in range(70): gb.update()
        gb.tide = 0.7
        render(screen, gb, font, bfont, sfont)
        pygame.image.save(screen, f"/mnt/user-data/outputs/paperbound_{ak}.png")
    GFX = "low"; BOARD_STATIC = None
    print("saved menu/deck/arena/options/mp/volcano + 3 biome shots + gfx comparison")

if __name__ == "__main__":
    main()
