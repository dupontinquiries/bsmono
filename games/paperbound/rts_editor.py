"""
Foldworks Map Editor — a standalone tool for authoring RTS maps.

Handles everything the RTS reads from rts_data.json for a map:
  • DATA      — name, size, seed, description, theme palette, weather mood
  • TERRAIN   — high-ground plateaus, ink pools, fold-valley bands, ink stains
  • MOGGLING  — placing objects/spawns: team spawns, ruins, obstacles, relics,
                rich ink-nodes, decorations (the "mob/model" layout of the map)
  • EVENTS    — a trigger→action script system (time / unit-count / region /
                structure-destroyed triggers; spawn / message / ink / reveal
                actions) that the RTS executes at runtime

Run:
    python rts_editor.py                 # open the GUI editor
    python rts_editor.py --selftest      # headless data round-trip test
    python rts_editor.py --shot out.png  # headless screenshot of the UI

Controls (GUI):
    Left toolbar   — pick a tool
    Left-click     — place / select on the canvas
    Right-drag / arrows / WASD — pan;  mouse-wheel — zoom
    Right panel    — edit fields of the map or the selected item (click, type,
                     Enter to commit;  Tab cycles preset fields)
    Bottom bar     — Events: add / edit / delete map event scripts
    Top bar        — choose map, New, Save, Export, Validate
    Delete / Backspace on a selected object removes it
"""

import os, sys, json, math, copy

HERE = os.path.dirname(os.path.abspath(__file__))
DATA_PATH = os.path.join(HERE, "rts_data.json")

# ─────────────────────────────────────────────────────────────────────────────
# Data model helpers
# ─────────────────────────────────────────────────────────────────────────────

# preset theme palettes the editor can cycle a map through
THEME_PRESETS = {
    "warm-basin":   dict(ground=[196,182,150], grid=[184,168,136], ruled=[150,175,140],
                         crease=[168,156,126], accent=[120,150,110], name_tint=[90,140,90]),
    "parchment":    dict(ground=[200,186,158], grid=[188,172,142], ruled=[205,150,120],
                         crease=[176,158,128], accent=[150,120,80], name_tint=[150,110,80]),
    "grey-spine":   dict(ground=[188,186,178], grid=[176,174,166], ruled=[150,160,190],
                         crease=[168,166,158], accent=[110,120,160], name_tint=[100,110,160]),
    "marsh-green":  dict(ground=[186,192,168], grid=[172,180,154], ruled=[140,170,150],
                         crease=[160,170,146], accent=[110,150,120], name_tint=[90,140,110]),
    "pale-linen":   dict(ground=[204,192,166], grid=[192,180,154], ruled=[190,160,140],
                         crease=[180,166,142], accent=[150,130,100], name_tint=[140,120,90]),
}
WEATHER_PRESETS = {
    "clear":    dict(kind="clear",    tint=[255,240,205], strength=0.12),
    "overcast": dict(kind="overcast", tint=[190,200,220], strength=0.18),
    "mist":     dict(kind="mist",     tint=[200,225,200], strength=0.16),
    "storm":    dict(kind="storm",    tint=[120,125,150], strength=0.30),
}

# object kinds you can place, with their default look
OBJECT_KINDS = {
    "ruin":       dict(r=70, blocks=True,  color=[110,100,86],  label="Folded Ruin",
                       desc="Impassable wreckage."),
    "obstacle":   dict(r=55, blocks=True,  color=[120,110,92],  label="Crease Barrier",
                       desc="Blocks movement."),
    "rock":       dict(r=48, blocks=True,  destructible=True, hp=600, color=[92,86,74],
                       label="Folded Debris", desc="Destructible blocker — clear it to open a path."),
    "watchtower": dict(r=46, blocks=False, watchtower=True, sight=560, color=[210,200,120],
                       label="Watch Perch", desc="Control it to reveal a wide area."),
    "relic":      dict(r=44, blocks=False, color=[210,180,90],  label="Ink Relic",
                       desc="Grants ink to whoever controls it."),
    "richnode":   dict(r=40, blocks=False, color=[120,108,190], label="Deep Ink Well",
                       amount=4000, desc="A rich ink source."),
    "decoration": dict(r=30, blocks=False, color=[160,150,130], label="Fold Mark",
                       desc="Cosmetic paper detail."),
}

# event trigger + action vocab (the schema the RTS runs)
TRIGGER_TYPES = ["time", "unit_count", "enter_region", "structure_destroyed"]
ACTION_TYPES  = ["spawn", "message", "ink", "reveal"]

# composable ability effect primitives (must match rts.EFFECT_PRIMITIVES) — each
# with its editable params; "target" chooses who it hits (enemy/ally/all/self)
EFFECT_PRIMITIVES = {
    "damage":    dict(amount=60, dot=False),
    "heal":      dict(amount=80, dot=True),
    "slow":      dict(duration=3.0),
    "stun":      dict(duration=1.5),
    "burn":      dict(dps=20, duration=3.0),
    "buff":      dict(stat="dmg", mult=1.3, duration=5.0),
    "shield":    dict(amount=100, duration=4.0),
    "shred":     dict(mult=1.4, duration=4.0),
    "drain":     dict(amount=50, dot=False),
    "knockback": dict(amount=140),
    "pull":      dict(amount=120),
    "spawn":     dict(unit="soldier", count=2),
    "reveal":    dict(),
    # ── splash primitives (engine: rts._run_effect) ──
    "splinter":  dict(count=6, range=110, amount=30, sub_radius=44),
    "inkpool":   dict(duration=6.0, dps=12, slow=2.0),
    "quake":     dict(amount=70, duration=1.2),
}

# delivery modes the engine understands ("rain" drops a staggered volley
# across the radius; rain_count / rain_radius tune it)
DELIVERY_MODES = ["area", "bolt", "chain", "rain"]

# projectile styles units/factions can wear ('projectile' on the spec)
PROJECTILE_STYLES = ["dart", "quill", "cinder", "inkglob", "gale",
                     "thorn", "sting"]


def new_map(map_id):
    return dict(
        name="New Map", size=[4000, 3000], seed=1, desc="A fresh battlefield.",
        spawns=[[400, 2600], [3600, 400]],
        theme=copy.deepcopy(THEME_PRESETS["parchment"]),
        weather=copy.deepcopy(WEATHER_PRESETS["clear"]),
        highground=[], pools=[], bands=[], stains=[], objects=[], nodes=3,
        regions=[], events=[],
    )


def load_data():
    with open(DATA_PATH) as f:
        return json.load(f)


def save_data(data):
    # keep a one-file backup before overwriting
    if os.path.exists(DATA_PATH):
        try:
            with open(DATA_PATH) as f:
                bak = f.read()
            with open(DATA_PATH + ".bak", "w") as f:
                f.write(bak)
        except Exception:
            pass
    with open(DATA_PATH, "w") as f:
        json.dump(data, f, indent=1)


def validate_map(m):
    """Return a list of human-readable problems (empty = valid)."""
    errs = []
    if not m.get("name"): errs.append("map has no name")
    if len(m.get("spawns", [])) < 2: errs.append("needs at least 2 spawns")
    w, h = m.get("size", [0, 0])
    if w < 1000 or h < 1000: errs.append("size looks too small")
    for i, sp in enumerate(m.get("spawns", [])):
        if not (0 <= sp[0] <= w and 0 <= sp[1] <= h):
            errs.append(f"spawn {i} is off the map")
    for o in m.get("objects", []):
        if o.get("kind") not in OBJECT_KINDS:
            errs.append(f"unknown object kind '{o.get('kind')}'")
    for e in m.get("events", []):
        if e.get("trigger", {}).get("type") not in TRIGGER_TYPES:
            errs.append("event with unknown trigger")
        for a in e.get("actions", []):
            if a.get("type") not in ACTION_TYPES:
                errs.append("event with unknown action")
    # pathing/connectivity: can a unit walk from spawn A to spawn B and to every
    # expansion, routing around IMPASSABLE blockers? (destructible rocks don't
    # count as permanent walls). This is the classic TL "is the map playable" check.
    reach = _reachable_from(m)
    if reach is not None:
        sp = m["spawns"]
        if not _cell_reachable(reach, m, sp[1][0], sp[1][1]):
            errs.append("spawn B is NOT reachable from spawn A (walled off)")
        for o in m.get("objects", []):
            if o.get("kind") in ("richnode", "relic", "watchtower"):
                if not _cell_reachable(reach, m, o["x"], o["y"]):
                    errs.append(f"{o.get('label', o['kind'])} is unreachable")
                    break
    return errs


def _reachable_from(m, cell=100):
    """Flood-fill a coarse grid from spawn A, blocked only by permanent
    (non-destructible) obstacles. Returns a 2-D bool grid or None."""
    sp = m.get("spawns", [])
    if len(sp) < 2:
        return None
    w, h = m["size"]
    cols = int(w // cell) + 1; rows = int(h // cell) + 1
    blocked = [[False] * cols for _ in range(rows)]
    for o in m.get("objects", []):
        if o.get("blocks") and not o.get("destructible"):
            cx = int(o["x"] // cell); cy = int(o["y"] // cell)
            rad = int(o.get("r", 40) // cell) + 1
            for gy in range(max(0, cy - rad), min(rows, cy + rad + 1)):
                for gx in range(max(0, cx - rad), min(cols, cx + rad + 1)):
                    if (gx - cx) ** 2 + (gy - cy) ** 2 <= rad * rad:
                        blocked[gy][gx] = True
    seen = [[False] * cols for _ in range(rows)]
    sx = min(cols - 1, int(sp[0][0] // cell)); sy = min(rows - 1, int(sp[0][1] // cell))
    stack = [(sx, sy)]
    while stack:
        x, y = stack.pop()
        if not (0 <= x < cols and 0 <= y < rows) or seen[y][x] or blocked[y][x]:
            continue
        seen[y][x] = True
        stack += [(x+1, y), (x-1, y), (x, y+1), (x, y-1)]
    return seen


def _cell_reachable(reach, m, x, y, cell=100):
    cols = len(reach[0]); rows = len(reach)
    gx = min(cols - 1, int(x // cell)); gy = min(rows - 1, int(y // cell))
    # accept if the target cell or any near neighbour is reachable
    for dy in range(-1, 2):
        for dx in range(-1, 2):
            nx, ny = gx + dx, gy + dy
            if 0 <= nx < cols and 0 <= ny < rows and reach[ny][nx]:
                return True
    return False


# ─────────────────────────────────────────────────────────────────────────────
# The editor (pygame GUI)
# ─────────────────────────────────────────────────────────────────────────────

def run_editor(headless_shot=None):
    import pygame
    pygame.init()
    W, H = 1280, 800
    if headless_shot:
        os.environ.setdefault("SDL_VIDEODRIVER", "dummy")
    screen = pygame.display.set_mode((W, H))
    pygame.display.set_caption("Foldworks Map Editor")
    font = pygame.font.SysFont("georgia", 16)
    sfont = pygame.font.SysFont("georgia", 13)
    bfont = pygame.font.SysFont("georgia", 26, bold=True)
    clock = pygame.time.Clock()

    ed = _EditorState(W, H, font, sfont, bfont)
    if headless_shot:
        ed.draw(screen)
        pygame.image.save(screen, headless_shot)
        pygame.quit()
        return

    running = True
    while running:
        dt = clock.tick(60) / 1000.0
        for ev in pygame.event.get():
            if ev.type == pygame.QUIT:
                running = False
            else:
                ed.handle(ev)
        ed.update(dt)
        ed.draw(screen)
        pygame.display.flip()
    pygame.quit()


# panel geometry
TOOLBAR_W = 140
PANEL_W = 300
TOPBAR_H = 44
BOTTOM_H = 150

TOOLS = [
    ("Select",      "select"),
    ("Base Layout", "base"),
    ("Spawn A",     "spawn0"),
    ("Spawn B",     "spawn1"),
    ("High Ground", "high"),
    ("Ink Pool",    "pool"),
    ("Ink Stain",   "stain"),
    ("Fold Band",   "band"),
    ("Ruin",        "ruin"),
    ("Obstacle",    "obstacle"),
    ("Rock",        "rock"),
    ("Watchtower",  "watchtower"),
    ("Relic",       "relic"),
    ("Rich Node",   "richnode"),
    ("Decoration",  "decoration"),
    ("Region",      "region"),
    ("Erase",       "erase"),
]


class _EditorState:
    def __init__(self, W, H, font, sfont, bfont):
        self.W, self.H = W, H
        self.font, self.sfont, self.bfont = font, sfont, bfont
        self.data = load_data()
        self.map_ids = list(self.data["maps"].keys())
        self.map_id = self.map_ids[0]
        self.m = self.data["maps"][self.map_id]
        self._ensure_fields(self.m)
        self.tool = "select"
        self.brush_r = 200           # radius for high/pool/stain
        self.sel = None              # selected item: (category, index)
        self.band_anchor = None      # first click of a band
        # competitive-melee mapmaking aids (SC2/TL tradition):
        self.symmetry = "rotate"     # none | mirror_x | mirror_y | rotate(180°)
        self.snap = 50               # grid snap in world units (0 = off)
        self.show_analysis = True    # balance overlay (rush dist, expansions)
        self.undo_stack = []
        self.redo_stack = []
        self.cam_x, self.cam_y = 0.0, 0.0
        self.zoom = 0.18
        self.pan = False; self.pan_last = (0, 0)
        self.status = "Loaded rts_data.json"
        self.status_t = 0.0
        self.active_field = None     # (owner, key) currently typed into
        self.pal_doc = None          # palette_scripts.json, loaded on first open
        self.pal_sel = 0             # selected script index in the palette window
        self.buffer = ""
        self.field_rects = []        # [(rect, owner, key)]
        self.tool_rects = []
        self.top_rects = []
        self.event_rects = []
        self.data_rects = []         # [(rect, (kind, val))] for the Data Editor
        self.editing_event = None    # dict being edited, or None
        self.param_form = False      # trigger/condition/action parameter overlay
        self.dragging = None         # (category, index) being dragged
        self.drag_off = (0, 0)       # world offset from item origin to cursor
        self.show_help = False       # F1 help overlay
        # floating window manager: draggable, closeable panes. Each window is a
        # dict {id, rect, drag_off}; z-order is list order (last = topmost).
        self.windows = []
        self.win_drag = None
        self.win_resize = None       # (window, mx0, my0, w0, h0) while resizing
        self.modal_off = [0, 0]      # draggable offset for the composer/param modals
        self.modal_drag = None       # (mx0, my0, ox0, oy0) while dragging a modal
        self.clipboard = None        # (category, deep-copied item) for copy/paste
        self.ctx_menu = None         # {x, y, items:[(label, action)], world:(wx,wy)}
        # touch state (tap = click, one-finger drag = pan/move, pinch = zoom)
        self.touches = {}
        self.pinch_d0 = None
        # optional card/origami access for model previews (falls back to tokens)
        self._cards = None; self._og = None
        try:
            import carddata, origami
            carddata.load()
            self._cards = getattr(carddata, "CARDS", None)
            # only use origami previews if its drawing helpers are bound
            self._og = origami if getattr(origami, "_DRAW", None) else None
        except Exception:
            pass
        # Data Editor (SC2 Data-Editor style): edit unit/structure/upgrade stats
        self.mode = "map"            # "map" | "data"
        self.data_fac = list(self.data["factions"].keys())[0]
        self.data_cat = "units"      # units | structures | upgrades
        self.data_sel = None         # selected entry key
        self.spell_edit = None       # ability dict open in the spell composer
        self._center_on_map()

    # ── map field defaults ──
    def _ensure_fields(self, m):
        for k, v in new_map(self.map_id).items():
            m.setdefault(k, v)
        # keep theme colours valid 3-int RGB (guards bad or externally-loaded values)
        th = m.get("theme")
        if isinstance(th, dict):
            for ck in ("ground", "grid", "ruled", "crease", "accent"):
                if ck in th:
                    th[ck] = list(self._safe_col(th[ck], (150, 140, 120)))

    def _center_on_map(self):
        w, h = self.m["size"]
        cw = self.W - TOOLBAR_W - PANEL_W
        ch = self.H - TOPBAR_H - BOTTOM_H
        self.zoom = min(cw / w, ch / h) * 0.92
        self.cam_x = w / 2 - cw / (2 * self.zoom)
        self.cam_y = h / 2 - ch / (2 * self.zoom)

    # ── competitive-map aids: snapping, symmetry, undo/redo ──
    def _snap(self, x, y):
        if self.snap:
            return round(x / self.snap) * self.snap, round(y / self.snap) * self.snap
        return round(x), round(y)

    def _mirror(self, x, y):
        """Return the symmetric counterpart of (x,y) under the current mode, or
        None if symmetry is off / the point is on the axis. Rotational (180°) is
        the standard for fair 1v1 melee maps."""
        w, h = self.m["size"]
        if self.symmetry == "none":
            return None
        if self.symmetry == "mirror_x":
            mx, my = w - x, y
        elif self.symmetry == "mirror_y":
            mx, my = x, h - y
        else:  # rotate 180°
            mx, my = w - x, h - y
        if abs(mx - x) < 1 and abs(my - y) < 1:
            return None
        return round(mx), round(my)

    def _snapshot(self):
        """Push a deep copy of the current map for undo (maps are small)."""
        self.undo_stack.append(json.dumps(self.m))
        if len(self.undo_stack) > 60:
            self.undo_stack.pop(0)
        self.redo_stack.clear()

    def _undo(self):
        if not self.undo_stack:
            self.flash("Nothing to undo"); return
        self.redo_stack.append(json.dumps(self.m))
        self.m.clear(); self.m.update(json.loads(self.undo_stack.pop()))
        self.data["maps"][self.map_id] = self.m
        self.sel = None; self.editing_event = None; self.flash("Undo")

    def _redo(self):
        if not self.redo_stack:
            self.flash("Nothing to redo"); return
        self.undo_stack.append(json.dumps(self.m))
        self.m.clear(); self.m.update(json.loads(self.redo_stack.pop()))
        self.data["maps"][self.map_id] = self.m
        self.sel = None; self.editing_event = None; self.flash("Redo")

    # ── coordinate transforms (canvas area) ──
    def cvx(self):  # canvas rect
        import pygame
        return pygame.Rect(TOOLBAR_W, TOPBAR_H, self.W - TOOLBAR_W - PANEL_W,
                           self.H - TOPBAR_H - BOTTOM_H)
    def w2s(self, x, y):
        c = self.cvx()
        return (c.x + (x - self.cam_x) * self.zoom, c.y + (y - self.cam_y) * self.zoom)
    def s2w(self, sx, sy):
        c = self.cvx()
        return ((sx - c.x) / self.zoom + self.cam_x, (sy - c.y) / self.zoom + self.cam_y)

    def flash(self, msg):
        self.status = msg; self.status_t = 3.0

    # ── input ──
    def handle(self, ev):
        import pygame
        if ev.type == pygame.KEYDOWN and self.active_field is not None:
            self._type_key(ev); return
        if ev.type == pygame.MOUSEBUTTONDOWN:
            self._mouse_down(ev)
        elif ev.type == pygame.MOUSEBUTTONUP:
            if ev.button == 3: self.pan = False
            if ev.button == 1 and self.dragging is not None:
                self.dragging = None
            if ev.button == 1:
                if self.win_drag is not None:
                    self._snap_window(self.win_drag[0])   # dock to edges on release
                self.win_drag = None; self.win_resize = None; self.modal_drag = None
        elif ev.type == pygame.MOUSEMOTION:
            if self.modal_drag is not None:
                mx0, my0, ox0, oy0 = self.modal_drag
                self.modal_off = [ox0 + (ev.pos[0] - mx0), oy0 + (ev.pos[1] - my0)]
            elif self.win_resize is not None:
                w, mx0, my0, w0, h0 = self.win_resize
                w["rect"].w = max(200, w0 + (ev.pos[0] - mx0))
                w["rect"].h = max(120, h0 + (ev.pos[1] - my0))
            elif self.win_drag is not None:
                w, ox, oy = self.win_drag
                w["rect"].x = ev.pos[0] - ox; w["rect"].y = max(26, ev.pos[1] - oy)
            elif self.pan:
                mx, my = ev.pos
                if self.ctx_menu is not None and (abs(mx - self.pan_last[0]) > 4 or abs(my - self.pan_last[1]) > 4):
                    self.ctx_menu = None      # dragging → panning, drop the menu
                self.cam_x -= (mx - self.pan_last[0]) / self.zoom
                self.cam_y -= (my - self.pan_last[1]) / self.zoom
                self.pan_last = (mx, my)
            elif self.dragging is not None:
                self._drag_to(*ev.pos)
        elif ev.type == pygame.MOUSEWHEEL:
            mx, my = pygame.mouse.get_pos()
            w = self._window_at(mx, my)
            if w is not None:                          # scroll the window under cursor
                w["scroll"] = max(0, w.get("scroll", 0) - ev.y * 40)
            elif self.cvx().collidepoint(mx, my):
                wx, wy = self.s2w(mx, my)
                self.zoom = max(0.05, min(1.2, self.zoom * (1.1 if ev.y > 0 else 0.9)))
                nx, ny = self.s2w(mx, my)
                self.cam_x += wx - nx; self.cam_y += wy - ny
        elif ev.type in (pygame.FINGERDOWN, pygame.FINGERMOTION, pygame.FINGERUP):
            self._touch(ev)
        elif ev.type == pygame.KEYDOWN:
            self._key(ev)

    def _snap_window(self, w):
        """Dock a window to a screen edge if its title bar is released near one."""
        import pygame
        r = w["rect"]; margin = 24
        if r.x < margin: r.x = 4
        elif r.right > self.W - margin: r.x = self.W - r.w - 4
        if r.y < TOPBAR_H + margin: r.y = TOPBAR_H + 26
        elif r.bottom > self.H - margin: r.y = self.H - r.h - 4

    def _touch(self, ev):
        """Map touch to editor actions: tap=click, 1-finger drag=pan/move,
        2-finger pinch=zoom."""
        import pygame
        fid = getattr(ev, "finger_id", 0)
        x = int(ev.x * self.W); y = int(ev.y * self.H)
        if ev.type == pygame.FINGERDOWN:
            self.touches[fid] = (x, y)
            if len(self.touches) == 1:
                # synthesize a left mouse-down for taps/UI/drag-start
                self._mouse_down(type("E", (), dict(pos=(x, y), button=1))())
                self._touch_last = (x, y)
            elif len(self.touches) == 2:
                pts = list(self.touches.values())
                self.pinch_d0 = math.hypot(pts[0][0] - pts[1][0], pts[0][1] - pts[1][1])
                self.pan = False; self.win_drag = None; self.dragging = None
        elif ev.type == pygame.FINGERMOTION:
            self.touches[fid] = (x, y)
            if len(self.touches) >= 2 and self.pinch_d0:
                pts = list(self.touches.values())
                d = math.hypot(pts[0][0] - pts[1][0], pts[0][1] - pts[1][1])
                cx = (pts[0][0] + pts[1][0]) // 2; cy = (pts[0][1] + pts[1][1]) // 2
                if self.cvx().collidepoint(cx, cy) and self.pinch_d0 > 1:
                    wx, wy = self.s2w(cx, cy)
                    self.zoom = max(0.05, min(1.2, self.zoom * (d / self.pinch_d0)))
                    nx, ny = self.s2w(cx, cy)
                    self.cam_x += wx - nx; self.cam_y += wy - ny
                self.pinch_d0 = d
            elif len(self.touches) == 1:
                # drag: reuse motion routing (window move / pan / object drag)
                if self.win_drag is not None:
                    w, ox, oy = self.win_drag
                    w["rect"].x = x - ox; w["rect"].y = y - oy
                elif self.dragging is not None:
                    self._drag_to(x, y)
                elif self.cvx().collidepoint(x, y) and self.tool == "select":
                    last = getattr(self, "_touch_last", (x, y))
                    self.cam_x -= (x - last[0]) / self.zoom
                    self.cam_y -= (y - last[1]) / self.zoom
                self._touch_last = (x, y)
        elif ev.type == pygame.FINGERUP:
            self.touches.pop(fid, None)
            if len(self.touches) < 2: self.pinch_d0 = None
            if not self.touches:
                self.win_drag = None; self.dragging = None; self.pan = False

    def _key(self, ev):
        import pygame
        if ev.key == pygame.K_F1:
            self.show_help = not self.show_help; return
        if ev.key == pygame.K_F5:
            self._launch_test(); return
        mods = pygame.key.get_mods()
        if mods & pygame.KMOD_CTRL:
            if ev.key == pygame.K_z: self._undo(); return
            if ev.key in (pygame.K_y, pygame.K_r): self._redo(); return
            if ev.key == pygame.K_c: self._copy_sel(); return
            if ev.key == pygame.K_v: self._paste(); return
            if ev.key == pygame.K_d: self._duplicate_sel(); return
        # Shift+arrows nudge the selected object; plain arrows/WASD pan the camera
        if (mods & pygame.KMOD_SHIFT) and self.sel and \
                ev.key in (pygame.K_LEFT, pygame.K_RIGHT, pygame.K_UP, pygame.K_DOWN):
            d = {pygame.K_LEFT: (-1, 0), pygame.K_RIGHT: (1, 0),
                 pygame.K_UP: (0, -1), pygame.K_DOWN: (0, 1)}[ev.key]
            self._snapshot(); self._nudge_sel(*d); return
        step = 60 / self.zoom
        if ev.key in (pygame.K_LEFT, pygame.K_a): self.cam_x -= step
        elif ev.key in (pygame.K_RIGHT, pygame.K_d): self.cam_x += step
        elif ev.key in (pygame.K_UP, pygame.K_w): self.cam_y -= step
        elif ev.key in (pygame.K_DOWN, pygame.K_s): self.cam_y += step
        elif ev.key in (pygame.K_DELETE, pygame.K_BACKSPACE):
            self._snapshot(); self._delete_sel()
        elif ev.key == pygame.K_LEFTBRACKET: self.brush_r = max(20, self.brush_r - 20)
        elif ev.key == pygame.K_RIGHTBRACKET: self.brush_r = min(600, self.brush_r + 20)
        elif ev.key == pygame.K_m:            # cycle symmetry
            modes = ["none", "rotate", "mirror_x", "mirror_y"]
            self.symmetry = modes[(modes.index(self.symmetry) + 1) % len(modes)]
            self.flash(f"Symmetry: {self.symmetry}")
        elif ev.key == pygame.K_g:            # toggle grid snap
            self.snap = 0 if self.snap else 50
            self.flash(f"Snap: {'off' if not self.snap else self.snap}")

    def _mouse_down(self, ev):
        import pygame
        mx, my = ev.pos
        # a click anywhere dismisses an open context menu first
        if self.ctx_menu is not None:
            for rect, action in self.ctx_menu.get("_hot", []):
                if rect.collidepoint(mx, my):
                    self.ctx_menu = None
                    self._ctx_action(action); return
            self.ctx_menu = None
            if ev.button == 1: return
        # right button → context menu over the canvas (map mode), else pan
        if ev.button == 3:
            if self.mode != "data" and self.cvx().collidepoint(mx, my) \
                    and self._window_at(mx, my) is None:
                self._open_ctx_menu(mx, my)
            self.pan = True; self.pan_last = (mx, my); return
        if ev.button != 1:
            return
        # a centred modal (spell composer / param form) grabs its title bar first
        if (self.spell_edit is not None or (self.param_form and self.editing_event)) \
                and getattr(self, "_modal_bar", None) and self._modal_bar.collidepoint(mx, my):
            self.modal_drag = (mx, my, self.modal_off[0], self.modal_off[1]); return
        # floating windows intercept clicks first (topmost wins)
        w = self._window_at(mx, my)
        if w is not None:
            self.windows.remove(w); self.windows.append(w)   # focus
            if w.get("_close") and w["_close"].collidepoint(mx, my):
                self._close_window(w["id"]); return
            if w.get("_resize") and w["_resize"].collidepoint(mx, my):
                self.win_resize = (w, mx, my, w["rect"].w, w["rect"].h); return
            tb = pygame.Rect(w["rect"].x, w["rect"].y - 26, w["rect"].w, 26)
            if tb.collidepoint(mx, my):
                self.win_drag = (w, mx - w["rect"].x, my - w["rect"].y); return
            self._win_click(w, mx, my); return
        # top bar always available (holds the Mode toggle + window launchers)
        for rect, kind in self.top_rects:
            if rect.collidepoint(mx, my):
                self._top_action(kind); return
        # Data Editor mode: only data tabs + stat fields are interactive
        if self.mode == "data":
            for rect, owner, key in self.field_rects:
                if rect.collidepoint(mx, my):
                    self._begin_field(owner, key); return
            self._commit_field()
            self._data_click(mx, my)
            return
        # UI regions first
        for rect, tid in self.tool_rects:
            if rect.collidepoint(mx, my):
                self.tool = tid; self.sel = None; self.band_anchor = None; return
        for rect, owner, key in self.field_rects:
            if rect.collidepoint(mx, my):
                self._begin_field(owner, key); return
        for rect, act in self.event_rects:
            if rect.collidepoint(mx, my):
                self._event_action(act); return
        # else commit any active field and act on the canvas
        self._commit_field()
        if self.cvx().collidepoint(mx, my):
            self._canvas_click(mx, my)

    def _canvas_click(self, mx, my):
        rawx, rawy = self.s2w(mx, my)
        wx, wy = self._snap(rawx, rawy)
        t = self.tool
        if t == "select":
            self.sel = self._pick(rawx, rawy)
            if self.sel:
                # begin a drag: record cursor offset from the item's origin
                ox, oy = self._item_pos(self.sel)
                self.drag_off = (rawx - ox, rawy - oy)
                self.dragging = self.sel
                self._snapshot()
            return
        if t == "erase":
            hit = self._pick(rawx, rawy)
            if hit:
                self._snapshot(); self.sel = hit; self._delete_sel()
            return
        # everything below mutates → snapshot for undo
        self._snapshot()
        mirror = self._mirror(wx, wy)
        if t == "base":
            # drop a starting base: a spawn + a small arc of ink wells (a mineral
            # line) — the classic main-base template. Symmetry mirrors the pair.
            def lay(bx, by, which):
                if which == 0: self.m["spawns"][0] = [bx, by]
                else:
                    if len(self.m["spawns"]) < 2: self.m["spawns"].append([bx, by])
                    else: self.m["spawns"][1] = [bx, by]
                for k in range(3):
                    a = -0.6 + k * 0.6
                    self.m["objects"].append(dict(
                        kind="richnode", x=round(bx + math.cos(a) * 220),
                        y=round(by + math.sin(a) * 220 - 40), r=40,
                        blocks=False, color=[120, 108, 190], amount=4000,
                        label="Ink Well"))
            lay(wx, wy, 0)
            if mirror: lay(mirror[0], mirror[1], 1)
            self.flash("Placed base layout" + (" (+mirror)" if mirror else ""))
            return
        if t == "spawn0":
            self.m["spawns"][0] = [wx, wy]
            if mirror and len(self.m["spawns"]) >= 2:
                self.m["spawns"][1] = list(mirror)      # keep spawns symmetric
            self.flash("Moved Spawn A" + (" (+mirror B)" if mirror else "")); return
        if t == "spawn1":
            if len(self.m["spawns"]) < 2: self.m["spawns"].append([wx, wy])
            else: self.m["spawns"][1] = [wx, wy]
            self.flash("Moved Spawn B"); return
        if t == "high":
            self._place_sym("highground", [wx, wy, self.brush_r], mirror); return
        if t == "pool":
            self._place_sym("pools", [wx, wy, self.brush_r], mirror); return
        if t == "stain":
            self._place_sym("stains", [wx, wy, self.brush_r * 0.2, 0.12], mirror); return
        if t == "band":
            if self.band_anchor is None:
                self.band_anchor = (wx, wy)
            else:
                x1, y1 = self.band_anchor
                self.m["bands"].append([x1, y1, wx, wy, 100])
                if mirror:
                    m1 = self._mirror(x1, y1)
                    if m1: self.m["bands"].append([m1[0], m1[1], mirror[0], mirror[1], 100])
                self.band_anchor = None
            return
        if t == "region":
            regs = self.m.setdefault("regions", [])
            regs.append(dict(name=f"region{len(regs)+1}", x=wx, y=wy, r=self.brush_r))
            if mirror:
                regs.append(dict(name=f"region{len(regs)+1}", x=mirror[0], y=mirror[1], r=self.brush_r))
            self.flash("Placed region"); return
        if t in OBJECT_KINDS:
            spec = dict(kind=t, x=wx, y=wy); spec.update(copy.deepcopy(OBJECT_KINDS[t]))
            self.m["objects"].append(spec)
            if mirror:
                m2 = dict(kind=t, x=mirror[0], y=mirror[1]); m2.update(copy.deepcopy(OBJECT_KINDS[t]))
                self.m["objects"].append(m2)
            self.flash(f"Placed {t}" + (" (+mirror)" if mirror else "")); return

    def _place_sym(self, cat, entry, mirror):
        self.m[cat].append(entry)
        if mirror:
            me = list(entry); me[0], me[1] = mirror[0], mirror[1]
            self.m[cat].append(me)

    def _sel_item(self):
        """Return the currently-selected (category, index, item), or None if the
        selection is stale (its list shrank / item was removed). Clears a stale
        selection so it can't crash a later draw or drag."""
        if not self.sel:
            return None
        cat, i = self.sel
        lst = self.m.get(cat)
        if not isinstance(lst, list) or not (0 <= i < len(lst)):
            self.sel = None
            return None
        return cat, i, lst[i]

    def _item_pos(self, sel):
        cat, i = sel
        lst = self.m.get(cat, [])
        if not (0 <= i < len(lst)):
            return 0, 0
        it = lst[i]
        if cat == "objects": return it["x"], it["y"]
        if cat == "regions": return it["x"], it["y"]
        if cat == "spawns": return it[0], it[1]
        return it[0], it[1]                 # highground / pools / stains: [x,y,...]

    def _drag_to(self, mx, my):
        """Move the dragged item to follow the cursor (snapped)."""
        if not self.dragging:
            return
        cat, i = self.dragging
        if i >= len(self.m.get(cat, [])):
            self.dragging = None; return
        rawx, rawy = self.s2w(mx, my)
        nx, ny = self._snap(rawx - self.drag_off[0], rawy - self.drag_off[1])
        it = self.m[cat][i]
        if cat in ("objects", "regions"):
            it["x"], it["y"] = nx, ny
        else:
            it[0], it[1] = nx, ny
        self.sel = self.dragging

    def _pick(self, wx, wy):
        """Find the topmost item under the cursor → (category, index)."""
        def near(x, y, rr): return (wx - x) ** 2 + (wy - y) ** 2 <= rr * rr
        for i, o in enumerate(self.m["objects"]):
            if near(o["x"], o["y"], o.get("r", 40)): return ("objects", i)
        for i, rg in enumerate(self.m.get("regions", [])):
            if near(rg["x"], rg["y"], rg["r"]): return ("regions", i)
        for i, (x, y, r) in enumerate(self.m["highground"]):
            if near(x, y, r): return ("highground", i)
        for i, (x, y, r) in enumerate(self.m["pools"]):
            if near(x, y, r): return ("pools", i)
        for i, sp in enumerate(self.m["spawns"]):
            if near(sp[0], sp[1], 120): return ("spawns", i)
        for i, st in enumerate(self.m["stains"]):
            if near(st[0], st[1], st[2]): return ("stains", i)
        return None

    def _delete_sel(self):
        if not self.sel: return
        cat, i = self.sel
        if cat == "spawns":
            self.flash("Can't delete a spawn (need 2)"); return
        try:
            del self.m[cat][i]; self.sel = None; self.flash("Deleted")
        except Exception:
            pass

    def _copy_sel(self):
        """Copy the selected item (objects/regions/terrain) to the clipboard."""
        sel = self._sel_item()
        if not sel:
            self.flash("Nothing to copy"); return
        cat, i, it = sel
        if cat == "spawns":
            self.flash("Can't copy a spawn"); return
        self.clipboard = (cat, copy.deepcopy(it))
        self.flash(f"Copied {cat[:-1] if cat.endswith('s') else cat}")

    def _paste(self, at=None):
        """Paste the clipboard item, offset so it doesn't sit exactly on the
        original. Optionally place it at a world point (`at`). Selects the copy."""
        if not self.clipboard:
            self.flash("Clipboard empty"); return
        cat, item = self.clipboard
        self._snapshot()
        new = copy.deepcopy(item)
        dx, dy = 80, 80
        if cat in ("objects", "regions"):
            if at: new["x"], new["y"] = self._snap(*at)
            else: new["x"] = new.get("x", 0) + dx; new["y"] = new.get("y", 0) + dy
        else:                                   # highground / pools / stains → [x,y,r,...]
            new = list(new)
            if at: new[0], new[1] = self._snap(*at)
            else: new[0] += dx; new[1] += dy
        self.m.setdefault(cat, []).append(new)
        self.sel = (cat, len(self.m[cat]) - 1)
        self.flash("Pasted")

    def _duplicate_sel(self):
        """Duplicate the selected item in place (copy + paste with offset)."""
        self._copy_sel()
        if self.clipboard:
            self._paste()

    def _nudge_sel(self, dx, dy):
        """Move the selected item by a small step (arrow keys)."""
        sel = self._sel_item()
        if not sel: return
        cat, i, it = sel
        step = self.snap or 20
        if cat in ("objects", "regions"):
            it["x"] += dx * step; it["y"] += dy * step
        elif cat == "spawns" or isinstance(it, list):
            it[0] += dx * step; it[1] += dy * step

    def _open_ctx_menu(self, mx, my):
        """Right-click menu on the canvas: copy/paste/duplicate/delete, targeting
        whatever object sits under the cursor."""
        rawx, rawy = self.s2w(mx, my)
        hit = self._pick(rawx, rawy)
        if hit:
            self.sel = hit
        items = []
        if hit and hit[0] != "spawns":
            items += [("Copy", "copy"), ("Duplicate", "duplicate"), ("Delete", "delete")]
        elif hit:
            items += [("(spawn — can't copy)", "noop")]
        if self.clipboard:
            items.append(("Paste here", "paste"))
        if not items:
            items = [("Nothing here", "noop")]
        self.ctx_menu = dict(x=mx, y=my, items=items, world=(rawx, rawy))

    def _ctx_action(self, action):
        if action == "copy": self._copy_sel()
        elif action == "duplicate": self._duplicate_sel()
        elif action == "delete": self._snapshot(); self._delete_sel()
        elif action == "paste":
            at = self.ctx_menu["world"] if self.ctx_menu else None
            self._paste(at=at)

    def _draw_ctx_menu(self, screen):
        import pygame
        cm = self.ctx_menu
        if not cm: return
        cm["_hot"] = []
        rows = cm["items"]
        w = 150; rh = 24; h = rh * len(rows) + 6
        x = min(cm["x"], self.W - w - 4); y = min(cm["y"], self.H - h - 4)
        pygame.draw.rect(screen, (46, 50, 60), (x, y, w, h), border_radius=6)
        pygame.draw.rect(screen, (150, 190, 220), (x, y, w, h), 1, border_radius=6)
        for i, (label, action) in enumerate(rows):
            rect = pygame.Rect(x + 3, y + 3 + i * rh, w - 6, rh - 2)
            hover = rect.collidepoint(pygame.mouse.get_pos())
            if hover and action != "noop":
                pygame.draw.rect(screen, (64, 72, 88), rect, border_radius=4)
            col = (150, 160, 170) if action == "noop" else (228, 234, 240)
            screen.blit(self.sfont.render(label, True, col), (rect.x + 8, rect.y + 4))
            cm["_hot"].append((rect, action))

    # ── command-palette script editing ────────────────────────────────────
    # The in-match command palette (press "/" in the RTS) runs user scripts —
    # sequences of the same safe ops the executor understands. This window
    # authors palette_scripts.json: add/rename scripts, add ops, click an op's
    # name to cycle its type, click any parameter to type a new value.
    _PAL_OPS = [
        ("build",    {"kind": "well"}),
        ("train",    {"unit": "scribe", "count": 1}),
        ("research", {"upgrade": ""}),
        ("select",   {"what": "army"}),
        ("group",    {"n": 1, "do": "assign"}),
        ("camera",   {"what": "base"}),
        ("stop",     {}),
        ("hold",     {}),
        ("fog",      {}),
        ("flash",    {"text": "…"}),
    ]

    def _pal_path(self):
        import os
        return os.path.join(os.path.dirname(os.path.abspath(__file__)),
                            "palette_scripts.json")

    def _pal_load(self):
        import json
        if self.pal_doc is not None:
            return
        try:
            with open(self._pal_path()) as f:
                self.pal_doc = json.load(f)
            self.pal_doc.setdefault("scripts", [])
        except Exception:
            self.pal_doc = {"scripts": []}

    def _pal_save(self):
        import json, os
        tmp = self._pal_path() + ".tmp"
        with open(tmp, "w") as f:
            json.dump(self.pal_doc, f, indent=1)
        os.replace(tmp, self._pal_path())
        self.flash(f"Saved {len(self.pal_doc['scripts'])} palette scripts")

    def _win_palette(self, screen, w):
        import pygame
        self._pal_load()
        r = w["rect"]; w["_hot"] = []
        oy = self._win_oy(w)
        scripts = self.pal_doc["scripts"]
        y = r.y + 8 - oy
        # toolbar
        for label, tag in (("+ New script", ("pal", ("new", None))),
                           ("Save", ("pal", ("save", None)))):
            tw = self.sfont.size(label)[0] + 18
            rect = pygame.Rect(r.x + 10 if label.startswith("+") else r.right - tw - 10,
                               y, tw, 24)
            pygame.draw.rect(screen, (52, 62, 52), rect, border_radius=5)
            pygame.draw.rect(screen, (120, 170, 120), rect, 1, border_radius=5)
            screen.blit(self.sfont.render(label, True, (222, 236, 222)),
                        (rect.x + 9, rect.y + 5))
            w["_hot"].append((rect, tag))
        y += 32
        # script list
        for i, sc in enumerate(scripts):
            hot = (i == self.pal_sel)
            row = pygame.Rect(r.x + 8, y, r.w - 16, 24)
            pygame.draw.rect(screen, (58, 54, 44) if hot else (46, 48, 56), row,
                             border_radius=5)
            if hot:
                pygame.draw.rect(screen, (200, 170, 90), row, 1, border_radius=5)
            editing = self.active_field == (sc, "name")
            name = (self.buffer + "▏") if editing else sc.get("name", "?")
            screen.blit(self.sfont.render(name, True, (232, 226, 208)),
                        (row.x + 8, row.y + 4))
            nops = self.sfont.render(f"{len(sc.get('ops', []))} ops", True, (160, 152, 138))
            screen.blit(nops, (row.right - 92, row.y + 4))
            del_r = pygame.Rect(row.right - 24, row.y + 3, 18, 18)
            pygame.draw.rect(screen, (92, 52, 48), del_r, border_radius=4)
            screen.blit(self.sfont.render("×", True, (240, 200, 190)),
                        (del_r.x + 5, del_r.y + 1))
            w["_hot"].append((del_r, ("pal", ("del", i))))
            w["_hot"].append((pygame.Rect(row.x, row.y, row.w - 100, row.h),
                              ("pal", ("sel", i))))
            y += 28
        # selected script's ops
        if 0 <= self.pal_sel < len(scripts):
            sc = scripts[self.pal_sel]
            pygame.draw.line(screen, (88, 92, 104), (r.x + 8, y + 4),
                             (r.right - 8, y + 4), 1)
            y += 12
            screen.blit(self.sfont.render(
                "ops — click a name to cycle its type, click a value to edit",
                True, (150, 156, 168)), (r.x + 10, y))
            y += 22
            for j, op in enumerate(sc.setdefault("ops", [])):
                name_r = pygame.Rect(r.x + 10, y, 84, 22)
                pygame.draw.rect(screen, (50, 58, 74), name_r, border_radius=4)
                screen.blit(self.sfont.render(op.get("op", "?"), True, (210, 222, 240)),
                            (name_r.x + 7, name_r.y + 4))
                w["_hot"].append((name_r, ("pal", ("opcycle", j))))
                x = name_r.right + 8
                for pk, pv in list(op.items()):
                    if pk == "op":
                        continue
                    editing = self.active_field == (op, pk)
                    txt = f"{pk}={self.buffer + chr(9615) if editing else pv}"
                    tw = self.sfont.size(txt)[0] + 12
                    pr = pygame.Rect(x, y, tw, 22)
                    pygame.draw.rect(screen, (60, 56, 48) if not editing else (78, 68, 44),
                                     pr, border_radius=4)
                    screen.blit(self.sfont.render(txt, True, (226, 220, 204)),
                                (pr.x + 6, pr.y + 4))
                    w["_hot"].append((pr, ("pal", ("field", (j, pk)))))
                    x = pr.right + 6
                del_r = pygame.Rect(r.right - 30, y + 2, 18, 18)
                pygame.draw.rect(screen, (92, 52, 48), del_r, border_radius=4)
                screen.blit(self.sfont.render("×", True, (240, 200, 190)),
                            (del_r.x + 5, del_r.y + 1))
                w["_hot"].append((del_r, ("pal", ("opdel", j))))
                y += 26
            add_r = pygame.Rect(r.x + 10, y + 2, 70, 22)
            pygame.draw.rect(screen, (52, 62, 52), add_r, border_radius=4)
            screen.blit(self.sfont.render("+ op", True, (210, 232, 210)),
                        (add_r.x + 12, add_r.y + 4))
            w["_hot"].append((add_r, ("pal", ("opadd", None))))

    def _pal_action(self, tag):
        act, val = tag
        scripts = self.pal_doc["scripts"]
        if act == "new":
            scripts.append({"name": f"Script {len(scripts) + 1}", "desc": "",
                            "ops": [{"op": "flash", "text": "hello"}]})
            self.pal_sel = len(scripts) - 1
        elif act == "save":
            self._pal_save()
        elif act == "sel":
            self.pal_sel = val
            self._begin_field(scripts[val], "name")
        elif act == "del":
            if 0 <= val < len(scripts):
                scripts.pop(val)
                self.pal_sel = max(0, min(self.pal_sel, len(scripts) - 1))
        elif act == "opadd" and 0 <= self.pal_sel < len(scripts):
            name, params = self._PAL_OPS[0]
            scripts[self.pal_sel]["ops"].append({"op": name, **params})
        elif act == "opdel" and 0 <= self.pal_sel < len(scripts):
            ops = scripts[self.pal_sel]["ops"]
            if 0 <= val < len(ops):
                ops.pop(val)
        elif act == "opcycle" and 0 <= self.pal_sel < len(scripts):
            ops = scripts[self.pal_sel]["ops"]
            if 0 <= val < len(ops):
                names = [n for n, _ in self._PAL_OPS]
                cur = ops[val].get("op", names[0])
                nxt = names[(names.index(cur) + 1) % len(names)] \
                    if cur in names else names[0]
                _, params = next(p for p in self._PAL_OPS if p[0] == nxt)
                ops[val] = {"op": nxt, **params}
        elif act == "field" and 0 <= self.pal_sel < len(scripts):
            j, pk = val
            ops = scripts[self.pal_sel]["ops"]
            if 0 <= j < len(ops):
                self._begin_field(ops[j], pk)

    # ── text field editing ──
    def _begin_field(self, owner, key):
        self.active_field = (owner, key)
        if key == "__newfield__":
            self.buffer = ""; return
        cur = owner.get(key, "") if isinstance(owner, dict) else ""
        if isinstance(cur, list):
            self.buffer = ", ".join(str(x) for x in cur)
        else:
            self.buffer = str(cur)

    def _commit_field(self):
        if self.active_field is None: return
        owner, key = self.active_field
        val = self.buffer.strip()
        old = owner.get(key)
        try:
            if key == "__newfield__":
                # add a brand-new field (ability, cloak, worker, detector, …)
                nk = val.strip()
                if nk:
                    if nk == "ability":
                        owner[nk] = _default_ability()
                        owner.setdefault("max_energy", 100); owner.setdefault("energy", 50)
                    elif nk in ("worker", "cloak", "detector"):
                        owner[nk] = True
                    else:
                        owner[nk] = 0
                    self.flash(f"Added field '{nk}'")
            elif key == "size":
                parts = [int(p) for p in val.replace("×", "x").split("x")]
                owner[key] = parts[:2] if len(parts) >= 2 else old
            elif key == "seed":
                owner[key] = int(val)
            elif key == "strength":
                owner[key] = float(val)
            elif isinstance(old, bool):
                owner[key] = val.lower() in ("true", "1", "yes", "on")
            elif isinstance(old, list):
                # comma-separated; numeric if the elements look numeric
                items = [s.strip() for s in val.split(",") if s.strip()]
                def conv(s):
                    try: return int(s) if s.lstrip("-").isdigit() else float(s)
                    except ValueError: return s
                new = [conv(s) for s in items]
                # colour fields must stay valid RGB — pad/clamp to 3 ints [0,255]
                if key in ("ground", "grid", "ruled", "crease", "accent", "color", "tint"):
                    nums = [int(x) for x in new if isinstance(x, (int, float))]
                    while len(nums) < 3: nums.append(nums[-1] if nums else 128)
                    new = [max(0, min(255, n)) for n in nums[:3]]
                owner[key] = new
            elif isinstance(old, (int, float)) and key not in ("name", "desc", "text"):
                owner[key] = type(old)(float(val)) if "." in val else int(val)
            else:
                owner[key] = val
        except Exception:
            self.flash(f"Bad value for {key}")
        self.active_field = None; self.buffer = ""

    def _safe_col(self, col, fallback=(150, 150, 150)):
        """Coerce any stored value into a valid RGB tuple for pygame."""
        try:
            nums = [max(0, min(255, int(x))) for x in col]
            if len(nums) >= 3:
                return tuple(nums[:3])
        except Exception:
            pass
        return fallback

    def _type_key(self, ev):
        import pygame
        if ev.key in (pygame.K_RETURN, pygame.K_KP_ENTER):
            self._commit_field()
        elif ev.key == pygame.K_ESCAPE:
            self.active_field = None; self.buffer = ""
        elif ev.key == pygame.K_BACKSPACE:
            self.buffer = self.buffer[:-1]
        else:
            if ev.unicode and ev.unicode.isprintable():
                self.buffer += ev.unicode

    # ── top-bar actions ──
    def _top_action(self, kind):
        if kind == "save":
            self.data["maps"][self.map_id] = self.m
            save_data(self.data); self.flash("Saved to rts_data.json (backup .bak)")
        elif kind == "new":
            nid = self._unique_id("custom")
            self.data["maps"][nid] = new_map(nid)
            self.map_ids.append(nid); self.map_id = nid
            self.m = self.data["maps"][nid]; self._center_on_map()
            self.flash(f"New map '{nid}'")
        elif kind == "export":
            path = os.path.join(HERE, f"map_{self.map_id}.json")
            with open(path, "w") as f: json.dump(self.m, f, indent=1)
            self.flash(f"Exported {os.path.basename(path)}")
        elif kind == "validate":
            errs = validate_map(self.m)
            self.flash("Valid ✓" if not errs else f"{len(errs)} issue(s): {errs[0]}")
        elif kind == "prevmap":
            i = (self.map_ids.index(self.map_id) - 1) % len(self.map_ids)
            self._switch(self.map_ids[i])
        elif kind == "nextmap":
            i = (self.map_ids.index(self.map_id) + 1) % len(self.map_ids)
            self._switch(self.map_ids[i])
        elif kind == "theme":
            self._cycle_theme()
        elif kind == "weather":
            self._cycle_weather()
        elif kind == "sym":
            modes = ["none", "rotate", "mirror_x", "mirror_y"]
            self.symmetry = modes[(modes.index(self.symmetry) + 1) % len(modes)]
            self.flash(f"Symmetry: {self.symmetry}")
        elif kind == "dup":
            nid = self._unique_id(self.map_id + "_copy")
            self.data["maps"][nid] = copy.deepcopy(self.m)
            self.map_ids.append(nid); self._switch(nid); self.flash(f"Duplicated → '{nid}'")
        elif kind == "del":
            if len(self.map_ids) <= 1:
                self.flash("Can't delete the last map"); return
            del self.data["maps"][self.map_id]
            self.map_ids.remove(self.map_id)
            self.map_id = self.map_ids[0]; self.m = self.data["maps"][self.map_id]
            self._ensure_fields(self.m); self.sel = None; self._center_on_map()
            self.flash("Deleted map")
        elif kind == "test":
            self._launch_test()
        elif kind and kind.startswith("win_"):
            self._toggle_window(kind[4:])
        elif kind == "mode":
            self.mode = "data" if self.mode == "map" else "map"
            self.active_field = None; self.flash(f"{self.mode.title()} mode")

    def _switch(self, mid):
        self.map_id = mid; self.m = self.data["maps"][mid]
        self._ensure_fields(self.m); self.sel = None; self._center_on_map()
        self.flash(f"Editing '{mid}'")

    def _unique_id(self, base):
        i = 1; nid = base
        while nid in self.data["maps"]:
            i += 1; nid = f"{base}{i}"
        return nid

    def _cycle_theme(self):
        keys = list(THEME_PRESETS.keys())
        cur = self.m.get("theme", {})
        # find nearest current preset by ground colour, advance
        idx = 0
        for k in keys:
            if THEME_PRESETS[k]["ground"] == cur.get("ground"): idx = keys.index(k)
        nk = keys[(idx + 1) % len(keys)]
        self.m["theme"] = copy.deepcopy(THEME_PRESETS[nk]); self.flash(f"Theme: {nk}")

    def _cycle_weather(self):
        keys = list(WEATHER_PRESETS.keys())
        cur = self.m.get("weather", {}).get("kind", "clear")
        idx = keys.index(cur) if cur in keys else 0
        nk = keys[(idx + 1) % len(keys)]
        self.m["weather"] = copy.deepcopy(WEATHER_PRESETS[nk]); self.flash(f"Weather: {nk}")

    # ── event editing ──
    def _event_action(self, act):
        evs = self.m.setdefault("events", [])
        if act == "add":
            evs.append(dict(id=f"e{len(evs)+1}",
                            trigger=dict(type="time", at=30),
                            actions=[dict(type="message", text="Something happens!")]))
            self.editing_event = evs[-1]; self.flash("Added event")
        elif act == "del" and self.editing_event in evs:
            evs.remove(self.editing_event); self.editing_event = None
        elif act == "cycle_trigger" and self.editing_event:
            tt = self.editing_event["trigger"].get("type", "time")
            nt = TRIGGER_TYPES[(TRIGGER_TYPES.index(tt) + 1) % len(TRIGGER_TYPES)]
            self.editing_event["trigger"] = _default_trigger(nt)
        elif act == "cycle_action" and self.editing_event:
            a0 = self.editing_event["actions"][0]
            at = a0.get("type", "message")
            na = ACTION_TYPES[(ACTION_TYPES.index(at) + 1) % len(ACTION_TYPES)]
            self.editing_event["actions"][0] = _default_action(na)
        elif act == "add_cond" and self.editing_event:
            conds = self.editing_event.setdefault("conditions", [])
            conds.append(dict(type="time_after", at=60))
            self.flash("Added condition")
        elif act == "cycle_cond" and self.editing_event:
            conds = self.editing_event.get("conditions", [])
            if conds:
                order = ["time_after", "time_before", "ink_at_least", "unit_count"]
                ct = conds[0].get("type", "time_after")
                nc = order[(order.index(ct) + 1) % len(order)] if ct in order else order[0]
                conds[0] = _default_condition(nc)
        elif act == "clear_cond" and self.editing_event:
            self.editing_event["conditions"] = []
        elif act == "params":
            self.param_form = self.editing_event is not None; self.modal_off = [0, 0]
        elif act == "close_params":
            self.param_form = False
        elif act.startswith("sel:"):
            i = int(act.split(":")[1])
            if 0 <= i < len(evs): self.editing_event = evs[i]

    def _launch_test(self):
        """Play-test the current map immediately in the real RTS — the SC2
        'Test Document' workflow. Runs an inline session, F10 returns here."""
        import pygame
        errs = validate_map(self.m)
        if any("reachable" in e for e in errs):
            self.flash("Map not playable: " + errs[0]); return
        try:
            import rts
        except Exception as ex:
            self.flash(f"Can't load RTS: {ex}"); return
        mid = "__editor_test__"
        m = copy.deepcopy(self.m)
        rts.MAPS[mid] = m
        rts._MAP_OBJECTS[mid] = list(m.get("objects", []))
        rts._MAP_HIGH[mid] = [tuple(h) for h in m.get("highground", [])]
        # push any live Data-Editor stat edits into the RTS faction tables so the
        # test match reflects your current tuning. _factbook() reads the live
        # FACTIONS global, but the module-level UNITS/STRUCTURES aliases (used by
        # the team-0 HUD & build menu) are bound once at import to the default
        # faction — rebind those too, or edited names/costs show up stale.
        _saved = (rts.FACTIONS, rts.UNITS, rts.STRUCTURES)
        # test with the faction you're editing as team 0 so its edits are visible;
        # pick any other faction as the opponent.
        facs = list(self.data["factions"].keys())
        p0 = self.data_fac if self.data_fac in facs else facs[0]
        p1 = next((f for f in facs if f != p0), p0)
        try:
            rts.FACTIONS = copy.deepcopy(self.data["factions"])
            rts.UNITS = rts.FACTIONS[p0]["units"]
            rts.STRUCTURES = rts.FACTIONS[p0]["structures"]
        except Exception:
            pass
        cards, og, sfx = {}, None, None
        try:
            import carddata, origami
            cards = carddata.load()
            og = origami
        except Exception:
            pass
        screen = pygame.display.get_surface()
        ctx = dict(W=self.W, H=self.H, font=self.font, sfont=self.sfont,
                   bfont=self.bfont, draw_unit=None, cards=cards, origami=og, sfx=sfx)
        game = rts.RTS(ctx, map_id=mid, factions=[p0, p1])
        game.phase = "play"
        clock = pygame.time.Clock()
        self.flash("Testing… F10 to return")
        testing = True
        while testing:
            dt = clock.tick(60) / 1000.0
            for ev in pygame.event.get():
                if ev.type == pygame.QUIT:
                    testing = False
                elif ev.type == pygame.KEYDOWN and ev.key == pygame.K_F10:
                    testing = False
                else:
                    try: game.handle_event(ev)
                    except Exception: pass
            try:
                game.update(dt); game.draw(screen)
            except Exception as ex:
                self.flash(f"Test error: {ex}"); break
            hint = self.sfont.render("TEST MODE — F10 to return to editor", True, (245, 220, 140))
            bg = pygame.Surface((hint.get_width() + 12, 20), pygame.SRCALPHA)
            bg.fill((20, 16, 12, 180)); screen.blit(bg, (8, self.H - 24))
            screen.blit(hint, (14, self.H - 23))
            pygame.display.flip()
        for d in (rts.MAPS, rts._MAP_OBJECTS, rts._MAP_HIGH):
            d.pop(mid, None)
        rts.FACTIONS, rts.UNITS, rts.STRUCTURES = _saved
        self.flash("Returned from test")

    def update(self, dt):
        self.status_t = max(0.0, self.status_t - dt)

    # ── drawing ──
    def draw(self, screen):
        import pygame
        screen.fill((26, 23, 19))
        if self.mode == "data":
            self._draw_topbar(screen)
            self._draw_data(screen)
            if self.spell_edit is not None:
                self._draw_spell_form(screen)
        else:
            self._draw_canvas(screen)
            self._draw_toolbar(screen)
            self._draw_topbar(screen)
            self._draw_panel(screen)
            self._draw_events(screen)
            if self.param_form and self.editing_event:
                self._draw_param_form(screen)
        # floating windows render above everything, in z-order
        self._draw_windows(screen)
        if self.ctx_menu is not None:
            self._draw_ctx_menu(screen)
        if self.show_help:
            self._draw_help(screen)

    # ── floating window manager ───────────────────────────────────────────────
    _WIN_DEFAULTS = {
        "assets":   ("Assets", (260, 90, 380, 460)),
        "models":   ("Model Picker", (300, 120, 420, 420)),
        "triggers": ("Trigger Browser", (200, 100, 560, 400)),
        "sections": ("Sections", (40, 90, 200, 296)),
        "palette":  ("Palette Scripts", (240, 80, 560, 480)),
    }

    def _open_window(self, wid):
        import pygame
        for w in self.windows:
            if w["id"] == wid:               # already open → bring to front
                self.windows.remove(w); self.windows.append(w); return
        title, (x, y, ww, hh) = self._WIN_DEFAULTS.get(wid, (wid.title(), (200, 120, 400, 360)))
        self.windows.append(dict(id=wid, title=title, rect=pygame.Rect(x, y, ww, hh)))

    def _close_window(self, wid):
        self.windows = [w for w in self.windows if w["id"] != wid]

    def _toggle_window(self, wid):
        if any(w["id"] == wid for w in self.windows):
            self._close_window(wid)
        else:
            self._open_window(wid)

    def _window_at(self, mx, my):
        import pygame
        for w in reversed(self.windows):
            title = pygame.Rect(w["rect"].x, w["rect"].y - 26, w["rect"].w, 26)
            if w["rect"].collidepoint(mx, my) or title.collidepoint(mx, my):
                return w
        return None

    def _draw_windows(self, screen):
        import pygame
        for w in self.windows:
            r = w["rect"]
            # title bar
            tb = pygame.Rect(r.x, r.y - 26, r.w, 26)
            pygame.draw.rect(screen, (54, 60, 74), tb, border_top_left_radius=8, border_top_right_radius=8)
            pygame.draw.rect(screen, (150, 190, 220), tb, 1, border_top_left_radius=8, border_top_right_radius=8)
            screen.blit(self.sfont.render(w["title"], True, (225, 232, 240)), (tb.x + 10, tb.y + 6))
            # close button
            cb = pygame.Rect(tb.right - 24, tb.y + 3, 20, 20)
            pygame.draw.rect(screen, (90, 54, 50), cb, border_radius=4)
            screen.blit(self.sfont.render("×", True, (240, 210, 205)), (cb.x + 6, cb.y + 2))
            w["_close"] = cb
            # body
            pygame.draw.rect(screen, (40, 44, 54), r)
            pygame.draw.rect(screen, (120, 150, 180), r, 1)
            prev = screen.get_clip(); screen.set_clip(r)
            w["_content_h"] = 0          # renderers set this to their full height
            getattr(self, f"_win_{w['id']}", self._win_missing)(screen, w)
            screen.set_clip(prev)
            # scrollbar (if content taller than the window)
            content_h = w.get("_content_h", 0)
            if content_h > r.h:
                w["scroll"] = max(0, min(w.get("scroll", 0), content_h - r.h))
                track = pygame.Rect(r.right - 7, r.y + 2, 5, r.h - 4)
                pygame.draw.rect(screen, (30, 33, 40), track, border_radius=3)
                frac = r.h / content_h
                bar_h = max(24, int(track.h * frac))
                bar_y = track.y + int((track.h - bar_h) * (w["scroll"] / (content_h - r.h)))
                pygame.draw.rect(screen, (120, 150, 180), (track.x, bar_y, 5, bar_h), border_radius=3)
            else:
                w["scroll"] = 0
            # resize handle (bottom-right grip)
            hs = pygame.Rect(r.right - 14, r.bottom - 14, 14, 14)
            for gi in range(3):
                off = gi * 4
                pygame.draw.line(screen, (150, 170, 195),
                                 (hs.right - 3 - off, hs.bottom - 3),
                                 (hs.right - 3, hs.bottom - 3 - off), 1)
            w["_resize"] = hs

    def _win_oy(self, w):
        """Vertical offset for a window's content given its scroll position."""
        return -w.get("scroll", 0)

    def _win_missing(self, screen, w):
        import pygame
        screen.blit(self.sfont.render("(empty)", True, (170, 180, 190)), (w["rect"].x + 12, w["rect"].y + 12))

    def _win_click(self, w, mx, my):
        import pygame
        """Return True if the click was consumed by this window's content."""
        wid = w["id"]
        for rect, tag in w.get("_hot", []):
            if rect.collidepoint(mx, my):
                self._window_action(wid, tag); return True
        return True     # clicks inside a window are always consumed

    def _window_action(self, wid, tag):
        if wid == "sections" and isinstance(tag, tuple):
            act, val = tag
            if act == "mode": self.mode = val
            elif act == "open": self._open_window(val)
            elif act == "help": self.show_help = True
            return
        if wid == "palette" and isinstance(tag, tuple) and tag[0] == "pal":
            self._pal_action(tag[1]); return
        if wid == "models" and isinstance(tag, str):
            # assign the picked card model to the selected unit
            fac = self.data["factions"].get(self.data_fac, {})
            spec = fac.get("units", {}).get(self.data_sel)
            if spec is not None:
                spec["card"] = tag; self.flash(f"Model → {tag}")
        elif wid == "assets" and isinstance(tag, tuple):
            kind, val = tag
            if kind == "sound" and self.spell_edit is not None:
                self.spell_edit["sfx"] = val; self.flash(f"Cast sound → {val}")
            elif kind == "vfx" and self.spell_edit is not None and self.spell_edit.get("effects"):
                self.spell_edit["effects"][0]["vfx"] = val; self.flash(f"VFX → {val}")
        elif wid == "triggers" and isinstance(tag, int):
            evs = self.m.get("events", [])
            if 0 <= tag < len(evs):
                self.editing_event = evs[tag]; self.mode = "map"

    # ── window content renderers ──
    def _win_assets(self, screen, w):
        import pygame
        r = w["rect"]; w["_hot"] = []
        oy = self._win_oy(w)
        y = r.y + 8 + oy
        def head(t):
            nonlocal y
            screen.blit(self.sfont.render(t, True, (200, 210, 150)), (r.x + 10, y)); y += 18
        head("SOUNDS (click to assign as cast sfx)")
        sounds = ["whoosh", "thunder", "fire", "freeze", "deploy", "coin",
                  "tower_hit", "card_draw", "select", "error", "victory",
                  "defeat", "click", "hover", "back", "toggle"]
        for i, s in enumerate(sounds):
            rect = pygame.Rect(r.x + 12 + (i % 2) * 175, y + (i // 2) * 22, 165, 20)
            pygame.draw.rect(screen, (52, 56, 66), rect, border_radius=4)
            screen.blit(self.sfont.render("♪ " + s, True, (220, 226, 234)), (rect.x + 6, rect.y + 3))
            w["_hot"].append((rect, ("sound", s)))
        y += ((len(sounds) + 1) // 2) * 22 + 12
        head("VFX THEMES (click to theme spell's 1st effect)")
        import rts
        themes = list(getattr(rts, "VFX_THEMES", {}).keys()) or \
            ["fire", "ice", "ink", "holy", "shadow", "nature", "arcane", "water", "gold", "blood"]
        for i, t in enumerate(themes):
            rect = pygame.Rect(r.x + 12 + (i % 2) * 175, y + (i // 2) * 22, 165, 20)
            col = rts.VFX_THEMES.get(t, ((150, 150, 150), 0))[0] if hasattr(rts, "VFX_THEMES") else (150, 150, 150)
            pygame.draw.rect(screen, (52, 56, 66), rect, border_radius=4)
            pygame.draw.circle(screen, col, (rect.x + 10, rect.centery), 5)
            screen.blit(self.sfont.render(t, True, (220, 226, 234)), (rect.x + 22, rect.y + 3))
            w["_hot"].append((rect, ("vfx", t)))
        y += ((len(themes) + 1) // 2) * 22 + 8
        screen.blit(self.sfont.render("Open the Spell Composer to target effects.",
                    True, (150, 160, 170)), (r.x + 10, y)); y += 20
        w["_content_h"] = (y - oy) - r.y + 8

    def _win_models(self, screen, w):
        import pygame
        r = w["rect"]; w["_hot"] = []
        oy = self._win_oy(w)
        cards = self._all_card_keys()
        sel_card = None
        fac = self.data["factions"].get(self.data_fac, {})
        spec = fac.get("units", {}).get(self.data_sel)
        if spec: sel_card = spec.get("card")
        screen.blit(self.sfont.render(
            f"Models for: {spec.get('name') if spec else '(pick a unit in Data)'}",
            True, (200, 210, 150)), (r.x + 10, r.y + 8))
        cols = 3; cw = (r.w - 24) // cols
        top = r.y + 34
        for i, key in enumerate(cards):
            cx = r.x + 12 + (i % cols) * cw
            cy = top + (i // cols) * 74 + oy
            if cy + 68 < r.y or cy > r.bottom:      # cull off-view rows
                w["_hot"].append((pygame.Rect(-99, -99, 1, 1), key)); continue
            box = pygame.Rect(cx, cy, cw - 8, 68)
            on = (key == sel_card)
            pygame.draw.rect(screen, (60, 66, 80) if on else (48, 52, 62), box, border_radius=5)
            pygame.draw.rect(screen, (150, 200, 120) if on else (90, 96, 110), box, 2 if on else 1, border_radius=5)
            self._draw_model_preview(screen, key, box.centerx, box.y + 30)
            lbl = self.sfont.render(key[:12], True, (220, 226, 234))
            screen.blit(lbl, (box.centerx - lbl.get_width() // 2, box.bottom - 16))
            w["_hot"].append((box, key))
        rows = (len(cards) + cols - 1) // cols
        w["_content_h"] = 34 + rows * 74 + 10

    def _draw_model_preview(self, screen, card_key, cx, cy):
        import pygame
        """Draw a tiny origami preview of a card model, or a token fallback."""
        og = self._og; cards = self._cards
        if og is not None and cards is not None and card_key in cards:
            try:
                card = cards[card_key]
                recipe = og.recipe_for(card_key, card.get("family", "figure"), card)
                base = tuple(int(c * 0.6 + 90) for c in card.get("paper", (200, 190, 170)))
                s = 13
                for part in recipe:
                    ax, ay = part.get("at", [0, 0])
                    psz = s * part.get("size", 1.0)
                    pcol = tuple(min(255, int(c * part.get("tone", 1.0))) for c in base)
                    og.draw_part(screen, part.get("part"), cx + ax * s, cy + ay * s - 4,
                                 psz, 1, pcol, part.get("h", 0.4) * psz * 0.5, 0.0, 0.0, None)
                return
            except Exception:
                pass
        pygame.draw.polygon(screen, (150, 160, 175),
                            [(cx, cy - 12), (cx + 11, cy + 8), (cx - 11, cy + 8)])

    def _win_triggers(self, screen, w):
        import pygame
        r = w["rect"]; w["_hot"] = []
        screen.blit(self.sfont.render("Map events (click to edit in the bottom bar):",
                    True, (200, 210, 150)), (r.x + 10, r.y + 8))
        evs = self.m.get("events", [])
        y = r.y + 32
        for i, e in enumerate(evs):
            rect = pygame.Rect(r.x + 12, y, r.w - 24, 22)
            pygame.draw.rect(screen, (52, 56, 66), rect, border_radius=4)
            tt = e.get("trigger", {}).get("type", "?")
            at = (e.get("actions", [{}])[0]).get("type", "?")
            nc = len(e.get("conditions", []))
            screen.blit(self.sfont.render(f"{e.get('id','?')}: when {tt}"
                        f"{f' + {nc} cond' if nc else ''} → {at}", True, (222, 228, 236)),
                        (rect.x + 6, rect.y + 3))
            w["_hot"].append((rect, i))
            y += 26
        if not evs:
            screen.blit(self.sfont.render("No events yet — add them in the bottom bar.",
                        True, (150, 160, 170)), (r.x + 10, y))

    def _win_sections(self, screen, w):
        import pygame
        r = w["rect"]; w["_hot"] = []
        items = [("Map (terrain/objects)", ("mode", "map")),
                 ("Data (unit stats)", ("mode", "data")),
                 ("Triggers", ("open", "triggers")),
                 ("Assets", ("open", "assets")),
                 ("Models", ("open", "models")),
                 ("Palette scripts", ("open", "palette")),
                 ("Help (F1)", ("help", None))]
        y = r.y + 10
        for label, act in items:
            rect = pygame.Rect(r.x + 10, y, r.w - 20, 30)
            pygame.draw.rect(screen, (52, 56, 66), rect, border_radius=5)
            pygame.draw.rect(screen, (120, 150, 180), rect, 1, border_radius=5)
            screen.blit(self.sfont.render(label, True, (222, 228, 236)), (rect.x + 10, rect.y + 8))
            w["_hot"].append((rect, act))
            y += 36

    def _card_keys(self):
        if self._cards:
            return sorted(self._cards.keys())[:24]
        return ["gorilla", "crane", "sabertooth", "wolf", "turtle", "fox",
                "beetle", "hawk", "serpent", "bear", "stag", "moth"]

    def _all_card_keys(self):
        if self._cards:
            return sorted(self._cards.keys())
        return self._card_keys()

    def _draw_help(self, screen):
        import pygame
        dim = pygame.Surface((self.W, self.H), pygame.SRCALPHA); dim.fill((10, 8, 6, 200))
        screen.blit(dim, (0, 0))
        lines = [
            ("Foldworks Map Editor — Controls", True),
            ("", False),
            ("Left-click canvas — place / select with the current tool", False),
            ("Select tool — click an item, then DRAG to move it", False),
            ("Right-drag — pan     Mouse-wheel — zoom", False),
            ("Arrows / WASD — pan     [ ] — brush size", False),
            ("Delete / Backspace — remove the selected item", False),
            ("", False),
            ("M — cycle symmetry (none / rotate 180 / mirror X / mirror Y)", False),
            ("G — toggle grid snap", False),
            ("Ctrl+Z / Ctrl+Y — undo / redo", False),
            ("Ctrl+C / Ctrl+V / Ctrl+D — copy / paste / duplicate selection", False),
            ("Shift+arrows — nudge the selected object", False),
            ("", False),
            ("F5 or Test ▶ — play-test this map in the RTS now", False),
            ("F10 — return to the editor from test mode", False),
            ("F1 — toggle this help", False),
            ("", False),
            ("Top bar: New / Dup / Del maps · Save (to rts_data.json)", False),
            ("         Export (standalone JSON) · Validate (+pathing check)", False),
            ("◆ Mode — switch between Map layout and the Data Editor (unit stats)", False),
            ("Bottom: Event scripts — When (trigger) / If (conditions) / Do (actions)", False),
            ("Analysis overlay: rush distance, expansions per side, symmetry", False),
        ]
        y = 120
        for txt, hdr in lines:
            f = self.font if hdr else self.sfont
            col = (224, 196, 120) if hdr else (216, 208, 192)
            screen.blit(f.render(txt, True, col), (self.W // 2 - 300, y))
            y += 26 if hdr else 20
        tip = self.sfont.render("Press F1 to close", True, (150, 200, 235))
        screen.blit(tip, (self.W // 2 - tip.get_width() // 2, y + 10))

    def _draw_param_form(self, screen):
        """Modal overlay: edit every numeric/text field of the selected event's
        trigger, conditions, and actions — no JSON round-trip needed."""
        import pygame
        self.field_rects = [r for r in self.field_rects]  # keep map-field rects too
        dim = pygame.Surface((self.W, self.H), pygame.SRCALPHA); dim.fill((10, 8, 6, 180))
        screen.blit(dim, (0, 0))
        fw, fh = 560, 520
        fx, fy = (self.W - fw) // 2 + self.modal_off[0], (self.H - fh) // 2 + self.modal_off[1]
        pygame.draw.rect(screen, (40, 35, 28), (fx, fy, fw, fh), border_radius=8)
        pygame.draw.rect(screen, (200, 170, 110), (fx, fy, fw, fh), 2, border_radius=8)
        e = self.editing_event
        screen.blit(self.font.render(f"Event '{e.get('id','?')}' parameters  (drag to move)", True, (224, 196, 120)),
                    (fx + 16, fy + 12))
        self._modal_bar = pygame.Rect(fx, fy, fw, 36)
        y = fy + 44
        def section(title, d, prefix):
            nonlocal y
            screen.blit(self.sfont.render(title, True, (190, 210, 180)), (fx + 16, y)); y += 20
            for k, v in list(d.items()):
                if k in ("type",) or k.startswith("_"):
                    screen.blit(self.sfont.render(f"  {k}: {v}", True, (150, 150, 140)), (fx + 24, y))
                    y += 20; continue
                screen.blit(self.sfont.render(f"  {k}", True, (200, 192, 176)), (fx + 24, y))
                rect = pygame.Rect(fx + 180, y - 2, 340, 20)
                active = self.active_field == (d, k)
                pygame.draw.rect(screen, (60, 52, 40) if not active else (78, 66, 46), rect, border_radius=4)
                pygame.draw.rect(screen, (220, 180, 90) if active else (90, 80, 66), rect, 1, border_radius=4)
                shown = (self.buffer + "|") if active else str(v)
                screen.blit(self.sfont.render(shown[:44], True, (235, 228, 212)), (rect.x + 5, rect.y + 3))
                self.field_rects.append((rect, d, k))
                y += 24
        section("TRIGGER (when)", e.get("trigger", {}), "tr")
        for i, c in enumerate(e.get("conditions", [])):
            section(f"CONDITION {i+1} (if)", c, f"c{i}")
        for i, a in enumerate(e.get("actions", [])):
            section(f"ACTION {i+1} (do)", a, f"a{i}")
        # close button
        cb = pygame.Rect(fx + fw - 90, fy + fh - 36, 78, 26)
        pygame.draw.rect(screen, (86, 66, 34), cb, border_radius=5)
        pygame.draw.rect(screen, (220, 180, 90), cb, 1, border_radius=5)
        screen.blit(self.sfont.render("Close", True, (235, 228, 212)), (cb.x + 20, cb.y + 5))
        self.event_rects.append((cb, "close_params"))

    def _analyze(self):
        """Competitive-melee balance readout: rush distance between mains,
        expansions per side, and a rotational-symmetry check."""
        m = self.m; w, h = m["size"]
        sp = m.get("spawns", [])
        out = {}
        if len(sp) >= 2:
            out["rush"] = round(math.hypot(sp[0][0] - sp[1][0], sp[0][1] - sp[1][1]))
        exps = [o for o in m.get("objects", []) if o.get("kind") in ("richnode", "relic")]
        near0 = near1 = 0
        for o in exps:
            if len(sp) >= 2:
                d0 = math.hypot(o["x"] - sp[0][0], o["y"] - sp[0][1])
                d1 = math.hypot(o["x"] - sp[1][0], o["y"] - sp[1][1])
                if d0 < d1: near0 += 1
                else: near1 += 1
        out["exp"] = (near0, near1, len(exps))
        def mirror_of(x, y): return (w - x, h - y)
        pts = [(o["x"], o["y"]) for o in m.get("objects", [])] + \
              [(x, y) for x, y, r in m.get("highground", [])]
        unmatched = 0
        for (x, y) in pts:
            mx, my = mirror_of(x, y)
            if not any(abs(px - mx) < 80 and abs(py - my) < 80 for (px, py) in pts):
                unmatched += 1
        out["sym"] = (len(pts) - unmatched, len(pts))
        out["high"] = len(m.get("highground", []))
        return out

    def _draw_analysis(self, screen, x, y):
        a = self._analyze()
        lines = []
        if "rush" in a: lines.append(f"Rush distance: {a['rush']}")
        e0, e1, et = a.get("exp", (0, 0, 0))
        lines.append(f"Expansions: {et}  (A:{e0}  B:{e1})")
        sm, st = a.get("sym", (0, 0))
        bal = "balanced" if st and sm >= st - 1 else f"{st-sm} unmatched"
        lines.append(f"Symmetry: {bal}")
        lines.append(f"High ground: {a.get('high',0)}")
        if e0 != e1:
            lines.append("! uneven expansions per side")
        for i, ln in enumerate(lines):
            col = (235, 150, 110) if ln.startswith("!") else (190, 210, 180)
            screen.blit(self.sfont.render(ln, True, col), (x, y + i * 16))

    def _draw_data(self, screen):
        """The Data Editor: pick a faction and category, choose an entry, and
        edit its numeric/text stats (hp, dmg, cost, supply, …) — SC2 style."""
        import pygame
        self.data_rects = []
        self.field_rects = []
        facs = list(self.data["factions"].keys())
        x = 16; y = TOPBAR_H + 10
        for fk in facs:
            fac = self.data["factions"][fk]
            rect = pygame.Rect(x, y, 150, 28)
            active = fk == self.data_fac
            pygame.draw.rect(screen, (60,52,40) if not active else tuple(int(c*0.5) for c in fac["color"]),
                             rect, border_radius=5)
            pygame.draw.rect(screen, fac["color"] if active else (90,82,70), rect, 2 if active else 1, border_radius=5)
            screen.blit(self.sfont.render(fac["name"], True, (232,224,208)), (rect.x+8, rect.y+7))
            self.data_rects.append((rect, ("fac", fk)))
            x += 158
        y2 = y + 38; x = 16
        for cat in ("units", "structures", "upgrades"):
            rect = pygame.Rect(x, y2, 120, 24)
            active = cat == self.data_cat
            pygame.draw.rect(screen, (74,62,42) if active else (48,42,34), rect, border_radius=5)
            pygame.draw.rect(screen, (220,180,90) if active else (90,82,70), rect, 1, border_radius=5)
            screen.blit(self.sfont.render(cat.title(), True, (232,224,208)), (rect.x+10, rect.y+4))
            self.data_rects.append((rect, ("cat", cat)))
            x += 128
        # Add / Duplicate / Delete entry buttons (author custom content)
        for label, act, bx in (("+ Add", "add", x+20), ("Dup", "dup", x+90),
                               ("Del", "del", x+150)):
            rect = pygame.Rect(bx, y2, 60 if act != "add" else 60, 24)
            pygame.draw.rect(screen, (50,66,44) if act=="add" else (60,52,40), rect, border_radius=5)
            pygame.draw.rect(screen, (150,200,120) if act=="add" else (200,170,110), rect, 1, border_radius=5)
            screen.blit(self.sfont.render(label, True, (232,224,208)), (rect.x+8, rect.y+4))
            self.data_rects.append((rect, ("entryop", act)))
        fac = self.data["factions"][self.data_fac]
        entries = fac.get(self.data_cat, {})
        ly = y2 + 36; lx = 16
        pygame.draw.rect(screen, (34,30,24), (8, ly-6, 250, self.H - ly - 8))
        screen.blit(self.sfont.render(f"{len(entries)} {self.data_cat}", True, (200,180,120)), (lx, ly-2))
        ly += 20
        for key in entries:
            rect = pygame.Rect(lx, ly, 236, 22)
            sel = key == self.data_sel
            pygame.draw.rect(screen, (66,54,36) if sel else (44,38,30), rect, border_radius=3)
            nm = entries[key].get("name", key)
            screen.blit(self.sfont.render(f"{nm}", True, (232,224,208)), (rect.x+6, rect.y+3))
            screen.blit(self.sfont.render(key, True, (140,150,150)), (rect.right-70, rect.y+3))
            self.data_rects.append((rect, ("entry", key)))
            ly += 24
            if ly > self.H - 30: break
        if self.data_sel and self.data_sel in entries:
            spec = entries[self.data_sel]
            fx = 280; fy = y2 + 30
            title = f"{spec.get('name', self.data_sel)}   ({self.data_fac} · {self.data_cat[:-1]}, key '{self.data_sel}')"
            screen.blit(self.font.render(title, True, (224,196,120)), (fx, fy)); fy += 30
            col = 0; startx = fx
            for k, v in spec.items():
                screen.blit(self.sfont.render(k, True, (190,182,166)), (startx + col*330, fy))
                rect = pygame.Rect(startx + col*330 + 90, fy-2, 210, 20)
                active = self.active_field == (spec, k)
                # lists/dicts editable as comma-separated text (e.g. makes, researches)
                # the 'ability' dict gets a dedicated spell composer
                if k == "projectile":
                    # cycle through the engine's missile styles — the unit's
                    # ranged shots wear this (faction-level default supported
                    # by setting it on the faction itself)
                    pygame.draw.rect(screen, (58, 50, 62), rect, border_radius=4)
                    pygame.draw.rect(screen, (190, 160, 210), rect, 1, border_radius=4)
                    screen.blit(self.sfont.render(f"style: {v}", True, (226, 212, 236)),
                                (rect.x + 5, rect.y + 3))
                    self.data_rects.append((rect, ("cycleproj", k)))
                elif k == "ability" and isinstance(v, dict):
                    n_eff = len(v.get("effects", []))
                    pygame.draw.rect(screen, (50,60,74), rect, border_radius=4)
                    pygame.draw.rect(screen, (150,190,220), rect, 1, border_radius=4)
                    screen.blit(self.sfont.render(f"Edit spell… ({n_eff} effects)", True, (200,224,235)),
                                (rect.x+5, rect.y+3))
                    self.data_rects.append((rect, ("editspell", None)))
                elif isinstance(v, dict):
                    shown = str(v)[:28]
                    pygame.draw.rect(screen, (40,36,30), rect, border_radius=4)
                    screen.blit(self.sfont.render(shown, True, (150,150,140)), (rect.x+5, rect.y+3))
                else:
                    pygame.draw.rect(screen, (48,42,34) if not active else (70,60,44), rect, border_radius=4)
                    pygame.draw.rect(screen, (220,180,90) if active else (80,72,60), rect, 1, border_radius=4)
                    if active:
                        shown = self.buffer + "|"
                    elif isinstance(v, list):
                        shown = ", ".join(str(x) for x in v)
                    else:
                        shown = str(v)
                    screen.blit(self.sfont.render(shown[:28], True, (235,228,212)), (rect.x+5, rect.y+3))
                    self.field_rects.append((rect, spec, k))
                fy += 26
                if fy > self.H - 60:
                    fy = y2 + 60; col += 1
            # add-field control (for abilities / behaviours / flags)
            addf = pygame.Rect(startx + col*330, fy + 4, 210, 22)
            pygame.draw.rect(screen, (50,60,44), addf, border_radius=4)
            pygame.draw.rect(screen, (150,200,120), addf, 1, border_radius=4)
            if self.active_field == (spec, "__newfield__"):
                screen.blit(self.sfont.render("new field: " + self.buffer + "|", True, (235,228,212)), (addf.x+6, addf.y+4))
            else:
                screen.blit(self.sfont.render("+ add field (ability, cloak, worker…)", True, (200,224,200)), (addf.x+6, addf.y+4))
            self.field_rects.append((addf, spec, "__newfield__"))
            hint = "Click a value, type, Enter · lists are comma-separated · Save / Test to apply"
            screen.blit(self.sfont.render(hint, True, (150,140,124)), (fx, self.H - 22))
        else:
            screen.blit(self.sfont.render("Select an entry, or + Add to create a custom one.",
                        True, (150,140,124)), (300, y2 + 40))
        if self.status_t > 0:
            screen.blit(self.sfont.render(self.status, True, (245,220,140)),
                        (self.W - 300, self.H - 22))

    def _data_click(self, mx, my):
        for rect, (kind, val) in getattr(self, "data_rects", []):
            if rect.collidepoint(mx, my):
                if kind == "fac": self.data_fac = val; self.data_sel = None
                elif kind == "cat": self.data_cat = val; self.data_sel = None
                elif kind == "entry": self.data_sel = val
                elif kind == "entryop": self._data_entry_op(val)
                elif kind == "editspell":
                    fac = self.data["factions"][self.data_fac]
                    spec = fac.get(self.data_cat, {}).get(self.data_sel, {})
                    self.spell_edit = spec.get("ability"); self.modal_off = [0, 0]
                elif kind == "spellop": self._spell_op(val)
                elif kind == "cycleproj":
                    fac = self.data["factions"][self.data_fac]
                    spec = fac.get(self.data_cat, {}).get(self.data_sel, {})
                    cur = spec.get("projectile", "dart")
                    nxt = PROJECTILE_STYLES[
                        (PROJECTILE_STYLES.index(cur) + 1) % len(PROJECTILE_STYLES)] \
                        if cur in PROJECTILE_STYLES else PROJECTILE_STYLES[0]
                    spec["projectile"] = nxt
                    self.flash(f"Projectile → {nxt}")
                self.active_field = None
                return True
        return False

    def _data_entry_op(self, op):
        fac = self.data["factions"][self.data_fac]
        entries = fac.setdefault(self.data_cat, {})
        if op == "add":
            nk = self._unique_key(entries, "custom")
            entries[nk] = _default_entry(self.data_cat, nk)
            self.data_sel = nk; self.flash(f"Added {self.data_cat[:-1]} '{nk}'")
        elif op == "dup" and self.data_sel in entries:
            nk = self._unique_key(entries, self.data_sel + "2")
            entries[nk] = copy.deepcopy(entries[self.data_sel])
            entries[nk]["name"] = entries[nk].get("name", nk) + " (copy)"
            self.data_sel = nk; self.flash(f"Duplicated → '{nk}'")
        elif op == "del" and self.data_sel in entries:
            del entries[self.data_sel]; self.data_sel = None; self.flash("Deleted entry")

    def _unique_key(self, d, base):
        k = base; i = 1
        while k in d:
            i += 1; k = f"{base}{i}"
        return k

    def _spell_op(self, val):
        ab = self.spell_edit
        if ab is None: return
        effs = ab.setdefault("effects", [])
        if val == "close":
            self.spell_edit = None
        elif val == "addeff":
            effs.append(_default_effect("damage")); self.flash("Added effect")
        elif isinstance(val, tuple) and val[0] == "cycle":
            i = val[1]
            if 0 <= i < len(effs):
                order = list(EFFECT_PRIMITIVES.keys())
                cur = effs[i].get("effect", "damage")
                nk = order[(order.index(cur) + 1) % len(order)] if cur in order else order[0]
                effs[i] = _default_effect(nk)
        elif isinstance(val, tuple) and val[0] == "deleff":
            i = val[1]
            if 0 <= i < len(effs): effs.pop(i)
        elif isinstance(val, tuple) and val[0] == "addmod":
            i = val[1]
            if 0 <= i < len(effs):
                # cycle in the next optional modifier not yet present
                for mk, mv in (("delay", 0.5), ("vfx", "fire"),
                               ("if_hp_below", 0.5), ("if_hp_above", 0.5)):
                    if mk not in effs[i]:
                        effs[i][mk] = mv; self.flash(f"Added {mk}"); break
        elif isinstance(val, tuple) and val[0] == "cycledelivery":
            cur = ab.get("delivery", "area")
            nxt = DELIVERY_MODES[(DELIVERY_MODES.index(cur) + 1)
                                 % len(DELIVERY_MODES)] \
                if cur in DELIVERY_MODES else DELIVERY_MODES[0]
            ab["delivery"] = nxt
            self.flash(f"Delivery → {nxt}")
        elif isinstance(val, tuple) and val[0] == "chainparams":
            # toggle chain params onto the ability when delivery is chain
            ab.setdefault("chain_count", 3); ab.setdefault("chain_range", 240)

    def _draw_spell_form(self, screen):
        """Compose a custom spell from effect primitives — the abstraction that
        lets the editor build abilities the engine runs generically."""
        import pygame
        ab = self.spell_edit
        self.data_rects = getattr(self, "data_rects", [])
        self.field_rects = []
        dim = pygame.Surface((self.W, self.H), pygame.SRCALPHA); dim.fill((10, 8, 6, 190))
        screen.blit(dim, (0, 0))
        fw, fh = 720, 560
        fx, fy = (self.W - fw)//2 + self.modal_off[0], (self.H - fh)//2 + self.modal_off[1]
        pygame.draw.rect(screen, (38,42,52), (fx, fy, fw, fh), border_radius=8)
        pygame.draw.rect(screen, (150,190,220), (fx, fy, fw, fh), 2, border_radius=8)
        screen.blit(self.font.render("Spell Composer  (drag title to move)", True, (200,224,235)), (fx+16, fy+12))
        self._modal_bar = pygame.Rect(fx, fy, fw, 36)   # draggable title strip
        y = fy + 44
        # ability-level scalar fields
        for k in ("name", "radius", "cast_range", "dur", "cost", "cooldown",
                  "delivery", "sfx", "desc"):
            if k not in ab: continue
            if k == "delivery":
                # click-to-cycle through the engine's delivery modes; rain
                # grows its tuning knobs the moment it's chosen
                screen.blit(self.sfont.render(k, True, (200, 192, 176)), (fx + 16, y + 2))
                rect = pygame.Rect(fx + 110, y, 110, 20)
                pygame.draw.rect(screen, (50, 62, 56), rect, border_radius=4)
                pygame.draw.rect(screen, (120, 170, 130), rect, 1, border_radius=4)
                screen.blit(self.sfont.render(str(ab.get("delivery", "area")),
                                              True, (214, 232, 216)),
                            (rect.x + 8, rect.y + 3))
                self.data_rects.append((rect, ("spellop", ("cycledelivery", None))))
                if ab.get("delivery") == "rain":
                    for pi, pk in enumerate(("rain_count", "rain_radius")):
                        ab.setdefault(pk, 6 if pk == "rain_count" else 60)
                        pr = pygame.Rect(rect.right + 8 + pi * 116, y, 108, 20)
                        pactive = self.active_field == (ab, pk)
                        pygame.draw.rect(screen, (52, 56, 66) if not pactive
                                         else (72, 76, 86), pr, border_radius=4)
                        screen.blit(self.sfont.render(f"{pk}={ab[pk]}", True,
                                                      (208, 200, 184)),
                                    (pr.x + 6, pr.y + 3))
                        self.field_rects.append((pr, (ab, pk)))
                y += 26
                continue
            screen.blit(self.sfont.render(k, True, (200,192,176)), (fx+16, y+2))
            rect = pygame.Rect(fx+110, y, 180, 20)
            active = self.active_field == (ab, k)
            pygame.draw.rect(screen, (52,56,66) if not active else (72,76,86), rect, border_radius=4)
            pygame.draw.rect(screen, (150,190,220) if active else (90,96,110), rect, 1, border_radius=4)
            shown = (self.buffer+"|") if active else str(ab[k])
            screen.blit(self.sfont.render(shown[:22], True, (235,235,240)), (rect.x+5, rect.y+3))
            self.field_rects.append((rect, ab, k))
            y += 24
        y += 6
        screen.blit(self.sfont.render("EFFECTS (run in order):", True, (190,210,220)), (fx+16, y)); y += 22
        for i, eff in enumerate(ab.get("effects", [])):
            # effect-kind cycle button
            krect = pygame.Rect(fx+16, y, 100, 22)
            pygame.draw.rect(screen, (60,66,80), krect, border_radius=4)
            pygame.draw.rect(screen, (150,190,220), krect, 1, border_radius=4)
            screen.blit(self.sfont.render(eff.get("effect","?"), True, (220,230,240)), (krect.x+8, krect.y+3))
            self.data_rects.append((krect, ("spellop", ("cycle", i))))
            # its params inline, wrapping to the next line if they run out of room
            px = fx + 126; rowy = y; maxx = fx + fw - 90
            for pk, pv in eff.items():
                if pk == "effect": continue
                lbl = self.sfont.render(f"{pk}", True, (170,180,190))
                bw = 60
                if px + lbl.get_width() + 6 + bw > maxx:      # wrap
                    rowy += 24; px = fx + 126
                screen.blit(lbl, (px, rowy+3))
                rect = pygame.Rect(px + lbl.get_width() + 4, rowy, bw, 20)
                active = self.active_field == (eff, pk)
                pygame.draw.rect(screen, (52,56,66) if not active else (72,76,86), rect, border_radius=3)
                pygame.draw.rect(screen, (150,190,220) if active else (90,96,110), rect, 1, border_radius=3)
                shown = (self.buffer+"|") if active else str(pv)
                screen.blit(self.sfont.render(shown[:8], True, (235,235,240)), (rect.x+4, rect.y+3))
                self.field_rects.append((rect, eff, pk))
                px = rect.right + 10
            # +mod (adds delay / hp conditions) and delete effect
            mrect = pygame.Rect(fx+fw-68, y, 24, 22)
            pygame.draw.rect(screen, (50,60,44), mrect, border_radius=4)
            screen.blit(self.sfont.render("+", True, (200,235,200)), (mrect.x+8, mrect.y+3))
            self.data_rects.append((mrect, ("spellop", ("addmod", i))))
            drect = pygame.Rect(fx+fw-40, y, 24, 22)
            pygame.draw.rect(screen, (80,50,44), drect, border_radius=4)
            screen.blit(self.sfont.render("×", True, (235,200,200)), (drect.x+8, drect.y+3))
            self.data_rects.append((drect, ("spellop", ("deleff", i))))
            y = max(y, rowy) + 28
        # add-effect + close
        arect = pygame.Rect(fx+16, y+6, 150, 24)
        pygame.draw.rect(screen, (50,66,44), arect, border_radius=5)
        pygame.draw.rect(screen, (150,200,120), arect, 1, border_radius=5)
        screen.blit(self.sfont.render("+ Add effect", True, (220,235,220)), (arect.x+10, arect.y+5))
        self.data_rects.append((arect, ("spellop", "addeff")))
        crect = pygame.Rect(fx+fw-90, fy+fh-36, 78, 26)
        pygame.draw.rect(screen, (60,66,80), crect, border_radius=5)
        pygame.draw.rect(screen, (150,190,220), crect, 1, border_radius=5)
        screen.blit(self.sfont.render("Close", True, (235,235,240)), (crect.x+20, crect.y+5))
        self.data_rects.append((crect, ("spellop", "close")))
        pal = "13 effects · target enemy/ally/all/self · area/bolt/chain · +delay/vfx/hp-cond · vfx: fire ice holy shadow nature arcane water gold blood"
        screen.blit(self.sfont.render(pal, True, (140,150,160)), (fx+16, fy+fh-58))

    def _draw_canvas(self, screen):
        import pygame
        c = self.cvx()
        prev = screen.get_clip(); screen.set_clip(c)
        # ground
        th = self.m.get("theme", THEME_PRESETS["parchment"])
        pygame.draw.rect(screen, self._safe_col(th.get("ground"), (196, 178, 148)), c)
        # map border
        ox, oy = self.w2s(0, 0)
        w, h = self.m["size"]; ex, ey = self.w2s(w, h)
        pygame.draw.rect(screen, (90, 80, 62), (ox, oy, ex - ox, ey - oy), 2)
        # grid
        gstep = 400
        gx = 0
        while gx <= w:
            a = self.w2s(gx, 0); b = self.w2s(gx, h)
            pygame.draw.line(screen, tuple(th["grid"]), a, b); gx += gstep
        gy = 0
        while gy <= h:
            a = self.w2s(0, gy); b = self.w2s(w, gy)
            pygame.draw.line(screen, tuple(th["grid"]), a, b); gy += gstep
        # bands
        for (x1, y1, x2, y2, bw) in self.m.get("bands", []):
            a = self.w2s(x1, y1); b = self.w2s(x2, y2)
            pygame.draw.line(screen, tuple(int(v*0.9) for v in th["ground"]), a, b,
                             max(2, int(bw * self.zoom)))
        # high ground
        for (x, y, r) in self.m.get("highground", []):
            sx, sy = self.w2s(x, y)
            pygame.draw.circle(screen, tuple(min(255, v+16) for v in th["ground"]),
                               (int(sx), int(sy)), int(r*self.zoom))
            pygame.draw.circle(screen, (176,158,128), (int(sx), int(sy)), int(r*self.zoom), 1)
        # pools
        for (x, y, r) in self.m.get("pools", []):
            sx, sy = self.w2s(x, y)
            pygame.draw.circle(screen, (44,40,66), (int(sx), int(sy)), int(r*self.zoom))
            pygame.draw.circle(screen, (90,84,140), (int(sx), int(sy)), int(r*self.zoom), 1)
        # stains
        for (x, y, r, a) in self.m.get("stains", []):
            sx, sy = self.w2s(x, y)
            surf = pygame.Surface((int(r*self.zoom*2)+2, int(r*self.zoom*2)+2), pygame.SRCALPHA)
            rr = int(r*self.zoom)
            pygame.draw.circle(surf, (*th.get("accent",[150,120,80]), int(a*255)), (rr+1, rr+1), rr)
            screen.blit(surf, (sx-rr, sy-rr))
        # objects
        for o in self.m.get("objects", []):
            sx, sy = self.w2s(o["x"], o["y"]); rr = int(o.get("r", 40)*self.zoom)
            pygame.draw.circle(screen, tuple(o.get("color", [140,130,110])), (int(sx), int(sy)), max(2, rr))
            pygame.draw.circle(screen, (40,34,28), (int(sx), int(sy)), max(2, rr), 1)
            if rr > 8:
                lbl = self.sfont.render(o["kind"], True, (40,34,28))
                screen.blit(lbl, (sx - lbl.get_width()/2, sy - 6))
        # named regions (first-class, referenced by triggers)
        for rg in self.m.get("regions", []):
            sx, sy = self.w2s(rg["x"], rg["y"]); rr = int(rg["r"] * self.zoom)
            reg = pygame.Surface((rr * 2 + 2, rr * 2 + 2), pygame.SRCALPHA)
            pygame.draw.circle(reg, (120, 200, 160, 40), (rr + 1, rr + 1), rr)
            pygame.draw.circle(reg, (120, 210, 170, 180), (rr + 1, rr + 1), rr, 1)
            screen.blit(reg, (sx - rr - 1, sy - rr - 1))
            lbl = self.sfont.render(rg["name"], True, (150, 220, 180))
            screen.blit(lbl, (sx - lbl.get_width() / 2, sy - rr - 14))
        # symmetry axis guide
        if self.symmetry != "none":
            w, h = self.m["size"]
            if self.symmetry in ("mirror_x", "rotate"):
                a = self.w2s(w / 2, 0); b = self.w2s(w / 2, h)
                pygame.draw.line(screen, (230, 180, 90), a, b, 1)
            if self.symmetry in ("mirror_y", "rotate"):
                a = self.w2s(0, h / 2); b = self.w2s(w, h / 2)
                pygame.draw.line(screen, (230, 180, 90), a, b, 1)
        # event regions (enter_region triggers) as dashed rings
        for e in self.m.get("events", []):
            tr = e.get("trigger", {})
            if tr.get("type") == "enter_region":
                rx, ry, rr = tr.get("region", [0,0,200])
                sx, sy = self.w2s(rx, ry)
                pygame.draw.circle(screen, (230,180,90), (int(sx),int(sy)), int(rr*self.zoom), 1)
        # spawns
        for i, sp in enumerate(self.m.get("spawns", [])):
            sx, sy = self.w2s(sp[0], sp[1])
            col = (90,150,220) if i == 0 else (216,100,92)
            pygame.draw.circle(screen, col, (int(sx), int(sy)), 9)
            pygame.draw.circle(screen, (240,240,230), (int(sx), int(sy)), 9, 2)
            lbl = self.sfont.render("A" if i==0 else "B", True, (255,255,255))
            screen.blit(lbl, (sx - lbl.get_width()/2, sy - 8))
        # band anchor preview
        if self.band_anchor:
            sx, sy = self.w2s(*self.band_anchor)
            pygame.draw.circle(screen, (230,220,120), (int(sx),int(sy)), 4)
        # brush ghost for radius tools
        import pygame as pg
        mx, my = pg.mouse.get_pos()
        if self.tool in ("high","pool","stain") and c.collidepoint(mx,my):
            pygame.draw.circle(screen, (240,230,120), (mx,my),
                               int((self.brush_r if self.tool!='stain' else self.brush_r*0.2)*self.zoom), 1)
        # selection highlight
        sel = self._sel_item()
        if sel:
            cat, i, it = sel
            if cat == "objects": x,y,r = it["x"], it["y"], it.get("r",40)
            elif cat == "spawns": x,y,r = it[0], it[1], 120
            elif cat in ("highground","pools"): x,y,r = it[0],it[1],it[2]
            elif cat == "stains": x,y,r = it[0],it[1],it[2]
            elif cat == "regions": x,y,r = it["x"], it["y"], it["r"]
            else: x,y,r = 0,0,0
            sx, sy = self.w2s(x, y)
            pygame.draw.circle(screen, (245,235,120), (int(sx),int(sy)), int(r*self.zoom)+3, 2)
        screen.set_clip(prev)
        # balance-analysis overlay + symmetry/snap status (top-left of canvas)
        import pygame
        if self.show_analysis:
            ax, ay = c.x + 8, c.y + 6
            panel = pygame.Surface((190, 92), pygame.SRCALPHA); panel.fill((20, 18, 14, 170))
            screen.blit(panel, (ax - 4, ay - 4))
            self._draw_analysis(screen, ax, ay)
        stat = f"sym:{self.symmetry}  snap:{self.snap or 'off'}  (M/G)"
        screen.blit(self.sfont.render(stat, True, (200, 180, 120)),
                    (c.x + 8, c.bottom - 18))

    def _draw_toolbar(self, screen):
        import pygame
        self.tool_rects = []
        pygame.draw.rect(screen, (34,30,24), (0, TOPBAR_H, TOOLBAR_W, self.H-TOPBAR_H))
        y = TOPBAR_H + 8
        for label, tid in TOOLS:
            rect = pygame.Rect(8, y, TOOLBAR_W-16, 30)
            active = self.tool == tid
            pygame.draw.rect(screen, (86,66,34) if active else (52,46,38), rect, border_radius=5)
            pygame.draw.rect(screen, (220,180,90) if active else (90,82,70), rect, 1, border_radius=5)
            screen.blit(self.sfont.render(label, True, (232,224,208)), (rect.x+8, rect.y+8))
            self.tool_rects.append((rect, tid))
            y += 34
        # brush size hint
        screen.blit(self.sfont.render(f"brush [ ]: {self.brush_r}", True, (170,160,140)),
                    (10, y+6))

    def _draw_topbar(self, screen):
        import pygame
        self.top_rects = []
        pygame.draw.rect(screen, (40,34,26), (0,0,self.W,TOPBAR_H))
        screen.blit(self.bfont.render("Foldworks Map Editor", True, (224,196,120)), (12, 8))
        # map selector + buttons on the right
        x = self.W - 12
        def btn(label, kind, w=90, col=(60,52,40)):
            nonlocal x
            x -= w + 6
            rect = pygame.Rect(x, 8, w, 28)
            hover = rect.collidepoint(pygame.mouse.get_pos())
            pygame.draw.rect(screen, (80,68,54) if hover else col, rect, border_radius=5)
            pygame.draw.rect(screen, (200,170,110), rect, 1, border_radius=5)
            screen.blit(self.sfont.render(label, True, (235,228,212)),
                        (rect.centerx - self.sfont.size(label)[0]//2, rect.y+7))
            self.top_rects.append((rect, kind))
        btn("Validate", "validate"); btn("Export", "export")
        btn("Save", "save", col=(86,66,34)); btn("Test ▶", "test", col=(50,74,40))
        btn(f"◆ {self.mode.title()}", "mode", w=76, col=(44,54,70))
        btn("⊞", "win_sections", w=28, col=(44,54,70))
        btn("Models", "win_models", w=60, col=(44,54,70))
        btn("Assets", "win_assets", w=60, col=(44,54,70))
        btn("Trig", "win_triggers", w=46, col=(44,54,70))
        btn("New", "new"); btn("Dup", "dup", w=54); btn("Del", "del", w=54)
        btn("Weather", "weather"); btn("Theme", "theme")
        btn(f"Sym:{self.symmetry[:4]}", "sym", w=76)
        btn("▶", "nextmap", w=30)
        # map name label
        namew = 150
        x -= namew + 6
        rect = pygame.Rect(x, 8, namew, 28)
        pygame.draw.rect(screen, (30,26,20), rect, border_radius=5)
        screen.blit(self.sfont.render(self.map_id, True, (150,200,235)),
                    (rect.centerx - self.sfont.size(self.map_id)[0]//2, rect.y+7))
        self.top_rects.append((rect, "nextmap"))
        x -= 36
        prect = pygame.Rect(x, 8, 30, 28)
        pygame.draw.rect(screen, (60,52,40), prect, border_radius=5)
        pygame.draw.rect(screen, (200,170,110), prect, 1, border_radius=5)
        screen.blit(self.sfont.render("◀", True, (235,228,212)), (prect.x+9, prect.y+7))
        self.top_rects.append((prect, "prevmap"))

    def _draw_panel(self, screen):
        import pygame
        self.field_rects = []
        px = self.W - PANEL_W
        pygame.draw.rect(screen, (34,30,24), (px, TOPBAR_H, PANEL_W, self.H-TOPBAR_H-BOTTOM_H))
        y = TOPBAR_H + 10
        def header(t):
            nonlocal y
            screen.blit(self.font.render(t, True, (200,180,120)), (px+12, y)); y += 24
        def field(label, owner, key, valfmt=None):
            nonlocal y
            screen.blit(self.sfont.render(label, True, (190,182,166)), (px+12, y+4))
            rect = pygame.Rect(px+120, y, PANEL_W-132, 22)
            active = self.active_field == (owner, key)
            pygame.draw.rect(screen, (48,42,34) if not active else (70,60,44), rect, border_radius=4)
            pygame.draw.rect(screen, (220,180,90) if active else (80,72,60), rect, 1, border_radius=4)
            if active:
                shown = self.buffer + "|"
            else:
                v = owner.get(key, "")
                shown = valfmt(v) if valfmt else str(v)
            screen.blit(self.sfont.render(shown[:26], True, (235,228,212)), (rect.x+5, rect.y+4))
            self.field_rects.append((rect, owner, key))
            y += 26
        # map data
        header("Map Data")
        field("Name", self.m, "name")
        field("Size", self.m, "size", valfmt=lambda v: f"{v[0]}x{v[1]}")
        field("Seed", self.m, "seed")
        field("Desc", self.m, "desc")
        wk = self.m.get("weather", {}).get("kind", "?")
        screen.blit(self.sfont.render(f"Weather: {wk}  ·  cycle in top bar",
                    True, (150,140,124)), (px+12, y)); y += 22
        # editable map textures — the theme palette (RGB as "r,g,b")
        th = self.m.setdefault("theme", {})
        screen.blit(self.sfont.render("Texture colours (r,g,b):", True, (200,180,120)), (px+12, y)); y += 22
        for ck in ("ground", "grid", "ruled", "crease", "accent"):
            if ck not in th: continue
            screen.blit(self.sfont.render(ck, True, (190,182,166)), (px+12, y+4))
            rect = pygame.Rect(px+90, y, PANEL_W-150, 22)
            active = self.active_field == (th, ck)
            pygame.draw.rect(screen, (48,42,34) if not active else (70,60,44), rect, border_radius=4)
            pygame.draw.rect(screen, (220,180,90) if active else (80,72,60), rect, 1, border_radius=4)
            col = th[ck]
            shown = (self.buffer + "|") if active else ",".join(str(c) for c in col)
            screen.blit(self.sfont.render(shown[:20], True, (235,228,212)), (rect.x+5, rect.y+4))
            self.field_rects.append((rect, th, ck))
            # colour swatch
            try: pygame.draw.rect(screen, tuple(col), (px+PANEL_W-52, y, 40, 20))
            except Exception: pass
            y += 26
        # selected item
        y += 6
        pygame.draw.line(screen, (70,62,50), (px+8, y), (px+PANEL_W-8, y)); y += 8
        header("Selection")
        sel = self._sel_item()
        if not sel:
            screen.blit(self.sfont.render("Nothing selected.", True, (150,140,124)), (px+12, y)); y+=20
            screen.blit(self.sfont.render("Pick a tool, click the canvas.", True, (150,140,124)), (px+12, y))
        else:
            cat, i, it = sel
            screen.blit(self.sfont.render(f"{cat}[{i}]", True, (150,200,235)), (px+12, y)); y += 22
            if cat == "objects":
                field("Label", it, "label")
                field("x", it, "x"); field("y", it, "y"); field("r", it, "r")
                if "amount" in it: field("amount", it, "amount")
            elif cat in ("highground", "pools"):
                # edit x,y,r of the list-of-3 by wrapping in a proxy
                self._list3_fields(field, cat, i)
            elif cat == "spawns":
                screen.blit(self.sfont.render(f"({it[0]}, {it[1]})", True, (235,228,212)), (px+12, y)); y+=20
            elif cat == "stains":
                screen.blit(self.sfont.render(f"blot r={it[2]:.0f} a={it[3]:.2f}",
                            True, (235,228,212)), (px+12, y)); y+=20

    def _list3_fields(self, field, cat, i):
        # present x/y/r of a [x,y,r] entry via a tiny dict proxy synced back
        lst = self.m[cat][i]
        proxy = getattr(self, "_proxy", None)
        if not proxy or proxy.get("_cat") != cat or proxy.get("_i") != i:
            proxy = dict(_cat=cat, _i=i, x=lst[0], y=lst[1], r=lst[2]); self._proxy = proxy
        field("x", proxy, "x"); field("y", proxy, "y"); field("r", proxy, "r")
        # sync proxy → list
        lst[0], lst[1], lst[2] = int(proxy["x"]), int(proxy["y"]), int(proxy["r"])

    def _draw_events(self, screen):
        import pygame
        self.event_rects = []
        by = self.H - BOTTOM_H
        pygame.draw.rect(screen, (30,27,22), (0, by, self.W, BOTTOM_H))
        pygame.draw.line(screen, (70,62,50), (0, by), (self.W, by))
        screen.blit(self.font.render("Event Scripts", True, (200,180,120)), (12, by+6))
        evs = self.m.get("events", [])
        # list on the left
        lx = 12; ly = by + 30
        for i, e in enumerate(evs[:5]):
            rect = pygame.Rect(lx, ly, 300, 20)
            sel = e is self.editing_event
            pygame.draw.rect(screen, (66,54,36) if sel else (44,38,30), rect, border_radius=3)
            tt = e.get("trigger",{}).get("type","?"); at = (e.get("actions",[{}])[0]).get("type","?")
            screen.blit(self.sfont.render(f"{e.get('id','?')}: {tt} → {at}", True, (232,224,208)),
                        (rect.x+6, rect.y+3))
            self.event_rects.append((rect, f"sel:{i}"))
            ly += 22
        # buttons
        def ebtn(label, act, bx, by2, w=110):
            rect = pygame.Rect(bx, by2, w, 24)
            pygame.draw.rect(screen, (60,52,40), rect, border_radius=4)
            pygame.draw.rect(screen, (200,170,110), rect, 1, border_radius=4)
            screen.blit(self.sfont.render(label, True, (235,228,212)), (rect.x+8, rect.y+5))
            self.event_rects.append((rect, act))
        ebtn("+ Add Event", "add", 330, by+30)
        ebtn("Delete", "del", 330, by+58)
        # editor for the selected event on the right (Event → Conditions → Actions)
        ex = 460
        if self.editing_event:
            e = self.editing_event
            tr = e.get("trigger", {}); a0 = (e.get("actions") or [{}])[0]
            conds = e.get("conditions", [])
            screen.blit(self.sfont.render("When:", True, (190,182,166)), (ex, by+28))
            ebtn(tr.get("type","time"), "cycle_trigger", ex+52, by+25, w=140)
            screen.blit(self.sfont.render(_trigger_desc(tr), True, (150,190,220)), (ex+200, by+30))
            screen.blit(self.sfont.render("If:", True, (190,182,166)), (ex, by+54))
            ctxt = _cond_desc(conds[0]) if conds else "(always)"
            ebtn("condition", "cycle_cond", ex+52, by+51, w=90)
            ebtn("+", "add_cond", ex+146, by+51, w=24)
            ebtn("×", "clear_cond", ex+174, by+51, w=24)
            screen.blit(self.sfont.render(ctxt, True, (150,220,180)), (ex+206, by+56))
            screen.blit(self.sfont.render("Do:", True, (190,182,166)), (ex, by+80))
            ebtn(a0.get("type","message"), "cycle_action", ex+52, by+77, w=140)
            screen.blit(self.sfont.render(_action_desc(a0), True, (220,190,150)), (ex+200, by+82))
            ebtn("Edit params…", "params", ex, by+104, w=120)
        # status line
        if self.status_t > 0:
            screen.blit(self.sfont.render(self.status, True, (245,220,140)),
                        (12, self.H-20))


def _default_entry(cat, key):
    """A sensible template when authoring a new unit / structure / upgrade."""
    if cat == "units":
        return dict(name="Custom Unit", cost=60, hp=140, dmg=15, spd=88, rng=16,
                    r=13, card="gorilla", build_t=5.0, tier=1, key=key[:1].upper(),
                    desc="A custom unit.", supply=2)
    if cat == "structures":
        return dict(name="Custom Structure", cost=140, hp=800, r=42, build=9.0,
                    makes=[], requires=[], researches=[], tier=1,
                    key=key[:1].upper(), desc="A custom structure.")
    return dict(name="Custom Upgrade", cost=120, time=20, stat="dmg", mult=1.2,
                applies="army", desc="A custom upgrade.")


def _default_ability():
    """A caster ability built from composable effect primitives."""
    return dict(name="Custom Spell", radius=160, cast_range=520, dur=3.0, cost=40,
                cooldown=0.0, delivery="area", sfx="whoosh", desc="A custom ability.",
                effects=[dict(effect="damage", amount=60, dot=True)])


def _default_effect(kind):
    e = dict(effect=kind)
    e.update(copy.deepcopy(EFFECT_PRIMITIVES.get(kind, {})))
    # default targeting mode by effect (editable in the composer)
    e["target"] = "ally" if kind in ("heal", "buff", "shield") else \
                  "self" if kind == "reveal" else "enemy"
    if kind in ("spawn", "reveal"):
        e.pop("target", None)               # these don't target units
    return e


def _default_trigger(t):
    if t == "time": return dict(type="time", at=30)
    if t == "unit_count": return dict(type="unit_count", team=1, op=">=", n=6)
    if t == "enter_region": return dict(type="enter_region", region=[2000,1500,300], team=0)
    if t == "structure_destroyed": return dict(type="structure_destroyed", team=1, kind="scriptorium")
    return dict(type="time", at=30)

def _default_action(a):
    if a == "spawn": return dict(type="spawn", team=1, unit="soldier", x=2000, y=1500, count=3)
    if a == "message": return dict(type="message", text="Something happens!")
    if a == "ink": return dict(type="ink", team=0, amount=200)
    if a == "reveal": return dict(type="reveal", x=2000, y=1500, r=400)
    return dict(type="message", text="...")

def _default_condition(c):
    if c == "time_after": return dict(type="time_after", at=60)
    if c == "time_before": return dict(type="time_before", at=180)
    if c == "ink_at_least": return dict(type="ink_at_least", team=0, amount=300)
    if c == "unit_count": return dict(type="unit_count", team=0, op=">=", n=5)
    return dict(type="time_after", at=60)

def _cond_desc(c):
    t = c.get("type")
    if t == "time_after": return f"time ≥ {c.get('at')}s"
    if t == "time_before": return f"time ≤ {c.get('at')}s"
    if t == "ink_at_least": return f"team {c.get('team')} ink ≥ {c.get('amount')}"
    if t == "unit_count": return f"team {c.get('team')} units {c.get('op')} {c.get('n')}"
    return ""

def _trigger_desc(tr):
    t = tr.get("type")
    if t == "time": return f"at {tr.get('at',0)}s"
    if t == "unit_count": return f"team {tr.get('team')} units {tr.get('op')} {tr.get('n')}"
    if t == "enter_region": return f"team {tr.get('team')} enters {tr.get('region')}"
    if t == "structure_destroyed": return f"team {tr.get('team')} loses {tr.get('kind')}"
    return ""

def _action_desc(a):
    t = a.get("type")
    if t == "spawn": return f"spawn {a.get('count')}× {a.get('unit')} (team {a.get('team')})"
    if t == "message": return f'say "{a.get("text","")[:24]}"'
    if t == "ink": return f"+{a.get('amount')} ink to team {a.get('team')}"
    if t == "reveal": return f"reveal r={a.get('r')} at ({a.get('x')},{a.get('y')})"
    return ""


# ─────────────────────────────────────────────────────────────────────────────
# Headless self-test — proves the data pipeline end to end without a GUI
# ─────────────────────────────────────────────────────────────────────────────

def selftest():
    ok = True
    data = load_data()
    assert "maps" in data and data["maps"], "no maps in data"
    # build a fresh map programmatically the way the editor would
    m = new_map("editor_test")
    m["name"] = "Editor Test"; m["size"] = [4200, 2800]; m["seed"] = 42
    m["spawns"] = [[400, 2400], [3800, 400]]
    m["highground"].append([2100, 1400, 260])
    m["pools"].append([1500, 1500, 190])
    m["stains"].append([900, 900, 40, 0.12])
    m["bands"].append([0, 900, 4200, 1200, 100])
    m["objects"].append(dict(kind="ruin", x=2100, y=1400, r=70,
                             blocks=True, color=[110,100,86], label="Test Ruin"))
    m["objects"].append(dict(kind="richnode", x=1500, y=700, r=40,
                             color=[120,108,190], amount=4000, label="Well"))
    # an event script with a CONDITION (Event → Conditions → Actions)
    m["events"].append(dict(id="e1", trigger=dict(type="time", at=45),
                            conditions=[dict(type="time_after", at=40)],
                            actions=[dict(type="spawn", team=1, unit="soldier",
                                          x=2000, y=1500, count=4),
                                     dict(type="message", text="Reinforcements!")]))
    m["events"].append(dict(id="e2", trigger=dict(type="enter_region",
                            region=[2100,1400,300], team=0),
                            actions=[dict(type="ink", team=0, amount=250)]))
    # named regions + rotational symmetry check
    m["regions"] = [dict(name="mid", x=2100, y=1400, r=300)]
    errs = validate_map(m)
    print("validate:", "OK" if not errs else errs)
    ok = ok and not errs
    # round-trip through JSON
    s = json.dumps(m); m2 = json.loads(s)
    assert m2 == m, "JSON round-trip changed the map"
    # write into a copy of the data and confirm it reloads & the RTS accepts it
    data["maps"]["editor_test"] = m
    tmp = os.path.join(HERE, "_editor_selftest.json")
    with open(tmp, "w") as f: json.dump(data, f)
    with open(tmp) as f: reloaded = json.load(f)
    assert "editor_test" in reloaded["maps"]
    os.remove(tmp)
    # verify the RTS can load a map dict with these fields (import + build)
    try:
        os.environ.setdefault("SDL_VIDEODRIVER", "dummy")
        os.environ.setdefault("SDL_AUDIODRIVER", "dummy")
        import pygame, rts
        pygame.init(); pygame.display.set_mode((320, 240))
        rts.MAPS["editor_test"] = m
        rts._MAP_OBJECTS["editor_test"] = list(m["objects"])
        rts._MAP_HIGH["editor_test"] = [tuple(h) for h in m["highground"]]
        f = pygame.font.SysFont("georgia", 14)
        ctx = dict(W=1280, H=800, font=f, sfont=f, bfont=f, draw_unit=None, cards={})
        r = rts.RTS(ctx, map_id="editor_test", factions=["court", "ember"])
        for _ in range(30): r.update(1/30)
        assert r.events_loaded_count() >= 2, "RTS didn't load the events"
        print("RTS loaded editor map + events:", r.events_loaded_count(), "events")
        # a CONDITION should gate a trigger: condition time_after=999 blocks it
        r2 = rts.RTS(ctx, map_id="editor_test", factions=["court", "ember"])
        r2.events = [dict(id="c", trigger=dict(type="time", at=1),
                          conditions=[dict(type="time_after", at=999)],
                          actions=[dict(type="ink", team=0, amount=500)], _fired=False)]
        ink0 = r2.ink[0]
        for _ in range(60): r2.update(1/30)
        assert r2.ink[0] == ink0, "condition failed to gate the event"
        print("condition gating works (blocked event did not fire)")
    except Exception as ex:
        print("RTS integration check skipped/failed:", ex)
        ok = False
    # ── Data Editor round-trip: author a custom unit + spell, run it live ──
    try:
        ed = _EditorState(1280, 800, f, f, f)
        ed.data_cat = "units"
        fac0 = ed.data_fac
        before = len(ed.data["factions"][fac0]["units"])
        ed._data_entry_op("add")                       # create a custom unit
        newk = ed.data_sel
        assert newk and len(ed.data["factions"][fac0]["units"]) == before + 1
        spec = ed.data["factions"][fac0]["units"][newk]
        # edit a numeric stat through the exact commit path the UI uses
        ed.active_field = (spec, "hp"); ed.buffer = "222"; ed._commit_field()
        assert spec["hp"] == 222 and isinstance(spec["hp"], int), spec["hp"]
        # attach an ability and compose a spell from primitives
        ed.active_field = (spec, "__newfield__"); ed.buffer = "ability"; ed._commit_field()
        assert isinstance(spec.get("ability"), dict), "ability not added"
        ed.spell_edit = spec["ability"]
        ed._spell_op("addeff")                         # add a 2nd effect
        ed._spell_op(("cycle", 1))                     # cycle its primitive
        for e in spec["ability"]["effects"]:
            assert e["effect"] in rts.EFFECT_PRIMITIVES, e["effect"]
        # duplicate + delete exercise the other entry ops
        ed._data_entry_op("dup"); assert ed.data_sel != newk
        ed._data_entry_op("del")
        ed.data_sel = newk
        # push into the engine exactly like _launch_test and confirm it's live
        rts.FACTIONS = copy.deepcopy(ed.data["factions"])
        rts.UNITS = rts.FACTIONS[fac0]["units"]
        rts.STRUCTURES = rts.FACTIONS[fac0]["structures"]
        other = next((x for x in rts.FACTIONS if x != fac0), fac0)
        r3 = rts.RTS(ctx, map_id="editor_test", factions=[fac0, other])
        assert r3._factbook()[fac0]["units"][newk]["hp"] == 222, "edit not live in engine"
        # spawn the custom unit and run — proves the engine builds & steps it
        r3.units.append(rts.Unit(newk, 500, 2300, 0, spec=r3.uspec(0, newk)))
        for _ in range(20): r3.update(1/30)
        print("data-editor round-trip OK: custom unit hp=222, spell effects valid, engine ran it")
    except Exception as ex:
        import traceback; traceback.print_exc()
        print("data-editor check failed:", ex); ok = False

    # symmetry helper: rotational mirror of a point round-trips
    import types
    ed = types.SimpleNamespace(m=dict(size=[4000, 3000]), symmetry="rotate")
    mx, my = _EditorState._mirror(ed, 1000, 800)
    assert (mx, my) == (3000, 2200), f"bad mirror {(mx,my)}"
    print("rotational symmetry mirror OK:", (mx, my))
    print("✅ editor self-test passed" if ok else "❌ editor self-test FAILED")
    return ok


if __name__ == "__main__":
    if "--selftest" in sys.argv:
        sys.exit(0 if selftest() else 1)
    elif "--shot" in sys.argv:
        i = sys.argv.index("--shot")
        out = sys.argv[i+1] if i+1 < len(sys.argv) else "editor.png"
        run_editor(headless_shot=out)
        print("wrote", out)
    else:
        run_editor()
