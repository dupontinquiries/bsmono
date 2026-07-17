"""
Gyroball Maze Editor — author hand-made mazes for the tilting ball game.

The game (v15.py) normally carves a random maze; this tool lets you draw one by
hand and save it as a *maze pack* the game can load and cycle through.

Data model (matches v15 exactly):
  • a maze is a grid of COLS×ROWS cells with interior walls stored as edges
    (c, r, nc, nr, 'h'|'v') — 'h' walls sit between (c,r)-(c+1,r), 'v' walls
    between (c,r)-(c,r+1). The outer border is always present.
  • start and goal are cell coordinates (v15 historically fixed them at
    (0,0) and (cols-1, rows-1); packs may move them).

Pack file (mazes/<name>.json):
    {
      "name": "Spiral Gauntlet",
      "author": "you",
      "mazes": [
        {"name": "Level 1", "cols": 12, "rows": 9,
         "start": [0, 0], "goal": [11, 8],
         "walls": [[0,0,1,0,"h"], [3,2,3,3,"v"], ...]},
        ...
      ]
    }

Run:
    python maze_editor.py                    # open the GUI editor
    python maze_editor.py pack.json          # edit an existing pack
    python maze_editor.py --selftest         # headless round-trip + validation

Controls (GUI):
    Left-click an edge     — toggle a wall on that edge
    Left-drag              — paint walls;  hold Shift while dragging to erase
    S / G  + click a cell  — place the Start / Goal marker
    R                      — fill with a random carved maze (as the game does)
    C                      — clear all interior walls
    V                      — validate (start→goal reachability, prints verdict)
    N / P                  — next / previous maze in the pack;  A adds a new one
    Ctrl+S                 — save the pack
    ESC                    — quit (prompts in the title bar if unsaved)

Game integration:
    v15.py: `python v15.py [username] --pack mazes/mypack.json` loads the pack
    and cycles its mazes in order (falling back to random generation when the
    pack runs out). See load_pack() below — v15 imports it from this module.
"""

import os, sys, json, random

# ── data model ───────────────────────────────────────────────────────────────

def all_edges(cols, rows):
    """Every interior edge of a cols×rows grid, in the game's tuple format."""
    edges = set()
    for c in range(cols):
        for r in range(rows):
            if c < cols - 1: edges.add((c, r, c + 1, r, 'h'))
            if r < rows - 1: edges.add((c, r, c, r + 1, 'v'))
    return edges


def new_maze(cols=12, rows=9, name="Untitled"):
    """A maze with EVERY interior wall present (a sealed grid to carve from)."""
    return {
        "name": name, "cols": cols, "rows": rows,
        "start": [0, 0], "goal": [cols - 1, rows - 1],
        "walls": sorted(all_edges(cols, rows)),
    }


def carve_random(maze, seed=None):
    """Replace walls with a random DFS carve — the exact algorithm v15 uses,
    reimplemented on a local RNG so the editor never disturbs global random."""
    cols, rows = maze["cols"], maze["rows"]
    rng = random.Random(seed)
    visited = [[False] * rows for _ in range(cols)]
    walls = all_edges(cols, rows)

    def nb(c, r):
        n = []
        if c > 0:        n.append((c - 1, r))
        if c < cols - 1: n.append((c + 1, r))
        if r > 0:        n.append((c, r - 1))
        if r < rows - 1: n.append((c, r + 1))
        return n

    stack = [(0, 0)]; visited[0][0] = True; removed = set()
    while stack:
        c, r = stack[-1]
        uv = [(nc, nr) for nc, nr in nb(c, r) if not visited[nc][nr]]
        if uv:
            nc, nr = rng.choice(uv); visited[nc][nr] = True
            if   nc == c + 1: removed.add((c, r, c + 1, r, 'h'))
            elif nc == c - 1: removed.add((nc, r, c, r, 'h'))
            elif nr == r + 1: removed.add((c, r, c, r + 1, 'v'))
            elif nr == r - 1: removed.add((c, nr, c, r, 'v'))
            stack.append((nc, nr))
        else:
            stack.pop()
    maze["walls"] = sorted(walls - removed)
    return maze


def walls_as_set(maze):
    return {tuple(w[:4]) + (w[4],) for w in maze["walls"]}


def toggle_wall(maze, edge):
    ws = walls_as_set(maze)
    if edge in ws: ws.discard(edge)
    else: ws.add(edge)
    maze["walls"] = sorted(ws)


def reachable(maze):
    """BFS start→goal through open edges. Returns (ok, n_reachable_cells)."""
    cols, rows = maze["cols"], maze["rows"]
    ws = walls_as_set(maze)
    sx, sy = maze["start"]; gx, gy = maze["goal"]

    def open_between(c, r, nc, nr):
        if nc == c + 1: return (c, r, nc, r, 'h') not in ws
        if nc == c - 1: return (nc, r, c, r, 'h') not in ws
        if nr == r + 1: return (c, r, c, nr, 'v') not in ws
        if nr == r - 1: return (c, nr, c, r, 'v') not in ws
        return False

    seen = {(sx, sy)}; frontier = [(sx, sy)]
    while frontier:
        c, r = frontier.pop()
        for nc, nr in ((c+1, r), (c-1, r), (c, r+1), (c, r-1)):
            if 0 <= nc < cols and 0 <= nr < rows and (nc, nr) not in seen \
                    and open_between(c, r, nc, nr):
                seen.add((nc, nr)); frontier.append((nc, nr))
    return (gx, gy) in seen, len(seen)


def validate(maze):
    """Structural checks + reachability. Returns a list of problem strings."""
    probs = []
    cols, rows = maze.get("cols", 0), maze.get("rows", 0)
    if not (2 <= cols <= 64 and 2 <= rows <= 64):
        probs.append(f"grid {cols}x{rows} out of range (2..64)")
        return probs
    legal = all_edges(cols, rows)
    for w in maze.get("walls", []):
        t = tuple(w[:4]) + (w[4],)
        if t not in legal:
            probs.append(f"illegal wall edge {w}")
    for key in ("start", "goal"):
        x, y = maze.get(key, (0, 0))
        if not (0 <= x < cols and 0 <= y < rows):
            probs.append(f"{key} {x},{y} outside the grid")
    if tuple(maze.get("start", ())) == tuple(maze.get("goal", ())):
        probs.append("start and goal are the same cell")
    if not probs:
        ok, n = reachable(maze)
        if not ok:
            probs.append(f"goal unreachable from start ({n} cells reachable)")
    return probs


# ── pack I/O (this is what v15 imports) ──────────────────────────────────────

def load_pack(path):
    """Load a maze pack → normalized dict. Raises on structural failure but
    tolerates per-maze validation problems (they're reported, not fatal, so a
    pack with one bad level still ships its good ones — buildscript spirit)."""
    with open(path) as f:
        doc = json.load(f)
    mazes = []
    for i, m in enumerate(doc.get("mazes", [])):
        m = dict(m)
        m.setdefault("name", f"Level {i+1}")
        m["walls"] = [tuple(w[:4]) + (w[4],) for w in m.get("walls", [])]
        probs = validate(m)
        if probs:
            print(f"[maze pack] '{m['name']}' has problems (kept anyway): {probs}")
        mazes.append(m)
    if not mazes:
        raise ValueError(f"{path}: pack has no mazes")
    return {"name": doc.get("name", os.path.basename(path)),
            "author": doc.get("author", ""), "mazes": mazes}


def pack_walls(maze):
    """The wall set in the exact type v15's maze_to_rects expects."""
    return walls_as_set(maze)


def save_pack(doc, path):
    out = {
        "name": doc.get("name", "Untitled Pack"),
        "author": doc.get("author", ""),
        "mazes": [{
            "name": m["name"], "cols": m["cols"], "rows": m["rows"],
            "start": list(m["start"]), "goal": list(m["goal"]),
            "walls": [list(w) for w in sorted(walls_as_set(m))],
        } for m in doc["mazes"]],
    }
    tmp = path + ".tmp"
    with open(tmp, "w") as f:
        json.dump(out, f, indent=1)
    os.replace(tmp, path)


# ── GUI ──────────────────────────────────────────────────────────────────────

def run_editor(path=None):
    import pygame
    pygame.init()
    W, H = 1080, 780
    screen = pygame.display.set_mode((W, H), pygame.RESIZABLE)
    clock = pygame.time.Clock()
    font = pygame.font.Font(None, 22)
    sfont = pygame.font.Font(None, 17)

    if path and os.path.exists(path):
        doc = load_pack(path)
    else:
        doc = {"name": "New Pack", "author": "", "mazes": [carve_random(new_maze())]}
        path = path or os.path.join(os.path.dirname(os.path.abspath(__file__)),
                                    "mazes", "pack.json")
    cur = 0
    dirty = False
    mode = None                 # None | "start" | "goal"
    dragging = None             # None | "add" | "erase"
    status, status_t = "", 0.0

    def flash(s):
        nonlocal status, status_t
        status, status_t = s, 2.5

    def title():
        m = doc["mazes"][cur]
        star = " *" if dirty else ""
        pygame.display.set_caption(
            f"Gyroball Maze Editor — {doc['name']} · {m['name']} "
            f"({cur+1}/{len(doc['mazes'])}){star}")

    def layout():
        m = doc["mazes"][cur]
        cw = min((W - 80) // m["cols"], (H - 140) // m["rows"])
        ox = (W - cw * m["cols"]) // 2
        oy = 70
        return m, cw, ox, oy

    def edge_at(pos):
        """The interior edge nearest pos, if within snapping distance."""
        m, cw, ox, oy = layout()
        x, y = pos[0] - ox, pos[1] - oy
        if not (0 <= x <= cw * m["cols"] and 0 <= y <= cw * m["rows"]):
            return None
        c, r = int(x // cw), int(y // cw)
        fx, fy = x - c * cw, y - r * cw
        snap = max(6, cw // 5)
        cands = []
        if fx < snap and c > 0:              cands.append((fx,        (c - 1, r, c, r, 'h')))
        if cw - fx < snap and c < m["cols"] - 1: cands.append((cw - fx, (c, r, c + 1, r, 'h')))
        if fy < snap and r > 0:              cands.append((fy,        (c, r - 1, c, r, 'v')))
        if cw - fy < snap and r < m["rows"] - 1: cands.append((cw - fy, (c, r, c, r + 1, 'v')))
        if not cands:
            return None
        return min(cands)[1]

    def cell_at(pos):
        m, cw, ox, oy = layout()
        c = (pos[0] - ox) // cw; r = (pos[1] - oy) // cw
        if 0 <= c < m["cols"] and 0 <= r < m["rows"]:
            return int(c), int(r)
        return None

    def apply_drag(pos):
        m, *_ = layout()
        e = edge_at(pos)
        if e is None:
            return
        ws = walls_as_set(m)
        if dragging == "add" and e not in ws:
            toggle_wall(m, e); mark()
        elif dragging == "erase" and e in ws:
            toggle_wall(m, e); mark()

    def mark():
        nonlocal dirty
        dirty = True; title()

    title()
    running = True
    while running:
        dt = clock.tick(60) / 1000.0
        status_t = max(0.0, status_t - dt)
        for e in pygame.event.get():
            if e.type == pygame.QUIT:
                running = False
            elif e.type == pygame.VIDEORESIZE:
                W, H = e.w, e.h
            elif e.type == pygame.KEYDOWN:
                m = doc["mazes"][cur]
                mods = pygame.key.get_mods()
                if e.key == pygame.K_ESCAPE:
                    running = False
                elif e.key == pygame.K_s and (mods & pygame.KMOD_CTRL):
                    os.makedirs(os.path.dirname(path), exist_ok=True)
                    save_pack(doc, path); dirty = False; title()
                    flash(f"Saved → {path}")
                elif e.key == pygame.K_s:
                    mode = "start"; flash("Click a cell to place START")
                elif e.key == pygame.K_g:
                    mode = "goal"; flash("Click a cell to place GOAL")
                elif e.key == pygame.K_r:
                    carve_random(m); mark(); flash("Random carve")
                elif e.key == pygame.K_c:
                    m["walls"] = []; mark(); flash("Cleared interior walls")
                elif e.key == pygame.K_v:
                    probs = validate(m)
                    flash("Valid ✓ start→goal reachable" if not probs
                          else "Problems: " + "; ".join(probs))
                elif e.key == pygame.K_n:
                    cur = (cur + 1) % len(doc["mazes"]); title()
                elif e.key == pygame.K_p:
                    cur = (cur - 1) % len(doc["mazes"]); title()
                elif e.key == pygame.K_a:
                    doc["mazes"].append(carve_random(
                        new_maze(name=f"Level {len(doc['mazes'])+1}")))
                    cur = len(doc["mazes"]) - 1; mark()
                    flash("Added a new maze")
            elif e.type == pygame.MOUSEBUTTONDOWN and e.button == 1:
                m = doc["mazes"][cur]
                if mode in ("start", "goal"):
                    cell = cell_at(e.pos)
                    if cell:
                        m[mode] = list(cell); mode = None; mark()
                        flash(f"{'Start' if mode is None else mode} placed")
                else:
                    edge = edge_at(e.pos)
                    if edge is not None:
                        ws = walls_as_set(m)
                        dragging = "erase" if (edge in ws or
                                   pygame.key.get_mods() & pygame.KMOD_SHIFT) else "add"
                        toggle_wall(m, edge); mark()
            elif e.type == pygame.MOUSEBUTTONUP and e.button == 1:
                dragging = None
            elif e.type == pygame.MOUSEMOTION and dragging:
                apply_drag(e.pos)

        # ── draw ──
        screen.fill((24, 27, 36))
        m, cw, ox, oy = layout()
        gw, gh = cw * m["cols"], cw * m["rows"]
        pygame.draw.rect(screen, (34, 38, 50), (ox, oy, gw, gh))
        for c in range(m["cols"] + 1):
            pygame.draw.line(screen, (44, 49, 64), (ox + c * cw, oy), (ox + c * cw, oy + gh))
        for r in range(m["rows"] + 1):
            pygame.draw.line(screen, (44, 49, 64), (ox, oy + r * cw), (ox + gw, oy + r * cw))
        # border
        pygame.draw.rect(screen, (120, 190, 255), (ox, oy, gw, gh), 4)
        # interior walls
        for (c, r, nc, nr, kind) in walls_as_set(m):
            if kind == 'h':
                x = ox + nc * cw
                pygame.draw.line(screen, (170, 220, 255), (x, oy + r * cw), (x, oy + (r + 1) * cw), 5)
            else:
                y = oy + nr * cw
                pygame.draw.line(screen, (170, 220, 255), (ox + c * cw, y), (ox + (c + 1) * cw, y), 5)
        # start / goal
        sx, sy = m["start"]; gx, gy = m["goal"]
        pygame.draw.circle(screen, (90, 160, 255),
                           (ox + sx * cw + cw // 2, oy + sy * cw + cw // 2), max(6, cw // 4))
        pygame.draw.circle(screen, (60, 220, 120),
                           (ox + gx * cw + cw // 2, oy + gy * cw + cw // 2), max(6, cw // 4))
        pygame.draw.circle(screen, (210, 255, 230),
                           (ox + gx * cw + cw // 2, oy + gy * cw + cw // 2), max(3, cw // 8))
        # reachability hint (live, cheap at these sizes)
        ok, _ = reachable(m)
        verdict = "reachable ✓" if ok else "GOAL UNREACHABLE ✗"
        col = (110, 220, 140) if ok else (240, 110, 100)
        screen.blit(font.render(verdict, True, col), (ox, oy - 28))
        # help + status
        help1 = "click/drag edges: walls (Shift-drag erases) · S/G+click: start/goal · R random · C clear · V validate"
        help2 = "N/P prev-next · A add maze · Ctrl+S save · ESC quit"
        screen.blit(sfont.render(help1, True, (150, 158, 178)), (16, H - 44))
        screen.blit(sfont.render(help2, True, (150, 158, 178)), (16, H - 24))
        if status_t > 0:
            screen.blit(font.render(status, True, (240, 220, 150)), (16, 16))
        pygame.display.flip()
    pygame.quit()


# ── selftest ─────────────────────────────────────────────────────────────────

def _selftest():
    import tempfile
    # a carved maze is always fully connected → valid
    m = carve_random(new_maze(12, 9), seed=7)
    assert not validate(m), f"carved maze failed validation: {validate(m)}"
    ok, n = reachable(m)
    assert ok and n == 12 * 9, "carve should connect every cell"
    # sealed maze is invalid (goal unreachable)
    sealed = new_maze(6, 6)
    assert any("unreachable" in p for p in validate(sealed))
    # toggle round-trip
    e = (0, 0, 1, 0, 'h')
    ws0 = walls_as_set(m); toggle_wall(m, e); toggle_wall(m, e)
    assert walls_as_set(m) == ws0, "toggle is not an involution"
    # pack save/load round-trip preserves walls exactly
    doc = {"name": "t", "author": "", "mazes": [m, carve_random(new_maze(8, 5), seed=3)]}
    with tempfile.TemporaryDirectory() as td:
        p = os.path.join(td, "p.json")
        save_pack(doc, p)
        back = load_pack(p)
        assert len(back["mazes"]) == 2
        assert walls_as_set(back["mazes"][0]) == walls_as_set(m), "walls drifted in round-trip"
        assert pack_walls(back["mazes"][0]) == walls_as_set(m)
    # determinism: same seed → same carve
    a = carve_random(new_maze(12, 9), seed=42)["walls"]
    b = carve_random(new_maze(12, 9), seed=42)["walls"]
    assert a == b, "seeded carve is not deterministic"
    print("[maze_editor] selftest OK — model, validation, round-trip, determinism")


if __name__ == "__main__":
    if "--selftest" in sys.argv:
        _selftest()
    else:
        args = [a for a in sys.argv[1:] if not a.startswith("-")]
        run_editor(args[0] if args else None)
