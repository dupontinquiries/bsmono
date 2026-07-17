"""
Paperbound Pack Editor — author expansion packs (cards + their origami models).

The RTS side already has rts_editor.py; this is the battler side: it edits the
`expansions/<pack>.json` documents that expansions.py merges into the game —
card stats, ability, rarity, paper colour, and the folded `model` part list
that game.py's origami renderer draws. A live preview approximates the game's
paper style so you can design a card's look without launching a match.

Run:
    python pack_editor.py                          # edit expansions/dragon_legion.json
    python pack_editor.py expansions/mypack.json   # edit / create a pack
    python pack_editor.py --selftest               # headless validation

Controls:
    Left list        — click a card to select;  N new card;  Del deletes
    Middle panel     — click a stat chip, type, Enter commits (Esc cancels)
                       F / B toggle flying / building;  A / Shift+A cycle ability
                       R cycles rarity;  ← → adjust paper hue
    Model (right)    — click a part to select;  drag moves it
                       P cycles the selected part's kind;  + / - scales it
                       , . rotate (ang);  [ ] tone down / up
                       Ins adds a part;  Del (with a part selected) removes it
    Ctrl+S           — save;  V validate;  ESC quit

Everything writes plain JSON — the pack format is deliberately hand-editable
and LLM-authorable (names not IDs, defaults for everything), in the same
spirit as soldier's buildscript and the arena forge.
"""

import os, sys, json, math, copy

HERE = os.path.dirname(os.path.abspath(__file__))
DEFAULT_PACK = os.path.join(HERE, "expansions", "dragon_legion.json")

def _part_vocabulary():
    """The full part list: origami's built-ins + any JSON-defined parts, with
    the JSON templates returned too so the preview can fold them accurately."""
    builtins = ["slab", "wing", "leg", "wedge", "tail", "crest", "spine",
                "panel", "shell", "orb", "tusk"]
    try:
        import origami
        extra = [p for p in origami.json_part_names() if p not in builtins]
        templates = {n: origami._JSON_PARTS[n] for n in extra}
        return builtins + extra, templates
    except Exception:
        return builtins, {}

PART_KINDS, JSON_PART_TEMPLATES = _part_vocabulary()
RARITIES = ["common", "rare", "epic", "legendary"]

STAT_FIELDS = [("cost", int, 1, 10), ("hp", int, 10, 6000), ("dmg", int, 0, 400),
               ("spd", int, 0, 140), ("rng", int, 0, 300), ("size", int, 10, 48),
               ("count", int, 1, 6)]


def load_abilities():
    """Ability ids from cards.json (so validation matches the live game)."""
    try:
        with open(os.path.join(HERE, "cards.json")) as f:
            return sorted(json.load(f).get("abilities", {}).keys())
    except Exception:
        return ["taunt", "armored", "sharp"]


def new_card(key="newcard"):
    return {
        "id": key, "name": key.title(), "cost": 3, "hp": 400, "dmg": 60,
        "spd": 50, "rng": 24, "size": 24, "flying": False, "building": False,
        "riverhop": False, "count": 1, "paper": [180, 160, 130],
        "ability": "taunt", "rarity": "common", "rotation": True, "blurb": "",
        "model": [{"part": "slab", "at": [0, 0.05], "size": 0.85, "h": 0.45, "tone": 1.0}],
    }


def new_pack(pid="newpack"):
    return {"id": pid, "name": pid.replace("_", " ").title(),
            "blurb": "", "cards": {}, "rts_maps": {}}


# ── validation ───────────────────────────────────────────────────────────────

def validate_card(key, card, abilities):
    probs = []
    for f, typ, lo, hi in STAT_FIELDS:
        v = card.get(f)
        if not isinstance(v, (int, float)) or not (lo <= v <= hi):
            probs.append(f"{key}.{f}={v!r} outside {lo}..{hi}")
    if card.get("ability") not in abilities:
        probs.append(f"{key}.ability '{card.get('ability')}' unknown to cards.json")
    if card.get("rarity") not in RARITIES:
        probs.append(f"{key}.rarity '{card.get('rarity')}' not in {RARITIES}")
    p = card.get("paper")
    if not (isinstance(p, list) and len(p) == 3 and all(0 <= c <= 255 for c in p)):
        probs.append(f"{key}.paper must be [r,g,b]")
    if not card.get("model"):
        probs.append(f"{key}.model is empty — the card would be invisible")
    for i, part in enumerate(card.get("model", [])):
        if part.get("part") not in PART_KINDS:
            probs.append(f"{key}.model[{i}].part '{part.get('part')}' unknown")
        at = part.get("at", [0, 0])
        if not (isinstance(at, list) and len(at) == 2):
            probs.append(f"{key}.model[{i}].at must be [x,y]")
    if card.get("building") and card.get("spd", 0) != 0:
        probs.append(f"{key}: buildings should have spd 0")
    return probs


def validate_pack(doc, abilities):
    probs = []
    if not doc.get("id"):
        probs.append("pack has no id")
    for key, card in (doc.get("cards") or {}).items():
        probs += validate_card(key, card, abilities)
    return probs


# ── pack I/O ─────────────────────────────────────────────────────────────────

def load_pack(path):
    with open(path) as f:
        doc = json.load(f)
    doc.setdefault("cards", {})
    doc.setdefault("rts_maps", {})
    return doc


def save_pack(doc, path):
    tmp = path + ".tmp"
    with open(tmp, "w") as f:
        json.dump(doc, f, indent=2)
    os.replace(tmp, path)


# ── model preview (approximates game.py's paper look, self-contained) ────────

def draw_model_preview(pygame, surf, rect, card, sel_part=None):
    """A stylised preview: each part becomes a shaded paper polygon around the
    card's centre, matching the proportions the game reads (at/size/ang/tone)."""
    cx, cy = rect.center
    scale = min(rect.w, rect.h) * 0.28
    paper = tuple(card.get("paper", [180, 160, 130]))

    def shade(c, f):
        return tuple(max(0, min(255, int(x * f))) for x in c)

    pygame.draw.rect(surf, (238, 232, 218), rect, border_radius=10)
    pygame.draw.rect(surf, (90, 80, 64), rect, 2, border_radius=10)
    # a soft drop shadow under the piece
    sh = pygame.Surface(rect.size, pygame.SRCALPHA)
    pygame.draw.ellipse(sh, (40, 34, 28, 60),
                        (rect.w * 0.2, rect.h * 0.68, rect.w * 0.6, rect.h * 0.16))
    surf.blit(sh, rect.topleft)

    def poly_for(part):
        kind = part.get("part", "slab")
        ax, ay = part.get("at", [0, 0])
        s = part.get("size", 0.8) * scale
        ang = part.get("ang", 0.0)
        h = part.get("h", 0.4)
        px, py = cx + ax * scale, cy + ay * scale
        ca, sa = math.cos(ang), math.sin(ang)

        def rot(pts):
            return [(px + x * ca - y * sa, py + x * sa + y * ca) for x, y in pts]
        if kind == "slab":
            w, hh = s, s * max(0.2, h)
            return rot([(-w, -hh), (w, -hh), (w * 0.85, hh), (-w * 0.85, hh)])
        if kind == "wing":
            return rot([(0, 0), (s, -s * 0.55), (s * 0.55, s * 0.1)])
        if kind == "leg":
            w, hh = s * 0.3, s * max(0.3, h)
            return rot([(-w, 0), (w, 0), (w * 0.7, hh), (-w * 0.7, hh)])
        if kind == "wedge":
            return rot([(0, -s * 0.35), (s, 0), (0, s * 0.35)])
        if kind == "tail":
            return rot([(0, -s * 0.18), (-s, 0), (0, s * 0.18)])
        if kind == "crest":
            return rot([(-s * 0.4, 0), (0, -s), (s * 0.4, 0)])
        if kind == "spine":
            w = s * 0.12
            return rot([(-w, -s), (w, -s), (w, s * 0.2), (-w, s * 0.2)])
        if kind == "panel":
            return rot([(-s * 0.55, -s * 0.75), (s * 0.55, -s * 0.75),
                        (s * 0.55, s * 0.75), (-s * 0.55, s * 0.75)])
        if kind == "shell":
            steps = 8
            return rot([(math.cos(math.pi * i / steps) * s,
                         -math.sin(math.pi * i / steps) * s * 0.75)
                        for i in range(steps + 1)] + [(s, 0), (-s, 0)])
        if kind == "orb":
            steps = 10
            return rot([(math.cos(2 * math.pi * i / steps) * s * 0.5,
                         math.sin(2 * math.pi * i / steps) * s * 0.5)
                        for i in range(steps)])
        if kind == "tusk":
            return rot([(0, -s * 0.12), (s, -s * 0.35), (s * 0.9, -s * 0.1), (0, s * 0.12)])
        if kind in JSON_PART_TEMPLATES:
            spec = JSON_PART_TEMPLATES[kind]
            if spec["kind"] == "disc":
                steps = 10; rr = spec.get("radius", 0.4) * s
                return rot([(math.cos(2 * math.pi * i / steps) * rr,
                             math.sin(2 * math.pi * i / steps) * rr)
                            for i in range(steps)])
            pts = [(px2 * s, py2 * s) for px2, py2 in spec["points"]]
            if spec.get("pair"):
                pts = pts + [(-px2, py2) for px2, py2 in reversed(pts)]
            return rot(pts)
        return rot([(-s * 0.4, -s * 0.4), (s * 0.4, -s * 0.4),
                    (s * 0.4, s * 0.4), (-s * 0.4, s * 0.4)])

    hits = []
    for i, part in enumerate(card.get("model", [])):
        tone = part.get("tone", 1.0)
        pts = poly_for(part)
        col = shade(paper, tone)
        pygame.draw.polygon(surf, col, pts)
        pygame.draw.polygon(surf, shade(paper, tone * 0.55), pts, 2)
        if i == sel_part:
            pygame.draw.polygon(surf, (240, 190, 80), pts, 3)
        xs = [p[0] for p in pts]; ys = [p[1] for p in pts]
        hits.append((pygame.Rect(min(xs), min(ys), max(xs) - min(xs) + 1,
                                 max(ys) - min(ys) + 1), i))
    return hits


# ── GUI ──────────────────────────────────────────────────────────────────────

def run_editor(path=DEFAULT_PACK):
    import pygame
    pygame.init()
    W, H = 1240, 800
    screen = pygame.display.set_mode((W, H), pygame.RESIZABLE)
    clock = pygame.time.Clock()
    font = pygame.font.Font(None, 22)
    sfont = pygame.font.Font(None, 17)
    abilities = load_abilities()

    doc = load_pack(path) if os.path.exists(path) else new_pack(
        os.path.splitext(os.path.basename(path))[0])
    keys = sorted(doc["cards"].keys())
    cur = keys[0] if keys else None
    sel_part = None
    editing = None              # (field, buffer)
    drag = None
    dirty = False
    status, status_t = "", 0.0
    stat_chips = []             # (rect, field)
    list_rows = []              # (rect, key)
    model_hits = []

    def flash(s):
        nonlocal status, status_t
        status, status_t = s, 2.6

    def mark():
        nonlocal dirty
        dirty = True

    def commit_edit():
        nonlocal editing
        if not editing or cur is None:
            return
        field, buf = editing
        card = doc["cards"][cur]
        try:
            if field == "name":
                card["name"] = buf
            elif field == "blurb":
                card["blurb"] = buf
            else:
                spec = next(s for s in STAT_FIELDS if s[0] == field)
                v = spec[1](float(buf))
                card[field] = max(spec[2], min(spec[3], v))
            mark()
        except Exception:
            flash(f"bad value for {field}")
        editing = None

    running = True
    while running:
        dt = clock.tick(60) / 1000.0
        status_t = max(0, status_t - dt)
        for e in pygame.event.get():
            if e.type == pygame.QUIT:
                running = False
            elif e.type == pygame.VIDEORESIZE:
                W, H = e.w, e.h
            elif e.type == pygame.KEYDOWN:
                mods = pygame.key.get_mods()
                if editing is not None:
                    if e.key == pygame.K_RETURN:
                        commit_edit()
                    elif e.key == pygame.K_ESCAPE:
                        editing = None
                    elif e.key == pygame.K_BACKSPACE:
                        editing = (editing[0], editing[1][:-1])
                    elif e.unicode and e.unicode.isprintable():
                        editing = (editing[0], editing[1] + e.unicode)
                    continue
                if e.key == pygame.K_ESCAPE:
                    running = False
                elif e.key == pygame.K_s and (mods & pygame.KMOD_CTRL):
                    save_pack(doc, path); dirty = False
                    flash(f"Saved → {path}")
                elif e.key == pygame.K_v:
                    probs = validate_pack(doc, abilities)
                    flash("Valid ✓" if not probs else f"{len(probs)} problems — first: {probs[0]}")
                elif e.key == pygame.K_n:
                    base = "newcard"; i = 1
                    while f"{base}{i}" in doc["cards"]: i += 1
                    k = f"{base}{i}"
                    doc["cards"][k] = new_card(k)
                    keys = sorted(doc["cards"]); cur = k; sel_part = None; mark()
                elif cur is None:
                    continue
                elif e.key == pygame.K_DELETE:
                    card = doc["cards"][cur]
                    if sel_part is not None and len(card["model"]) > 1:
                        card["model"].pop(sel_part); sel_part = None; mark()
                    elif sel_part is None:
                        del doc["cards"][cur]
                        keys = sorted(doc["cards"])
                        cur = keys[0] if keys else None
                        mark()
                elif e.key == pygame.K_INSERT:
                    doc["cards"][cur]["model"].append(
                        {"part": "wedge", "at": [0.4, -0.2], "size": 0.5, "tone": 1.1})
                    sel_part = len(doc["cards"][cur]["model"]) - 1; mark()
                elif e.key == pygame.K_f:
                    c = doc["cards"][cur]; c["flying"] = not c.get("flying"); mark()
                elif e.key == pygame.K_b:
                    c = doc["cards"][cur]; c["building"] = not c.get("building")
                    if c["building"]: c["spd"] = 0
                    mark()
                elif e.key == pygame.K_a:
                    c = doc["cards"][cur]
                    i = abilities.index(c["ability"]) if c["ability"] in abilities else 0
                    step = -1 if (mods & pygame.KMOD_SHIFT) else 1
                    c["ability"] = abilities[(i + step) % len(abilities)]; mark()
                elif e.key == pygame.K_r and sel_part is None:
                    c = doc["cards"][cur]
                    c["rarity"] = RARITIES[(RARITIES.index(c.get("rarity", "common")) + 1)
                                           % len(RARITIES)]; mark()
                elif e.key in (pygame.K_LEFT, pygame.K_RIGHT) and sel_part is None:
                    # rotate paper hue: rgb -> gbr / brg
                    c = doc["cards"][cur]; p = c["paper"]
                    c["paper"] = [p[1], p[2], p[0]] if e.key == pygame.K_RIGHT \
                                 else [p[2], p[0], p[1]]
                    mark()
                elif sel_part is not None:
                    part = doc["cards"][cur]["model"][sel_part]
                    if e.key == pygame.K_p:
                        i = PART_KINDS.index(part.get("part", "slab"))
                        part["part"] = PART_KINDS[(i + 1) % len(PART_KINDS)]; mark()
                    elif e.key in (pygame.K_PLUS, pygame.K_EQUALS, pygame.K_KP_PLUS):
                        part["size"] = round(part.get("size", 0.6) * 1.1, 3); mark()
                    elif e.key in (pygame.K_MINUS, pygame.K_KP_MINUS):
                        part["size"] = round(part.get("size", 0.6) / 1.1, 3); mark()
                    elif e.key == pygame.K_COMMA:
                        part["ang"] = round(part.get("ang", 0.0) - 0.15, 3); mark()
                    elif e.key == pygame.K_PERIOD:
                        part["ang"] = round(part.get("ang", 0.0) + 0.15, 3); mark()
                    elif e.key == pygame.K_LEFTBRACKET:
                        part["tone"] = round(max(0.4, part.get("tone", 1.0) - 0.05), 3); mark()
                    elif e.key == pygame.K_RIGHTBRACKET:
                        part["tone"] = round(min(1.6, part.get("tone", 1.0) + 0.05), 3); mark()
            elif e.type == pygame.MOUSEBUTTONDOWN and e.button == 1:
                commit_edit()
                for r, k in list_rows:
                    if r.collidepoint(e.pos):
                        cur = k; sel_part = None
                        break
                else:
                    for r, f in stat_chips:
                        if r.collidepoint(e.pos) and cur is not None:
                            v = doc["cards"][cur].get(f, "")
                            editing = (f, "" if f in ("name", "blurb") else str(v))
                            break
                    else:
                        for r, i in reversed(model_hits):
                            if r.collidepoint(e.pos) and cur is not None:
                                sel_part = i
                                part = doc["cards"][cur]["model"][i]
                                drag = (e.pos, list(part.get("at", [0, 0])))
                                break
                        else:
                            sel_part = None
            elif e.type == pygame.MOUSEBUTTONUP and e.button == 1:
                drag = None
            elif e.type == pygame.MOUSEMOTION and drag and cur is not None \
                    and sel_part is not None:
                (px, py), at0 = drag
                prev = pygame.Rect(W - 420, 70, 400, 400)
                scale = min(prev.w, prev.h) * 0.28
                part = doc["cards"][cur]["model"][sel_part]
                part["at"] = [round(at0[0] + (e.pos[0] - px) / scale, 3),
                              round(at0[1] + (e.pos[1] - py) / scale, 3)]
                mark()

        # ── draw ──
        screen.fill((26, 24, 30))
        star = " *" if dirty else ""
        pygame.display.set_caption(f"Paperbound Pack Editor — {doc.get('name')}{star}")
        # card list
        list_rows = []
        y = 70
        screen.blit(font.render(f"Pack: {doc.get('name')}", True, (235, 225, 200)), (16, 14))
        screen.blit(sfont.render(f"{len(doc['cards'])} cards · N new · Ctrl+S save "
                                 f"· V validate", True, (150, 145, 135)), (16, 40))
        for k in sorted(doc["cards"]):
            c = doc["cards"][k]
            r = pygame.Rect(14, y, 240, 30)
            hot = (k == cur)
            pygame.draw.rect(screen, (66, 58, 46) if hot else (40, 37, 42), r,
                             border_radius=6)
            if hot:
                pygame.draw.rect(screen, (240, 190, 80), r, 2, border_radius=6)
            pygame.draw.rect(screen, tuple(c.get("paper", [150, 150, 150])),
                             (r.left + 6, r.top + 7, 16, 16), border_radius=3)
            screen.blit(sfont.render(f"{c.get('name', k)}  ·  {c.get('cost')}c",
                                     True, (225, 218, 200)), (r.left + 30, r.top + 8))
            list_rows.append((r, k))
            y += 34
        # stat panel
        stat_chips = []
        if cur is not None:
            c = doc["cards"][cur]
            x0, y0 = 280, 70
            def chip(label, field, val, w=150):
                nonlocal y0
                r = pygame.Rect(x0, y0, w, 30)
                pygame.draw.rect(screen, (44, 41, 48), r, border_radius=6)
                if editing and editing[0] == field:
                    pygame.draw.rect(screen, (240, 190, 80), r, 2, border_radius=6)
                    val = editing[1] + "▏"
                screen.blit(sfont.render(f"{label}: {val}", True, (225, 218, 200)),
                            (r.left + 8, r.top + 8))
                stat_chips.append((r, field))
                y0 += 36
            chip("name", "name", c.get("name", ""), 260)
            for f, *_ in STAT_FIELDS:
                chip(f, f, c.get(f))
            chip("blurb", "blurb", (c.get("blurb", "") or "—")[:26], 260)
            info = [f"ability [A]: {c.get('ability')}",
                    f"rarity [R]: {c.get('rarity')}",
                    f"flying [F]: {c.get('flying')}",
                    f"building [B]: {c.get('building')}",
                    f"paper [←→]: {c.get('paper')}"]
            for line in info:
                screen.blit(sfont.render(line, True, (185, 178, 160)), (x0, y0))
                y0 += 24
            # model preview + parts
            prev = pygame.Rect(W - 420, 70, 400, 400)
            model_hits = draw_model_preview(pygame, screen, prev, c, sel_part)
            py_ = prev.bottom + 12
            screen.blit(sfont.render("model parts — click to select, drag to move, "
                                     "P kind, +/- size, ,. angle, [] tone, Ins/Del",
                                     True, (150, 145, 135)), (prev.left, py_))
            py_ += 22
            for i, part in enumerate(c.get("model", [])):
                markc = (240, 190, 80) if i == sel_part else (185, 178, 160)
                txt = (f"{i}: {part.get('part')} at={part.get('at')} "
                       f"size={part.get('size')} ang={part.get('ang', 0)} "
                       f"tone={part.get('tone', 1.0)}")
                screen.blit(sfont.render(txt, True, markc), (prev.left, py_))
                py_ += 20
        if status_t > 0:
            screen.blit(font.render(status, True, (240, 220, 150)), (280, 14))
        pygame.display.flip()
    pygame.quit()


# ── selftest ─────────────────────────────────────────────────────────────────

def _selftest():
    import tempfile
    abilities = load_abilities()
    assert len(abilities) > 10, "abilities list should come from cards.json"
    # the shipped pack must validate clean
    if os.path.exists(DEFAULT_PACK):
        doc = load_pack(DEFAULT_PACK)
        probs = validate_pack(doc, abilities)
        assert not probs, f"shipped pack invalid: {probs}"
    # a fresh card validates; a broken one is caught
    c = new_card("x")
    assert not validate_card("x", c, abilities)
    bad = copy.deepcopy(c); bad["ability"] = "nope"; bad["hp"] = -5; bad["model"] = []
    probs = validate_card("x", bad, abilities)
    assert len(probs) == 3, probs
    # save/load round-trip
    with tempfile.TemporaryDirectory() as td:
        p = os.path.join(td, "p.json")
        d = new_pack("t"); d["cards"]["x"] = c
        save_pack(d, p)
        back = load_pack(p)
        assert back["cards"]["x"] == c, "round-trip drifted"
    # headless preview render (dummy video)
    os.environ.setdefault("SDL_VIDEODRIVER", "dummy")
    import pygame
    pygame.init(); pygame.display.set_mode((100, 100))
    surf = pygame.Surface((420, 420))
    hits = draw_model_preview(pygame, surf, pygame.Rect(10, 10, 400, 400), c, 0)
    assert hits, "preview produced no hit rects"
    print(f"[pack_editor] selftest OK — {len(abilities)} abilities, "
          "validation, round-trip, preview")


if __name__ == "__main__":
    if "--selftest" in sys.argv:
        _selftest()
    else:
        args = [a for a in sys.argv[1:] if not a.startswith("-")]
        run_editor(args[0] if args else DEFAULT_PACK)
