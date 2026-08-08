#!/usr/bin/env python3
"""Echelon map editor — the paperbound editor shell (shared/editor-kit)
pointed at MapDef v2. Paint the four layers of a map on the open book:

  layers (Tab):  height  ramps  bridges  things
  brushes:       keys shown on the chips row per layer
                 height: 0-5 levels, # void        ramps: 0-4, . erase
                 bridges: 2-5 deck, . erase        things: a f 1 2 .
  [ / ]          brush size 1-4
  M              mirror the top-left half 180 degrees onto the rest
  S              save    (path from argv, default new.echelon-map.ron)
  T              cycle faction theme (genetron / xayid / keiron)
  --theme NAME   start on a faction skin      --shot OUT.png headless

Run:  python3 tools/mapedit.py assets/data/maps/confluence.ron
"""
import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "..", "..", "shared", "editor-kit"))
sys.path.insert(0, os.path.join(os.path.dirname(__file__), "editor-kit"))
import editor_kit as ek  # noqa: E402
import pygame  # noqa: E402

LEVEL_COLS = [(32, 40, 46), (40, 50, 58), (49, 61, 70), (58, 72, 82), (68, 84, 95), (79, 97, 109)]
VOID_COL = (14, 16, 20)
RAMP_COL = (96, 116, 82)
DECK_COL = (128, 118, 84)
THING_COLS = {"a": (56, 214, 186), "f": (176, 108, 255), "1": (77, 163, 255), "2": (255, 107, 82)}

BRUSHES = {
    "height": list("012345#"),
    "ramps": list("01234."),
    "bridges": list("2345."),
    "things": list("af12."),
}


def colorer(layers, layer):
    hgt = layers["height"]

    def f(x, y, ch):
        base = VOID_COL if hgt[y][x] == "#" else LEVEL_COLS[int(hgt[y][x])]
        if layer != "height":
            top = layers[layer][y][x]
            if layer == "ramps" and top != ".":
                return RAMP_COL
            if layer == "bridges" and top != ".":
                return DECK_COL
            if layer == "things" and top in THING_COLS:
                return THING_COLS[top]
            # dim the underlay so the active layer pops
            return tuple(int(v * 0.55) for v in base)
        if layers["bridges"][y][x] != ".":
            return DECK_COL
        if layers["ramps"][y][x] != ".":
            return RAMP_COL
        return base

    return f


def mirror(layers):
    h = len(layers["height"])
    w = len(layers["height"][0])
    for key in ek.ron_layers.LAYERS:
        g = layers[key]
        for y in range(h):
            for x in range(w):
                if y * w + x < (h * w) // 2:
                    t = g[y][x]
                    if key == "things":
                        t = {"1": "2", "2": "1"}.get(t, t)
                    g[h - 1 - y][w - 1 - x] = t


def main():
    argv = sys.argv[1:]
    shot = theme_name = None
    if "--shot" in argv:
        i = argv.index("--shot")
        shot = argv[i + 1]
        argv = argv[:i] + argv[i + 2:]
        os.environ.setdefault("SDL_VIDEODRIVER", "dummy")
    if "--theme" in argv:
        i = argv.index("--theme")
        theme_name = argv[i + 1]
        argv = argv[:i] + argv[i + 2:]
    args = [a for a in argv if not a.startswith("--")]
    path = args[0] if args else "new.echelon-map.ron"
    if os.path.exists(path):
        meta, layers = ek.ron_layers.load(path)
    else:
        meta = {"name": "Untitled", "tileset": "relic_wastes"}
        layers = {k: [["." if k != "height" else "1"] * 64 for _ in range(64)] for k in ek.ron_layers.LAYERS}

    ek.init()
    W, H = 1280, 860
    screen = pygame.display.set_mode((W, H))
    pygame.display.set_caption("Echelon Map Editor — " + meta.get("name", ""))
    fonts = ek.fonts()
    theme_keys = list(ek.THEMES)
    theme_ix = theme_keys.index(theme_name) if theme_name in ek.THEMES else theme_keys.index("keiron")
    console = ek.Console(W, H, "ECHELON MAP EDITOR", ek.THEMES[theme_keys[theme_ix]])
    layer_ix, brush_ch, brush_sz, status = 0, "3", 1, "ready"
    grid = ek.GridPainter(console.page.inflate(-16, -60).move(0, 26), len(layers["height"][0]), len(layers["height"]))

    clock = pygame.time.Clock()
    running = True
    while running:
        mouse = pygame.mouse.get_pos()
        layer = ek.ron_layers.LAYERS[layer_ix]
        for ev in pygame.event.get():
            if ev.type == pygame.QUIT:
                running = False
            elif ev.type == pygame.KEYDOWN:
                if ev.key == pygame.K_ESCAPE:
                    running = False
                elif ev.key == pygame.K_TAB:
                    layer_ix = (layer_ix + 1) % 4
                    brush_ch = BRUSHES[ek.ron_layers.LAYERS[layer_ix]][0]
                elif ev.key == pygame.K_m:
                    mirror(layers)
                    status = "mirrored"
                elif ev.key == pygame.K_s:
                    ek.ron_layers.save(path, meta, layers)
                    status = "saved " + path
                elif ev.key == pygame.K_t:
                    theme_ix = (theme_ix + 1) % len(theme_keys)
                    console.t = ek.THEMES[theme_keys[theme_ix]]
                elif ev.key == pygame.K_LEFTBRACKET:
                    brush_sz = max(1, brush_sz - 1)
                elif ev.key == pygame.K_RIGHTBRACKET:
                    brush_sz = min(4, brush_sz + 1)
                elif ev.unicode in BRUSHES[layer]:
                    brush_ch = ev.unicode

        thm = console.t
        page = console.frame(screen, "%s — %s — [T] theme: %s" % (meta.get("name", ""), meta.get("tileset", ""), thm.name))
        # chips: layers + brushes
        x = page.x
        for i, name in enumerate(ek.ron_layers.LAYERS):
            r = ek.chip(screen, thm, x, page.y - 4, name, fonts["small"], active=i == layer_ix)
            if r.collidepoint(mouse) and pygame.mouse.get_pressed()[0]:
                layer_ix, brush_ch = i, BRUSHES[name][0]
            x = r.right + 6
        x += 16
        for ch in BRUSHES[layer]:
            r = ek.chip(screen, thm, x, page.y - 4, ch, fonts["mono"], active=ch == brush_ch)
            if r.collidepoint(mouse) and pygame.mouse.get_pressed()[0]:
                brush_ch = ch
            x = r.right + 4
        ek.chip(screen, thm, x + 12, page.y - 4, "brush %d  [ ]" % brush_sz, fonts["small"])

        hover = grid.cell_at(*mouse)
        if hover and pygame.mouse.get_pressed()[0]:
            grid.paint(layers[layer], hover[0], hover[1], brush_ch, brush_sz)
        grid.draw(screen, layers[layer], colorer(layers, layer), hover, brush_sz, cursor=thm.accent)

        st = fonts["small"].render(
            "Tab layer   M mirror   S save   T theme   Esc quit    |    " + status, True, thm.text_dim
        )
        screen.blit(st, (page.x, page.bottom + 10))
        pygame.display.flip()
        if shot:
            ek.screenshot(screen, shot)
            print("shot ->", shot)
            return
        clock.tick(60)


if __name__ == "__main__":
    main()
