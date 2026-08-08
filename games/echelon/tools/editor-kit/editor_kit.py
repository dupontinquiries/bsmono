"""editor-kit — the portfolio's little-tool shell, themed to Echelon.

The *idioms* came from paperbound (a framed workspace, hotkey chips,
one-key actions, a headless `--shot` path); the *style* is Echelon's:
every tool wears one of the three faction skins. Pick a theme, get a
console.

    Theme                     palette + faction chrome hooks
    THEMES = {genetron, xayid, keiron}
    Console(w, h, title, theme).frame(screen, subtitle) -> content Rect
    button(screen, theme, rect, label, font, mouse, enabled=True)
    chip(screen, theme, x, y, text, font, active=False) -> Rect
    GridPainter(rect, cols, rows)     cell picking + brush painting
    ron_layers.load/save              MapDef-style RON layer IO
    screenshot(screen, path)

No pygame side effects at import; call ek.init() once.
"""
import random
import re

import pygame


def shade(c, f):
    return tuple(max(0, min(255, int(v * f))) for v in c[:3])


def init():
    pygame.init()
    pygame.font.init()


def fonts():
    sans = "dejavusans,verdana"
    return {
        "title": pygame.font.SysFont(sans, 30, bold=True),
        "body": pygame.font.SysFont(sans, 15),
        "small": pygame.font.SysFont(sans, 12),
        "mono": pygame.font.SysFont("dejavusansmono,consolas", 13),
    }


class Theme:
    def __init__(self, name, **kw):
        self.name = name
        self.bg = kw["bg"]
        self.frame = kw["frame"]
        self.frame_hi = kw["frame_hi"]
        self.panel = kw["panel"]
        self.text = kw["text"]
        self.text_dim = kw["text_dim"]
        self.accent = kw["accent"]
        self.accent2 = kw["accent2"]
        self.btn = kw["btn"]
        self.radius = kw.get("radius", 6)
        self.backdrop = kw["backdrop"]
        self.chrome = kw["chrome"]


# ---------------------------------------------------------------- Genetron
def _gen_backdrop(s, w, h, t):
    s.fill(t.bg)
    for x in range(0, w, 132):  # hull panel seams + rivets
        pygame.draw.line(s, shade(t.bg, 1.35), (x, 0), (x, h), 1)
        pygame.draw.line(s, shade(t.bg, 0.6), (x + 1, 0), (x + 1, h), 1)
        for y in range(24, h, 96):
            pygame.draw.circle(s, shade(t.bg, 1.7), (x, y), 2)
    ov = pygame.Surface((w, h), pygame.SRCALPHA)  # faint scanlines
    for y in range(0, h, 4):
        pygame.draw.line(ov, (0, 0, 0, 22), (0, y), (w, y), 1)
    s.blit(ov, (0, 0))


def _gen_chrome(s, r, t):
    pygame.draw.rect(s, shade(t.bg, 0.55), r.inflate(26, 26).move(4, 6), border_radius=4)
    pygame.draw.rect(s, t.panel, r.inflate(18, 18), border_radius=3)
    pygame.draw.rect(s, t.frame, r.inflate(18, 18), 2, border_radius=3)
    for cx, cy in [(r.left - 4, r.top - 4), (r.right + 4, r.top - 4),
                   (r.left - 4, r.bottom + 4), (r.right + 4, r.bottom + 4)]:
        pygame.draw.circle(s, t.frame_hi, (cx, cy), 3)   # corner bolts
        pygame.draw.circle(s, shade(t.frame_hi, 0.5), (cx, cy), 3, 1)
    stripe = pygame.Rect(r.left, r.top - 30, r.w, 8)      # caution rail
    for i, x in enumerate(range(stripe.left, stripe.right, 16)):
        c = t.accent if i % 2 == 0 else shade(t.bg, 0.7)
        pygame.draw.polygon(s, c, [(x, stripe.bottom), (x + 8, stripe.top),
                                   (x + 16, stripe.top), (x + 8, stripe.bottom)])


# ------------------------------------------------------------------ Xayid
def _xay_backdrop(s, w, h, t):
    s.fill(t.bg)
    rng = random.Random(11)
    for _ in range(26):  # membrane blots under everything
        x, y = rng.randint(0, w), rng.randint(0, h)
        rr = rng.randint(30, 120)
        pygame.draw.ellipse(s, shade(t.bg, rng.uniform(0.8, 1.25)),
                            (x - rr, y - rr // 2, rr * 2, rr))
    for _ in range(60):  # spores
        x, y = rng.randint(0, w), rng.randint(0, h)
        pygame.draw.circle(s, shade(t.accent, 0.35), (x, y), rng.randint(1, 2))


def _xay_chrome(s, r, t):
    pygame.draw.rect(s, shade(t.bg, 0.5), r.inflate(30, 30).move(3, 7), border_radius=22)
    pygame.draw.rect(s, t.panel, r.inflate(22, 22), border_radius=18)
    pygame.draw.rect(s, t.frame, r.inflate(22, 22), 3, border_radius=18)
    rng = random.Random(5)
    fr = r.inflate(22, 22)
    for i in range(0, fr.w, 34):   # chitin plates along top/bottom lips
        bump = rng.randint(3, 6)
        pygame.draw.circle(s, t.frame_hi, (fr.left + i + 17, fr.top), bump, 2)
        pygame.draw.circle(s, t.frame_hi, (fr.left + i + 17, fr.bottom), bump, 2)


# ----------------------------------------------------------------- Keiron
def _kei_backdrop(s, w, h, t):
    s.fill(t.bg)
    cx, cy = w // 2, h // 2
    for rr in range(560, 60, -60):  # radial energy bloom
        k = 1.0 + (560 - rr) / 560 * 0.5
        pygame.draw.circle(s, shade(t.bg, k), (cx, cy), rr)
    rng = random.Random(7)
    for _ in range(46):  # light motes
        x, y = rng.randint(0, w), rng.randint(0, h)
        pygame.draw.circle(s, shade(t.accent2, rng.uniform(0.3, 0.8)), (x, y), 1)


def _kei_chrome(s, r, t):
    fr = r.inflate(20, 20)
    pygame.draw.rect(s, shade(t.bg, 1.35), fr, border_radius=2)
    pygame.draw.rect(s, t.panel, r.inflate(14, 14), border_radius=2)
    pygame.draw.rect(s, t.frame, fr, 1, border_radius=2)
    pygame.draw.rect(s, shade(t.frame, 0.55), fr.inflate(8, 8), 1, border_radius=2)
    cut = 14  # chamfered crystal corners
    for sx, sy, dx, dy in [(fr.left, fr.top, 1, 1), (fr.right, fr.top, -1, 1),
                           (fr.left, fr.bottom, 1, -1), (fr.right, fr.bottom, -1, -1)]:
        pygame.draw.polygon(s, t.frame_hi, [(sx, sy + dy * cut), (sx, sy), (sx + dx * cut, sy)], 2)
    pygame.draw.line(s, t.accent2, (fr.left - 6, r.top), (fr.left - 6, r.bottom), 1)


THEMES = {
    "genetron": Theme(
        "genetron",
        bg=(24, 26, 30), frame=(96, 102, 110), frame_hi=(158, 164, 172),
        panel=(34, 37, 42), text=(214, 218, 224), text_dim=(140, 146, 154),
        accent=(232, 164, 48), accent2=(120, 190, 235), btn=(52, 56, 63),
        radius=3, backdrop=_gen_backdrop, chrome=_gen_chrome,
    ),
    "xayid": Theme(
        "xayid",
        bg=(30, 22, 33), frame=(94, 66, 74), frame_hi=(140, 100, 104),
        panel=(40, 30, 42), text=(224, 212, 220), text_dim=(150, 132, 148),
        accent=(148, 214, 62), accent2=(196, 120, 255), btn=(58, 42, 58),
        radius=14, backdrop=_xay_backdrop, chrome=_xay_chrome,
    ),
    "keiron": Theme(
        "keiron",
        bg=(15, 17, 27), frame=(196, 168, 92), frame_hi=(238, 214, 140),
        panel=(24, 27, 40), text=(226, 228, 238), text_dim=(142, 148, 170),
        accent=(238, 214, 140), accent2=(96, 214, 230), btn=(33, 38, 58),
        radius=2, backdrop=_kei_backdrop, chrome=_kei_chrome,
    ),
}


class Console:
    """faction-skinned workspace frame; everything renders inside .page."""

    def __init__(self, w, h, title, theme):
        self.w, self.h, self.title, self.t = w, h, title, theme
        self.work = pygame.Rect(0, 0, int(w * 0.92), int(h * 0.88))
        self.work.center = (w // 2, h // 2 + 10)
        self.page = self.work.inflate(-28, -96)
        self.page.move_ip(0, 30)

    def frame(self, screen, subtitle=""):
        t = self.t
        t.backdrop(screen, self.w, self.h, t)
        t.chrome(screen, self.work, t)
        f = fonts()
        ti = f["title"].render(self.title, True, t.text)
        screen.blit(ti, (self.work.centerx - ti.get_width() / 2, self.work.top + 10))
        pygame.draw.line(
            screen, t.accent,
            (self.work.centerx - ti.get_width() / 2, self.work.top + 44),
            (self.work.centerx + ti.get_width() / 2, self.work.top + 44), 2)
        if subtitle:
            si = f["small"].render(subtitle, True, t.text_dim)
            screen.blit(si, (self.work.centerx - si.get_width() / 2, self.work.top + 50))
        return self.page


def button(screen, theme, rect, label, font, mouse, enabled=True):
    t = theme
    hover = rect.collidepoint(mouse) and enabled
    col = shade(t.btn, 1.35) if hover else (t.btn if enabled else shade(t.btn, 0.7))
    pygame.draw.rect(screen, shade(t.bg, 0.5), rect.move(2, 3), border_radius=t.radius)
    pygame.draw.rect(screen, col, rect, border_radius=t.radius)
    pygame.draw.rect(screen, t.accent if hover else t.frame, rect, 2, border_radius=t.radius)
    ti = font.render(label, True, t.text if enabled else t.text_dim)
    screen.blit(ti, (rect.centerx - ti.get_width() / 2, rect.centery - ti.get_height() / 2))
    return hover


def chip(screen, theme, x, y, text, font, active=False):
    t = theme
    ti = font.render(text, True, t.bg if active else t.text)
    r = pygame.Rect(x, y, ti.get_width() + 14, ti.get_height() + 8)
    base = t.accent if active else t.btn
    pygame.draw.rect(screen, base, r, border_radius=t.radius)
    pygame.draw.rect(screen, shade(base, 1.4), r, 1, border_radius=t.radius)
    screen.blit(ti, (r.x + 7, r.y + 4))
    return r


class GridPainter:
    """maps a char grid into a rect; handles hover + brush painting."""

    def __init__(self, rect, cols, rows):
        self.rect = rect
        self.cols, self.rows = cols, rows
        self.cell = max(2, min(rect.w // cols, rect.h // rows))
        self.ox = rect.x + (rect.w - self.cell * cols) // 2
        self.oy = rect.y + (rect.h - self.cell * rows) // 2

    def cell_at(self, mx, my):
        cx = (mx - self.ox) // self.cell
        cy = (my - self.oy) // self.cell
        if 0 <= cx < self.cols and 0 <= cy < self.rows:
            return int(cx), int(cy)
        return None

    def px(self, cx, cy):
        return self.ox + cx * self.cell, self.oy + cy * self.cell

    def paint(self, grid, cx, cy, ch, brush=1):
        r = brush - 1
        for y in range(cy - r, cy + r + 1):
            for x in range(cx - r, cx + r + 1):
                if 0 <= x < self.cols and 0 <= y < self.rows:
                    grid[y][x] = ch

    def draw(self, screen, grid, colorer, hover=None, brush=1, cursor=(255, 240, 200)):
        for y in range(self.rows):
            row = grid[y]
            for x in range(self.cols):
                c = colorer(x, y, row[x])
                if c is None:
                    continue
                px, py = self.px(x, y)
                pygame.draw.rect(screen, c, (px, py, self.cell, self.cell))
        if hover:
            hx, hy = hover
            r = brush - 1
            px, py = self.px(hx - r, hy - r)
            s = self.cell * (2 * r + 1)
            pygame.draw.rect(screen, cursor, (px, py, s, s), 1)


class ron_layers:
    """load/save the MapDef-style RON used by Echelon (string-array
    layers plus scalar metadata). Deliberately format-preserving-simple:
    good for tools, not a general RON parser."""

    LAYERS = ("height", "ramps", "bridges", "things")

    @staticmethod
    def load(path):
        s = open(path).read()
        meta = {}
        for k in ("name", "tileset"):
            m = re.search(k + r':\s*"([^"]*)"', s)
            meta[k] = m.group(1) if m else ""
        for k in ("alloy_amount", "flux_amount"):
            m = re.search(k + r":\s*(\d+)", s)
            meta[k] = int(m.group(1)) if m else 20000
        layers = {}
        for k in ron_layers.LAYERS:
            m = re.search(k + r":\s*\[(.*?)\]", s, re.S)
            layers[k] = (
                [list(r) for r in re.findall(r'"([^"]*)"', m.group(1))] if m else []
            )
        h = layers["height"]
        for k in ("ramps", "bridges", "things"):
            if not layers[k] and h:
                layers[k] = [["."] * len(h[0]) for _ in h]
        return meta, layers

    @staticmethod
    def save(path, meta, layers):
        q = lambda rows: ",\n".join('        "%s"' % "".join(r) for r in rows)
        with open(path, "w") as f:
            f.write(
                '(\n    name: "%s",\n    tileset: "%s",\n'
                "    alloy_amount: %d,\n    flux_amount: %d,\n"
                "    height: [\n%s,\n    ],\n    ramps: [\n%s,\n    ],\n"
                "    bridges: [\n%s,\n    ],\n    things: [\n%s,\n    ],\n)\n"
                % (
                    meta.get("name", "Untitled"),
                    meta.get("tileset", "relic_wastes"),
                    meta.get("alloy_amount", 20000),
                    meta.get("flux_amount", 20000),
                    q(layers["height"]),
                    q(layers["ramps"]),
                    q(layers["bridges"]),
                    q(layers["things"]),
                )
            )


def screenshot(screen, path):
    pygame.image.save(screen, path)
