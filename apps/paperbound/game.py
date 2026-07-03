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

import pygame

W, H = 1280, 800
TPS = 20
DT = 1.0 / TPS

DESK      = (32, 22, 18)
DESK_GRAIN= (24, 16, 13)
PAGE      = (204, 186, 150)
PAGE_LINE = (184, 166, 130)
COVER     = (58, 22, 20)
RIVER     = (16, 18, 30)
RIVER_HI  = (52, 66, 104)
RIBBON    = (128, 30, 36)
INK       = (18, 16, 22)
P_ACCENT  = (36, 128, 118)
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

CANDLE_R = (1092, 210)
CANDLE_L = (150, 596)

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
}

def set_arena(key):
    global CUR_ARENA, PAGE, PAGE_LINE, DESK, DESK_GRAIN, COVER, RIVER, RIVER_HI, RIBBON
    global BOOK, RIVER_Y, BRIDGE_XS, TOP_Y, BOT_Y, AMBIENT, LIGHTS, LIGHT_MAIN
    global WEATHER, HAZARD, POI, BOARD_STATIC
    a = ARENAS[key]; CUR_ARENA = key
    PAGE = a["page"]; PAGE_LINE = a["page_line"]; DESK = a["desk"]
    DESK_GRAIN = a["desk_grain"]; COVER = a["cover"]; RIVER = a["river"]
    RIVER_HI = a["river_hi"]; RIBBON = a["ribbon"]
    BOOK = pygame.Rect(*a["book"]); RIVER_Y = BOOK.centery
    BRIDGE_XS = (BOOK.left + 155, BOOK.right - 155)
    TOP_Y, BOT_Y = a["top_y"], a["bot_y"]
    AMBIENT = a["ambient"]; LIGHTS = a["lights"]; LIGHT_MAIN = a["light_main"]
    WEATHER = a["weather"]; HAZARD = a["hazard"]; POI = a["poi"]
    BOARD_STATIC = None

random.seed(11)

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
}
SIGIL_COLORS = {"pounce": WAX, "armored": GOLD, "feather": (110, 146, 188),
                "chill": (110, 156, 196), "frenzy": WAX, "drinker": (86, 58, 106),
                "slam": GOLD, "taunt": (104, 130, 76), "toxic": (72, 150, 78),
                "sharp": (150, 36, 42), "relentless": GOLD, "towerbane": (150, 36, 42),
                "sap": (150, 36, 42), "manyfold": (110, 110, 120), "doublestrike": (120, 158, 92),
                "rush": (200, 120, 60), "dormant": (140, 120, 90), "beach": (120, 150, 170),
                "grasp": (140, 70, 140), "thorns": (80, 110, 70), "plague": (150, 140, 70)}

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
              "plague": "Damages a wide area."}

def shade(c, f):
    return tuple(max(0, min(255, int(v * f))) for v in c)

def lerp(a, b, t): return a + (b - a) * t

def proj(x, y):
    t = max(0.0, min(1.0, (y - BOOK.top) / BOOK.h))
    ty = 0.66 * t + 0.34 * t * t
    sy = lerp(TOP_Y, BOT_Y, ty)
    s = lerp(S_FAR, S_NEAR, t)
    sx = CX + (x - BOOK.centerx) * s
    return sx, sy, s

def unproj(sx, sy):
    k = (sy - TOP_Y) / (BOT_Y - TOP_Y)
    k = max(0.0, min(1.0, k))
    t = (-0.66 + math.sqrt(0.66 * 0.66 + 4 * 0.34 * k)) / (2 * 0.34)
    y = BOOK.top + t * BOOK.h
    s = lerp(S_FAR, S_NEAR, t)
    x = (sx - CX) / s + BOOK.centerx
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
    def __init__(self, x, y, team, king=False):
        self.x, self.y, self.team, self.king = x, y, team, king
        self.hp = self.maxhp = 2600 if king else 1500
        self.rng, self.dmg, self.cd, self.t = 235, 60, 0.9, 0.0
        self.size = 46 if king else 36
    @property
    def alive(self): return self.hp > 0

class Unit:
    def __init__(self, key, x, y, team):
        d = CARDS[key]
        self.key, self.team = key, team
        self.x, self.y = x, y
        self.hp = self.maxhp = d["hp"]
        self.dmg, self.spd, self.rng = d["dmg"], d["spd"], d["rng"]
        self.size, self.flying = d["size"], d["flying"]
        self.paper = d["paper"]; self.sigil = d["sigil"]
        self.building = d.get("building", False)
        self.cool = 0.0; self.hits_ignored = 3 if self.sigil == "armored" else 0
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
    @property
    def alive(self): return self.hp > 0

class Game:
    def __init__(self, boss=None, deck=None):
        self.units, self.parts, self.texts, self.projs = [], [], [], []
        self.towers = []
        for team, ky in ((0, BOOK.bottom - 70), (1, BOOK.top + 70)):
            self.towers.append(Tower(BOOK.centerx, ky, team, king=True))
            py = BOOK.bottom - 150 if team == 0 else BOOK.top + 150
            for bx in BRIDGE_XS:
                self.towers.append(Tower(bx, py, team))
        self.ink = 5.0; self.ai_ink = 5.0
        self.deck = deck or ["sabertooth", "polarbear", "crane", "baobab",
                             "wolverine", "pangolin", "frog", "mammoth"]
        self.hand = self.deck[:4]
        self.cycle = self.deck[4:] + self.deck[:4]
        self.next_card = self.cycle[0]
        self.ci = 1
        self.selected = None
        self.time = 0.0
        self.ai_timer = 3.0
        self.hand_reach = None
        self.hand_t = 0.0
        self.result = None          # None | "win" | "lose"
        self.boss = boss            # None or "angler"
        self.hook_timer = 6.0
        self.hook_anim = None       # (unit, t, phase) phase: "cast"/"reel"
        self.hook_lure = (BOOK.centerx + 90, RIVER_Y)
        # weather / hazard / POI runtime state, keyed off active arena globals
        self.flash = 0.0            # lightning brightness pulse
        self.flash_timer = random.uniform(2, 5)
        self.lava = 0.0            # how far lava has crept in from the far edge (0..1)
        self.fissures = []         # list of dicts {x,y,open,phase,timer}
        if HAZARD == "fissures":
            for _ in range(3):
                self.fissures.append(self._new_fissure())
        self.tubs = []             # ink-tub POIs {x,y,owner,progress}
        if POI == "inktubs":
            self.tubs = [dict(x=BOOK.centerx, y=RIVER_Y, owner=None, prog=0.0)]

    def _new_fissure(self):
        return dict(x=random.uniform(BOOK.left + 80, BOOK.right - 80),
                    y=random.choice([RIVER_Y - 150, RIVER_Y + 150, RIVER_Y - 260, RIVER_Y + 260]),
                    open=0.0, state="opening", timer=random.uniform(3, 6))

    def deploy(self, key, x, y, team):
        d = CARDS[key]
        n = d.get("count", 1)
        for i in range(n):
            ox = (i - (n - 1) / 2) * 26
            self.units.append(Unit(key, x + ox, y, team))
        for _ in range(10):
            self.parts.append(Particle(x, y, PAGE, 70, 0.4, 2))

    def try_play(self, idx, pos, team=0):
        if self.result: return
        key = self.hand[idx]
        if self.ink < CARDS[key]["cost"]: return
        x, y = pos
        if y < RIVER_Y + RIVER_H: y = RIVER_Y + RIVER_H + 10
        self.ink -= CARDS[key]["cost"]
        self.deploy(key, x, y, 0)
        # deck cycling: played card goes to back, next-in-line comes up
        self.hand[idx] = self.next_card
        self.next_card = self.cycle[self.ci % len(self.cycle)]
        self.ci += 1

    def enemies_of(self, team):
        return ([u for u in self.units if u.team != team and u.alive] +
                [t for t in self.towers if t.team != team and t.alive])

    def _on_death(self, tgt):
        for _ in range(14):
            self.parts.append(Particle(tgt.x, tgt.y,
                shade(getattr(tgt, "paper", PAGE), 0.9), 120, 0.6, 3))
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

    def update(self):
        if self.result:
            for lst in (self.parts, self.texts):
                for e in lst: e.update()
            self.parts = [p for p in self.parts if p.life > 0]
            self.texts = [t for t in self.texts if t.life > 0]
            return

        self.time += DT
        self.ink = min(10, self.ink + DT / 2.8)
        self.ai_ink = min(10, self.ai_ink + DT / 2.8)

        if self.hand_reach:
            self.hand_t += DT
            if 0.55 < self.hand_t < 0.6 and self.hand_reach[3] is not None:
                self.deploy(self.hand_reach[3], self.hand_reach[0], self.hand_reach[1], 1)
                self.hand_reach = (self.hand_reach[0], self.hand_reach[1],
                                   self.hand_reach[2], None)
            if self.hand_t > 1.2:
                self.hand_reach = None

        self.ai_timer -= DT
        if self.ai_timer <= 0 and self.ai_ink >= 5 and not self.hand_reach:
            key = random.choice(["pangolin", "wolverine", "batcolony", "gorilla",
                                  "cassowary", "honeybadger", "mothswarm"])
            if self.ai_ink >= CARDS[key]["cost"]:
                self.ai_ink -= CARDS[key]["cost"]
                bx = random.choice(BRIDGE_XS)
                self.hand_reach = (bx + random.randint(-30, 30), BOOK.top + 130, bx, key)
                self.hand_t = 0.0
                self.ai_timer = random.uniform(4.5, 7.5)

        # ---- Angler boss: hook a player unit near the river and drag it across
        if self.boss == "angler" and not self.hook_anim:
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
            if u.building or u.hooked: continue
            foes = self.enemies_of(u.team)
            if u.sigil == "towerbane":
                foes = [f for f in foes if isinstance(f, Tower)]
            if not foes: continue
            taunts = [f for f in foes if isinstance(f, Unit) and f.sigil == "taunt"
                      and math.hypot(f.x - u.x, f.y - u.y) < 150]
            pool = taunts or foes
            tgt = min(pool, key=lambda f: math.hypot(f.x - u.x, f.y - u.y))
            dist = math.hypot(tgt.x - u.x, tgt.y - u.y)
            reach = u.rng + getattr(tgt, "size", 20)
            if dist <= reach:
                if u.cool <= 0:
                    dmg = u.dmg
                    if u.pounce: dmg *= 3; u.pounce = False
                    if u.sigil == "frenzy" and u.hp < u.maxhp / 2: u.cool = 0.45
                    else: u.cool = 0.9
                    if u.sigil == "towerbane": dmg *= 1.6
                    if u.sigil == "doublestrike": dmg *= 2      # two folds, one swing
                    hitlist = [tgt]
                    if u.sigil == "plague":                     # swarm hits an area
                        hitlist = [f for f in foes if math.hypot(f.x - u.x, f.y - u.y) <= reach]
                    for h in hitlist:
                        hd = dmg
                        if isinstance(h, Unit) and h.hits_ignored > 0:
                            h.hits_ignored -= 1; hd = 0
                            self.texts.append(FloatText(h.x, h.y - 24, "plated!", GOLD))
                        if isinstance(h, Unit) and h.sigil == "relentless" and h.hp - hd <= 0 and not getattr(h, "_spent", False):
                            h._spent = True; hd = h.hp - 1
                            self.texts.append(FloatText(h.x, h.y - 24, "refuses!", GOLD))
                        h.hp -= hd
                        if h is tgt and hd:
                            self.texts.append(FloatText(h.x, h.y - 24, str(int(hd)), (232, 220, 190)))
                        if u.sigil == "toxic" and isinstance(h, Unit):
                            h.dot = min(h.dot + 24, 60)
                        for _ in range(4):
                            self.parts.append(Particle(h.x, h.y, INK, 80, 0.45, 2))
                        if h.hp <= 0:
                            self._on_death(h)
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
                if u.sigil == "rush" and dist > 160: spd *= 1.5   # charges across open ground
                d = math.hypot(gx - u.x, gy - u.y) or 1
                u.x += (gx - u.x) / d * spd * DT
                u.y += (gy - u.y) / d * spd * DT
                u.face = 1 if gx >= u.x else -1
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
        # chill slow: nearby enemies of a chill unit attack slower (applied as cooldown drag)
        chillers = [u for u in self.units if u.alive and u.sigil == "chill"]
        for u in self.units:
            if u.alive and chillers and any(u.team != c.team and math.hypot(u.x-c.x, u.y-c.y) < 150 for c in chillers):
                u.cool = min(u.cool + 0.4 * DT, 1.4)

        for t in self.towers:
            if not t.alive: continue
            t.t = max(0, t.t - DT)
            foes = [u for u in self.units if u.team != t.team and u.alive and not u.hooked]
            near = [u for u in foes if math.hypot(u.x - t.x, u.y - t.y) < t.rng]
            if near and t.t <= 0:
                tgt = min(near, key=lambda u: math.hypot(u.x - t.x, u.y - t.y))
                self.projs.append([t.x, t.y - 30, tgt, t.dmg])
                t.t = t.cd

        for p in self.projs[:]:
            tgt = p[2]
            if not tgt.alive: self.projs.remove(p); continue
            d = math.hypot(tgt.x - p[0], tgt.y - p[1]) or 1
            sp = 340 * DT
            if d < sp:
                tgt.hp -= p[3]
                self.texts.append(FloatText(tgt.x, tgt.y - 24, str(p[3]), WAX))
                for _ in range(6): self.parts.append(Particle(tgt.x, tgt.y, INK, 90, .4, 2))
                self.projs.remove(p)
            else:
                p[0] += (tgt.x - p[0]) / d * sp; p[1] += (tgt.y - p[1]) / d * sp

        self.units = [u for u in self.units if u.alive]
        for lst in (self.parts, self.texts):
            for e in lst: e.update()
        self.parts = [p for p in self.parts if p.life > 0]
        self.texts = [t for t in self.texts if t.life > 0]

        self._update_environment()

        # ---- win / lose
        p_king = next(t for t in self.towers if t.team == 0 and t.king)
        e_king = next(t for t in self.towers if t.team == 1 and t.king)
        if not e_king.alive: self.result = "win"
        elif not p_king.alive: self.result = "lose"

    def _update_environment(self):
        # lightning: charge a flash, then decay it
        if WEATHER == "lightning":
            self.flash = max(0.0, self.flash - DT * 2.5)
            self.flash_timer -= DT
            if self.flash_timer <= 0:
                self.flash = 1.0
                self.flash_timer = random.uniform(2.5, 6)
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
        # ink tubs: capture by standing near; owner gains bonus ink
        if POI == "inktubs":
            for tub in self.tubs:
                near0 = [u for u in self.units if u.team == 0 and u.alive and math.hypot(u.x-tub["x"], u.y-tub["y"]) < 70]
                near1 = [u for u in self.units if u.team == 1 and u.alive and math.hypot(u.x-tub["x"], u.y-tub["y"]) < 70]
                if near0 and not near1: tub["prog"] = min(1.0, tub["prog"] + DT * 0.5); tub["owner"] = 0
                elif near1 and not near0: tub["prog"] = min(1.0, tub["prog"] + DT * 0.5); tub["owner"] = 1
                if tub["owner"] == 0 and tub["prog"] >= 1.0:
                    self.ink = min(10, self.ink + DT * 0.7)
                elif tub["owner"] == 1 and tub["prog"] >= 1.0:
                    self.ai_ink = min(10, self.ai_ink + DT * 0.7)

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

    # strip-project the worn page texture onto the trapezoid
    LT = make_page_texture()
    for sy in range(int(TOP_Y), int(BOT_Y) + 1):
        k = (sy - TOP_Y) / (BOT_Y - TOP_Y)
        t = (-0.66 + math.sqrt(0.66 * 0.66 + 4 * 0.34 * k)) / (2 * 0.34)
        ly = int(min(BOOK.h - 1, t * BOOK.h))
        s = lerp(S_FAR, S_NEAR, t)
        wpx = int(BOOK.w * s)
        row = LT.subsurface((0, ly, BOOK.w, 1))
        S.blit(pygame.transform.scale(row, (wpx, 1)), (CX - wpx / 2, sy))

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
    # inkwell (left of book)
    ix, iy = 232, 470
    prop_shadow(ix, iy + 26, 34)
    well = [(ix - 26, iy + 26), (ix - 20, iy - 18), (ix + 20, iy - 18), (ix + 26, iy + 26)]
    draw_prism(S, well, 10, (34, 30, 40))
    pygame.draw.ellipse(S, INK, (ix - 15, iy - 32, 30, 12))
    pygame.draw.ellipse(S, (96, 108, 148), (ix - 15, iy - 32, 30, 12), 1)
    pygame.draw.circle(S, (168, 178, 210), (ix + 8, iy - 28), 2)   # glass glint
    # quill leaning out of the well
    pygame.draw.line(S, (210, 202, 184), (ix + 4, iy - 28), (ix + 62, iy - 96), 3)
    pygame.draw.polygon(S, (222, 216, 200),
        [(ix + 26, iy - 52), (ix + 76, iy - 108), (ix + 56, iy - 106), (ix + 20, iy - 44)])
    pygame.draw.polygon(S, shade((222, 216, 200), 0.7),
        [(ix + 26, iy - 52), (ix + 56, iy - 106), (ix + 40, iy - 100), (ix + 20, iy - 44)])
    # scattered spare card, bottom-left in the small candle's pool
    scx, scy, ang = 236, 668, -0.28
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
        pygame.draw.circle(S, (110, 90, 60, 0), (scx, scy), 1)
        px, py = scx + rng.randint(-30, 30), scy + rng.randint(-40, 36)
        pygame.draw.circle(S, shade(PAGE, rng.uniform(0.72, 0.9)), (px, py - 3), rng.randint(1, 2))
    # wax seal stamp near the big candle
    wx, wy = 1014, 356
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
    pts = [(sx + px * wshort, sy + py * wshort * 0.4),
           (sx - px * wshort, sy - py * wshort * 0.4),
           (sx - px * wshort * 0.55 + dx * L, sy - py * wshort * 0.25 + dy * L * 0.45),
           (sx + px * wshort * 0.55 + dx * L, sy + py * wshort * 0.25 + dy * L * 0.45)]
    pygame.draw.polygon(layer, (8, 5, 4, alpha), pts)

def origami_animal(screen, shadow_layer, u):
    sx, sy, sc = proj(u.x, u.y)
    s = u.size * sc * UNIT_SCALE
    grow = 1.0 - u.spawn_t * 1.6
    s = max(6, s * grow)
    f = u.face
    c = u.paper
    hgt = max(4, s * 0.5)          # extrusion height: the 3D read
    cast_shadow(shadow_layer, sx, sy + s * 0.4, 1.0, s, LIGHT_MAIN)
    pygame.draw.ellipse(shadow_layer, (8, 5, 4, 130),
                        (sx - s * 1.15, sy + s * 0.35, s * 2.3, s * 0.7))
    x, y = sx, sy
    if u.riverhop:
        y -= abs(math.sin(pygame.time.get_ticks() * 0.008 + sx)) * s * 0.5
    if u.key in ("crane", "batcolony", "mothswarm"):
        wing = math.sin(pygame.time.get_ticks() * 0.02 + x) * s * 0.35
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
        # legs first (under the body slab)
        for lx0, lx1 in ((-0.85, -0.4), (0.35, 0.8)):
            leg = [(x + s * lx0 * f, y + s * 0.28), (x + s * lx1 * f, y + s * 0.28),
                   (x + s * (lx1 - 0.06) * f, y + s * 0.62), (x + s * (lx0 + 0.06) * f, y + s * 0.62)]
            pygame.draw.polygon(screen, shade(c, 0.55), leg)
        top = draw_prism(screen, body, hgt, c)
        # head wedge rides on the top face
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
    ty_ = y - s * 0.9 - hgt
    pygame.draw.polygon(screen, P_ACCENT if u.team == 0 else E_ACCENT,
        [(x - s, ty_), (x - s + 10, ty_ + 4), (x - s, ty_ + 8)])
    bw = s * 1.8
    pygame.draw.rect(screen, (30, 24, 20), (x - bw / 2, ty_ - 8, bw, 5))
    pygame.draw.rect(screen, P_ACCENT if u.team == 0 else E_ACCENT,
                     (x - bw / 2, ty_ - 8, bw * u.hp / u.maxhp, 5))

def draw_tower(screen, shadow_layer, t):
    sx, sy, sc = proj(t.x, t.y)
    s = t.size * sc * TOWER_SCALE
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
        pygame.draw.circle(screen, SIGIL_COLORS[CARDS[holding]["sigil"]],
                           (card.centerx, card.centery + 16), 7)
        if sfont:
            g2 = sfont.render(CARDS[holding]["name"].split()[0], True, INK)
            screen.blit(g2, (card.centerx - g2.get_width() / 2, card.y + 6))

def build_lightmap(t, flicker=True):
    lm = pygame.Surface((W, H))
    lm.fill(AMBIENT)
    fl = 1.0
    if flicker:
        tt = pygame.time.get_ticks() * 0.001
        fl = 0.92 + 0.06 * math.sin(tt * 11) + 0.04 * math.sin(tt * 23 + 1.7)
    for (lx, ly), maxr, warm in LIGHTS:
        ls = pygame.Surface((W, H))
        ls.fill((0, 0, 0))
        steps = 24
        for i in range(steps, 0, -1):
            r = maxr * i / steps * fl
            k = (1 - i / steps) ** 1.8
            col = tuple(int(warm[j] * k) for j in range(3))
            pygame.draw.circle(ls, col, (int(lx), int(ly)), int(r))
        lm.blit(ls, (0, 0), special_flags=pygame.BLEND_ADD)
    return lm

def draw_card(surf, key, rect, font, sfont, affordable):
    d = CARDS[key]
    body = pygame.Rect(rect)
    pygame.draw.rect(surf, (12, 8, 6), body.move(4, 6), border_radius=8)
    col = shade(PAGE, 1.0 if affordable else 0.66)
    pygame.draw.rect(surf, col, body, border_radius=8)
    pygame.draw.rect(surf, shade(PAGE, 0.45), body, 2, border_radius=8)
    # grubby corners + a crease so cards match the worn language
    rr = random.Random(hash(key) & 0xffff)
    for _ in range(10):
        px = body.x + rr.choice((rr.randint(3, 16), body.w - rr.randint(3, 16)))
        py = body.y + rr.choice((rr.randint(3, 16), body.h - rr.randint(3, 16)))
        pygame.draw.circle(surf, shade(col, rr.uniform(0.7, 0.88)), (px, py), rr.randint(1, 3))
    pygame.draw.line(surf, shade(col, 1.12), (body.x + rr.randint(8, 30), body.y + 4),
                     (body.x + rr.randint(30, 80), body.bottom - 6), 1)
    dummy = Unit(key, 0, 0, 0)
    dummy.spawn_t = 0
    dummy.x, dummy.y = unproj(body.centerx, body.centery + 6)
    sxx, syy, ssc = proj(dummy.x, dummy.y)
    dummy.size = min(d["size"], 18) / ssc
    sh = pygame.Surface((W, H), pygame.SRCALPHA)
    origami_animal(surf, sh, dummy)
    pygame.draw.rect(surf, col, (body.x + 2, body.y + 2, body.w - 4, 16))
    name = sfont.render(d["name"], True, INK)
    surf.blit(name, (body.centerx - name.get_width() / 2, body.bottom - 36))
    pygame.draw.circle(surf, INK, (body.x + 15, body.y + 15), 12)
    pygame.draw.circle(surf, RIVER_HI, (body.x + 15, body.y + 15), 12, 2)
    cost = font.render(str(d["cost"]), True, PAGE)
    surf.blit(cost, (body.x + 15 - cost.get_width() / 2, body.y + 15 - cost.get_height() / 2))
    sc2 = SIGIL_COLORS[d["sigil"]]
    pygame.draw.circle(surf, sc2, (body.right - 16, body.bottom - 16), 10)
    pygame.draw.circle(surf, shade(sc2, 0.6), (body.right - 16, body.bottom - 16), 10, 2)
    pygame.draw.circle(surf, shade(sc2, 1.35), (body.right - 19, body.bottom - 19), 3)

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
            cast_shadow(shadow_layer, sx, sy + e.size * sc * TOWER_SCALE * 0.5, 1.0, e.size * sc * TOWER_SCALE * 1.3,
                        LIGHT_MAIN, stretch=3.0, alpha=115)
    screen.blit(shadow_layer, (0, 0))
    shadow_layer = pygame.Surface((W, H), pygame.SRCALPHA)
    for e in sorted(ents, key=lambda e: e.y):
        if isinstance(e, Tower): draw_tower(screen, shadow_layer, e)
        else: origami_animal(screen, shadow_layer, e)
    # unit shadows composite beneath subsequent light pass
    screen.blit(shadow_layer, (0, 0), special_flags=pygame.BLEND_RGBA_MULT) if False else screen.blit(shadow_layer, (0, 0))

    for p in g.projs:
        px_, py_, _ = proj(p[0], p[1])
        pygame.draw.circle(screen, INK, (int(px_), int(py_)), 5)
        pygame.draw.circle(screen, RIVER_HI, (int(px_), int(py_)), 5, 1)
    for p in g.parts:
        a = max(0, p.life / p.max)
        px_, py_, psc = proj(p.x, p.y)
        pygame.draw.circle(screen, shade(p.col, 0.6 + 0.4 * a),
                           (int(px_), int(py_)), max(1, int(p.r * a * psc)))
    for ft in g.texts:
        px_, py_, _ = proj(ft.x, ft.y)
        img = bfont.render(ft.txt, True, ft.col)
        screen.blit(img, (px_ - img.get_width() / 2, py_))

    if g.boss == "angler" and g.hook_anim:
        tgt, ht, phase = g.hook_anim
        lx, ly, _ = proj(*g.hook_lure)
        tx_, ty_, _ = proj(tgt.x, tgt.y)
        wob = math.sin(tnow * 14) * 6
        pygame.draw.line(screen, (70, 64, 56), (lx, ly), (tx_ + wob, ty_), 2)
        pygame.draw.circle(screen, (40, 36, 32), (int(lx), int(ly)), 5)
        pygame.draw.circle(screen, WAX, (int(tx_ + wob), int(ty_)), 4)

    lm = build_lightmap(tnow)
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
        for (cx, cy), size in ((CANDLE_R, 1.0), (CANDLE_L, 0.7)):
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
            fl2 = (6 + math.sin(tnow * 9 + cx) * 2 + math.sin(tnow * 17) * 1.2) * size
            pygame.draw.circle(screen, (252, 186, 96), (int(cx), int(cy - 10)), int(fl2 * 1.6))
            pygame.draw.circle(screen, (255, 236, 178), (int(cx), int(cy - 11)), int(fl2))
            pygame.draw.circle(screen, (255, 255, 235), (int(cx), int(cy - 12)), max(1, int(fl2 * 0.4)))
        for i in range(22):
            rr3 = random.Random(i * 7)
            mx = CANDLE_R[0] - rr3.uniform(40, 420) + math.sin(tnow * 0.6 + i) * 8
            my = CANDLE_R[1] + rr3.uniform(-60, 300) + math.sin(tnow * 0.4 + i * 2) * 6
            pygame.draw.circle(screen, (200, 184, 150), (int(mx), int(my)), 1)

    draw_weather(screen, g, tnow)

    title = bfont.render("P A P E R B O U N D", True, shade(GOLD, 0.95))
    screen.blit(title, (W / 2 - title.get_width() / 2, 8))
    ch = sfont.render(ARENAS[CUR_ARENA]["chapter"] + " — " + ARENAS[CUR_ARENA]["name"], True, shade(PAGE, 0.65))
    screen.blit(ch, (W / 2 - ch.get_width() / 2, 34))

    cw, chh, gap = 108, 128, 14
    x0 = W / 2 - (4 * cw + 3 * gap) / 2
    for i, key in enumerate(g.hand):
        r = pygame.Rect(x0 + i * (cw + gap), HAND_Y - 30, cw, chh)
        if g.selected == i: r = r.move(0, -12)
        draw_card(screen, key, r, font, sfont, g.ink >= CARDS[key]["cost"])
    nxt = sfont.render("Next: " + CARDS[g.next_card]["name"], True, shade(PAGE, 0.75))
    screen.blit(nxt, (x0 - nxt.get_width() - 18, HAND_Y + 40))
    bar = pygame.Rect(x0 + 4 * (cw + gap) + 6, HAND_Y - 10, 26, 96)
    pygame.draw.rect(screen, (14, 10, 8), bar.inflate(8, 8), border_radius=6)
    pygame.draw.rect(screen, shade(PAGE, 0.35), bar, border_radius=4)
    fill = bar.h * g.ink / 10
    pygame.draw.rect(screen, RIVER_HI, (bar.x, bar.bottom - fill, bar.w, fill), border_radius=4)
    for i in range(1, 10):
        yy = bar.bottom - bar.h * i / 10
        pygame.draw.line(screen, shade(PAGE, 0.55), (bar.x, yy), (bar.right, yy), 1)
    it = font.render(f"{int(g.ink)}", True, PAGE)
    screen.blit(it, (bar.centerx - it.get_width() / 2, bar.bottom + 6))
    screen.blit(sfont.render("INK", True, shade(PAGE, 0.7)), (bar.x - 2, bar.y - 22))

    vg = pygame.Surface((W, H), pygame.SRCALPHA)
    for i in range(10):
        a = 12 + i * 7
        pygame.draw.rect(vg, (4, 2, 6, a), (i * 3, i * 3, W - i * 6, H - i * 6),
                         width=6, border_radius=30)
    screen.blit(vg, (0, 0))

    if g.result:
        ov = pygame.Surface((W, H), pygame.SRCALPHA)
        ov.fill((6, 4, 5, 190))
        screen.blit(ov, (0, 0))
        big = pygame.font.SysFont("georgia,dejavuserif", 64, bold=True)
        msg = "THE PAGE HOLDS" if g.result == "win" else "THE INK RUNS DRY"
        col = shade(GOLD, 1.0) if g.result == "win" else shade(WAX, 1.15)
        img = big.render(msg, True, col)
        screen.blit(img, (W / 2 - img.get_width() / 2, H / 2 - 60))
        sub = sfont.render("Chapter II — The Binder's Study  ·  click to continue", True, shade(PAGE, 0.8))
        screen.blit(sub, (W / 2 - sub.get_width() / 2, H / 2 + 20))

MENU_BOOK = pygame.Rect(232, 92, 816, 604)
ALL_CARDS = list(CARDS.keys())
STARTER = ["sabertooth", "pangolin", "crane", "polarbear", "wolverine",
           "batcolony", "gorilla", "baobab", "frog", "cassowary"]
UNLOCKS = set(STARTER)
DECK = ["sabertooth", "polarbear", "crane", "baobab", "wolverine", "pangolin", "frog", "mammoth"]
DEBUG_ALL = False
HOT = {}          # clickable rects populated during draw, read by event loop

def unlocked(key):
    return DEBUG_ALL or key in UNLOCKS

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
    bf = pygame.font.SysFont("georgia,dejavuserif", 40, bold=True)
    ti = bf.render(title, True, shade(INK, 1.0))
    screen.blit(ti, (b.centerx - ti.get_width() / 2, b.top + 30))
    if subtitle:
        sf = pygame.font.SysFont("georgia,dejavuserif", 17)
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
    f = pygame.font.SysFont("georgia,dejavuserif", 13, italic=True)
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
    # right page: menu buttons
    bx = b.centerx + 70
    button(screen, pygame.Rect(bx, b.top + 150, 300, 58), "Play", bfont, mouse, key="play")
    button(screen, pygame.Rect(bx, b.top + 226, 300, 58), "Deck Builder", bfont, mouse, key="deck")
    button(screen, pygame.Rect(bx, b.top + 302, 300, 58), "Choose Arena", bfont, mouse, key="arena")
    button(screen, pygame.Rect(bx, b.top + 378, 300, 58), "Codex / Debug", bfont, mouse, key="debug")
    button(screen, pygame.Rect(bx, b.top + 454, 300, 58), "Quit", bfont, mouse, key="quit")
    menu_vignette(screen)

def draw_deckbuilder(screen, fonts, t, mouse, scroll):
    font, bfont, sfont = fonts
    draw_desk_and_book(screen, t, "DECK BUILDER", f"choose 8 cards  ·  {len(DECK)}/8 chosen")
    b = MENU_BOOK
    HOT.clear()
    cols, cw, ch, gap = 5, 116, 132, 12
    gx0 = b.left + 40; gy0 = b.top + 120
    HOT["cards"] = []
    for i, key in enumerate(ALL_CARDS):
        r = pygame.Rect(gx0 + (i % cols) * (cw + gap), gy0 + (i // cols) * (ch + gap), cw, ch)
        if r.bottom > b.bottom - 90: continue
        if unlocked(key):
            draw_card(screen, key, r, font, sfont, key in DECK)
            if key in DECK:
                pygame.draw.rect(screen, GOLD, r, 3, border_radius=8)
                pygame.draw.circle(screen, GOLD, (r.right - 12, r.top + 12), 9)
                chk = sfont.render("+", True, INK)
                screen.blit(chk, (r.right - 16, r.top + 4))
            HOT["cards"].append((r, key))
        else:
            draw_torn_card(screen, r)
    # current deck strip along the bottom
    dy = b.bottom - 78
    lab = sfont.render("Your deck:", True, shade(INK, 2.2))
    screen.blit(lab, (b.left + 40, dy - 22))
    for i in range(8):
        r = pygame.Rect(b.left + 40 + i * 92, dy, 84, 60)
        pygame.draw.rect(screen, shade(PAGE, 0.7), r, border_radius=6)
        pygame.draw.rect(screen, shade(INK, 3.5), r, 2, border_radius=6)
        if i < len(DECK):
            d = CARDS[DECK[i]]
            nm = sfont.render(d["name"][:9], True, INK)
            screen.blit(nm, (r.centerx - nm.get_width() / 2, r.centery - 8))
            pygame.draw.circle(screen, INK, (r.left + 12, r.top + 12), 9)
            cc = sfont.render(str(d["cost"]), True, PAGE)
            screen.blit(cc, (r.left + 12 - cc.get_width() / 2, r.top + 12 - cc.get_height() / 2))
    button(screen, pygame.Rect(b.right - 190, dy - 4, 150, 44), "Back", font, mouse, key="back")
    menu_vignette(screen)

def draw_arena_select(screen, fonts, t, mouse, sel):
    font, bfont, sfont = fonts
    draw_desk_and_book(screen, t, "CHOOSE ARENA", "each page a different world")
    b = MENU_BOOK
    HOT.clear(); HOT["arenas"] = []
    keys = list(ARENAS.keys())
    for i, k in enumerate(keys):
        a = ARENAS[k]
        r = pygame.Rect(b.left + 44 + (i % 2) * 380, b.top + 120 + (i // 2) * 100, 356, 84)
        hover = r.collidepoint(mouse)
        swatch = a["page"]
        pygame.draw.rect(screen, shade(swatch, 1.1 if hover else 1.0), r, border_radius=8)
        pygame.draw.rect(screen, a["cover"], r, 3, border_radius=8)
        # mini light swatch + weather/hazard tags
        pygame.draw.circle(screen, a["lights"][0][2], (r.left + 30, r.centery), 14)
        nm = font.render(a["name"], True, INK)
        screen.blit(nm, (r.left + 58, r.top + 14))
        tags = []
        if a["weather"]: tags.append(a["weather"])
        if a["hazard"]: tags.append(a["hazard"])
        if a["poi"]: tags.append(a["poi"])
        tg = sfont.render("  ·  ".join(tags) if tags else "calm", True, shade(INK, 2.2))
        screen.blit(tg, (r.left + 58, r.top + 46))
        if k == sel:
            pygame.draw.rect(screen, GOLD, r, 3, border_radius=8)
        HOT["arenas"].append((r, k))
    button(screen, pygame.Rect(b.centerx - 220, b.bottom - 66, 200, 48), "Play This Arena", font, mouse, key="playarena")
    button(screen, pygame.Rect(b.centerx + 20, b.bottom - 66, 200, 48), "Back", font, mouse, key="back")
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
    # show every card's sigil as a codex
    sf = pygame.font.SysFont("georgia,dejavuserif", 13)
    for i, key in enumerate(ALL_CARDS):
        d = CARDS[key]
        col = INK if unlocked(key) else shade(INK, 3.2)
        x = b.left + 60 + (i // 11) * 300
        y = b.top + 250 + (i % 11) * 28
        pygame.draw.circle(screen, SIGIL_COLORS[d["sigil"]], (x, y + 7), 7)
        nm = sf.render(f"{d['name']} — {SIGIL_DESC.get(d['sigil'],'')}", True, col)
        screen.blit(nm, (x + 16, y))
    button(screen, pygame.Rect(b.right - 190, b.bottom - 66, 150, 48), "Back", font, mouse, key="back")
    menu_vignette(screen)

def menu_vignette(screen):
    vg = pygame.Surface((W, H), pygame.SRCALPHA)
    for i in range(10):
        a = 12 + i * 8
        pygame.draw.rect(vg, (4, 2, 6, a), (i * 3, i * 3, W - i * 6, H - i * 6), width=6, border_radius=30)
    screen.blit(vg, (0, 0))

def main():
    pygame.init()
    screen = pygame.display.set_mode((W, H))
    pygame.display.set_caption("Paperbound")
    font = pygame.font.SysFont("georgia,dejavuserif", 18, bold=True)
    bfont = pygame.font.SysFont("georgia,dejavuserif", 24, bold=True)
    sfont = pygame.font.SysFont("georgia,dejavuserif", 15)
    fonts = (font, bfont, sfont)
    set_arena("study")

    if HEADLESS:
        _headless_shots(screen, fonts); return

    clock = pygame.time.Clock()
    window = pygame.display.set_mode((W, H), pygame.RESIZABLE)
    canvas = pygame.Surface((W, H))
    acc = 0.0; running = True; t = 0.0
    scene = "menu"; sel_arena = "study"; g = None
    flip = None                # page-flip transition {img, t, to}

    def win_to_canvas(pos):
        ww, wh = window.get_size(); sc = min(ww / W, wh / H)
        ox, oy = (ww - W * sc) / 2, (wh - H * sc) / 2
        return ((pos[0] - ox) / sc, (pos[1] - oy) / sc)

    def start_flip(to):
        nonlocal flip
        flip = {"img": canvas.copy(), "t": 0.0, "to": to}

    while running:
        dt = clock.tick(60) / 1000.0
        acc += dt; t += dt
        mouse = win_to_canvas(pygame.mouse.get_pos())
        for e in pygame.event.get():
            if e.type == pygame.QUIT: running = False
            elif e.type == pygame.MOUSEBUTTONDOWN and flip is None:
                m = win_to_canvas(e.pos)
                if scene == "menu":
                    if HOT.get("play") and HOT["play"].collidepoint(m):
                        set_arena(sel_arena); g = Game(boss=("angler" if sel_arena == "study" else None), deck=DECK[:]); scene = "battle"; start_flip("battle")
                    elif HOT.get("deck") and HOT["deck"].collidepoint(m): scene = "deck"; start_flip("deck")
                    elif HOT.get("arena") and HOT["arena"].collidepoint(m): scene = "arena"; start_flip("arena")
                    elif HOT.get("debug") and HOT["debug"].collidepoint(m): scene = "debug"; start_flip("debug")
                    elif HOT.get("quit") and HOT["quit"].collidepoint(m): running = False
                elif scene == "deck":
                    if HOT.get("back") and HOT["back"].collidepoint(m): scene = "menu"; start_flip("menu")
                    for r, key in HOT.get("cards", []):
                        if r.collidepoint(m):
                            if key in DECK: DECK.remove(key)
                            elif len(DECK) < 8: DECK.append(key)
                elif scene == "arena":
                    if HOT.get("back") and HOT["back"].collidepoint(m): scene = "menu"; start_flip("menu")
                    elif HOT.get("playarena") and HOT["playarena"].collidepoint(m):
                        set_arena(sel_arena); g = Game(boss=("angler" if sel_arena == "study" else None), deck=DECK[:]); scene = "battle"; start_flip("battle")
                    for r, key in HOT.get("arenas", []):
                        if r.collidepoint(m): sel_arena = key
                elif scene == "debug":
                    if HOT.get("back") and HOT["back"].collidepoint(m): scene = "menu"; start_flip("menu")
                    elif HOT.get("toggle") and HOT["toggle"].collidepoint(m):
                        globals()["DEBUG_ALL"] = not DEBUG_ALL
                elif scene == "battle":
                    if g.result:
                        scene = "menu"; start_flip("menu"); g = None
                    else:
                        if m[1] > HAND_Y - 45:
                            cwid, gp = 108, 14; x0 = W / 2 - (4 * cwid + 3 * gp) / 2
                            idx = int((m[0] - x0) // (cwid + gp))
                            if 0 <= idx < 4: g.selected = idx
                        elif g.selected is not None:
                            lx, ly = unproj(*m)
                            if BOOK.collidepoint(lx, ly):
                                g.try_play(g.selected, (lx, ly)); g.selected = None

        # sim
        if scene == "battle" and g:
            while acc >= DT: g.update(); acc -= DT
        else:
            acc = 0.0

        # draw current scene
        if scene == "menu": draw_menu(canvas, fonts, t, mouse)
        elif scene == "deck": draw_deckbuilder(canvas, fonts, t, mouse, 0)
        elif scene == "arena": draw_arena_select(canvas, fonts, t, mouse, sel_arena)
        elif scene == "debug": draw_debug(canvas, fonts, t, mouse)
        elif scene == "battle" and g: render(canvas, g, font, bfont, sfont)

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

        # letterbox to window
        ww, wh = window.get_size(); sc = min(ww / W, wh / H)
        sw, sh2 = int(W * sc), int(H * sc)
        window.fill((6, 4, 5))
        window.blit(canvas if (ww, wh) == (W, H) else pygame.transform.smoothscale(canvas, (sw, sh2)),
                    (0, 0) if (ww, wh) == (W, H) else ((ww - sw) // 2, (wh - sh2) // 2))
        pygame.display.flip()
    pygame.quit()

def _headless_shots(screen, fonts):
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
    render(screen, g, font, bfont, sfont)
    pygame.image.save(screen, "/mnt/user-data/outputs/paperbound_volcano.png")
    # menu + deck builder + arena select shots
    set_arena("study")
    draw_menu(screen, fonts, 0.0, (-1, -1))
    pygame.image.save(screen, "/mnt/user-data/outputs/paperbound_menu.png")
    global DEBUG_ALL
    draw_deckbuilder(screen, fonts, 0.0, (-1, -1), 0)
    pygame.image.save(screen, "/mnt/user-data/outputs/paperbound_deckbuilder.png")
    draw_arena_select(screen, fonts, 0.0, (-1, -1), "volcano")
    pygame.image.save(screen, "/mnt/user-data/outputs/paperbound_arenaselect.png")
    print("saved menu/deck/arena/volcano shots")

if __name__ == "__main__":
    main()
