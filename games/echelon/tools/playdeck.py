#!/usr/bin/env python3
"""Echelon Command Deck — the playable client, on editor-kit's faction
consoles, speaking the runner's serve protocol. You are player 0; the
coach (or, soon, the arena champion) is player 1.

  python3 tools/playdeck.py Keiron Xayid --map confluence --seed 5
      [--theme keiron|xayid|genetron]  [--runner PATH]
      [--script FILE --shot-prefix P]     headless scripted CI mode

Controls
  drag LMB       select own units (click: nearest own entity)
  RMB            smart order: harvest on a node w/ workers, else move;
                 placement click in build mode; target click in cast mode
  A              attack-move selection to cursor
  S / H          stop / hold position
  R              set rally of selected structure to cursor
  1..9           production: train Nth option of selected structure
  B then 1..9    build: pick worker's Nth structure, then RMB places
  Q/W/E          cast ability slot 1/2/3 (then RMB targets)
  T              cycle faction theme      Esc  clear modes / quit
"""
import json
import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "editor-kit"))
sys.path.insert(0, os.path.join(HERE, "..", "..", "..", "shared", "editor-kit"))
import editor_kit as ek  # noqa: E402
import pygame  # noqa: E402

LEVEL_COLS = [(30, 37, 43), (37, 46, 54), (45, 56, 65), (53, 66, 77), (62, 77, 89), (72, 89, 102)]
VOID_COL = (10, 12, 16)
RAMP_COL = (86, 104, 74)
DECK_COL = (118, 108, 76)
P_COLS = [(96, 214, 230), (255, 107, 82)]
NODE_COLS = {0: (56, 214, 186), 1: (176, 108, 255)}


class Server:
    def __init__(self, runner, r1, r2, map_name, seed, opponent="coach"):
        self.p = subprocess.Popen(
            [runner, "serve", r1, r2, "--map", map_name, "--seed", str(seed),
             "--opponent", opponent],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, bufsize=1)
        self.hello = json.loads(self.p.stdout.readline())
        self.pending = []

    def send(self, line):
        self.pending.append(line)

    def step(self, n=1):
        for l in self.pending:
            self.p.stdin.write(l + "\n")
        self.pending = []
        self.p.stdin.write("step %d\n" % n)
        self.p.stdin.flush()
        return json.loads(self.p.stdout.readline())

    def quit(self):
        try:
            self.p.stdin.write("quit\n")
            self.p.stdin.flush()
            self.p.wait(timeout=3)
        except Exception:
            self.p.kill()


class Deck:
    def __init__(self, args):
        flags = {a.split("=")[0]: True for a in args if a.startswith("--")}
        val = lambda k, d: args[args.index(k) + 1] if k in args else d
        self.r1 = args[0] if args and not args[0].startswith("--") else "Keiron"
        self.r2 = args[1] if len(args) > 1 and not args[1].startswith("--") else "Xayid"
        runner = val("--runner", os.path.join(HERE, "..", "target", "release", "echelon-runner"))
        self.script = val("--script", None)
        self.shot_prefix = val("--shot-prefix", "/tmp/deck")
        if self.script:
            os.environ.setdefault("SDL_VIDEODRIVER", "dummy")
        self.opponent = val("--opponent", "coach")
        self.srv = Server(runner, self.r1, self.r2, val("--map", "confluence"),
                          val("--seed", "5"), self.opponent)
        h = self.srv.hello
        self.W, self.H = h["w"], h["h"]
        self.defs = h["defs"]
        self.nodes = h["nodes"]
        ek.init()
        self.SW, self.SH = 1280, 860
        self.screen = pygame.display.set_mode((self.SW, self.SH))
        pygame.display.set_caption("Echelon Command Deck")
        self.fonts = ek.fonts()
        tname = val("--theme", self.r1.lower())
        self.theme_keys = list(ek.THEMES)
        self.tix = self.theme_keys.index(tname) if tname in ek.THEMES else 2
        self.console = ek.Console(self.SW, self.SH, "ECHELON COMMAND DECK",
                                  ek.THEMES[self.theme_keys[self.tix]])
        page = self.console.page
        self.tile = max(4, min((page.w - 210) // self.W, (page.h - 20) // self.H))
        self.ox = page.x + 4
        self.oy = page.y + 28
        self.terrain = self._render_terrain(h)
        self.sel = set()
        self.snap = None
        self.ghosts = {}
        self.mode = None      # None | ("build", defid) | ("cast", slot)
        self.drag = None
        self.seen = set()
        self.flags = flags

    def _render_terrain(self, h):
        surf = pygame.Surface((self.W * self.tile, self.H * self.tile))
        hgt, ramp, bridge = h["hgt"], h["ramp"], h["bridge"]
        for y in range(self.H):
            for x in range(self.W):
                i = y * self.W + x
                if bridge[i] > 0:
                    c = DECK_COL
                elif ramp[i] >= 0:
                    c = RAMP_COL
                elif hgt[i] == 255:
                    c = VOID_COL
                else:
                    c = LEVEL_COLS[min(hgt[i], 5)]
                surf.fill(c, (x * self.tile, y * self.tile, self.tile, self.tile))
        for x, y, kind, _amt in self.nodes:
            pygame.draw.circle(surf, NODE_COLS[kind],
                               (x * self.tile + self.tile // 2, y * self.tile + self.tile // 2),
                               max(2, self.tile // 2 - 1))
        return surf

    # ---- coords ----
    def to_px(self, mx, my):
        return (self.ox + mx * self.tile // 1000, self.oy + my * self.tile // 1000)

    def to_milli(self, px, py):
        return ((px - self.ox) * 1000 // self.tile, (py - self.oy) * 1000 // self.tile)

    # ---- selection & orders ----
    def my_ents(self):
        return [e for e in (self.snap or {}).get("ents", []) if e[2] == 0]

    def ids(self):
        return [i for i in self.sel]

    def order(self, verb, mx, my):
        ids = self.ids()
        if not ids:
            return
        self.srv.send("%s %d %d %s" % (verb, mx, my, " ".join(map(str, ids))))

    def smart_rmb(self, px, py):
        mx, my = self.to_milli(px, py)
        if self.mode and self.mode[0] == "build":
            workers = [e for e in self.my_ents() if e[0] in self.sel and self.defs[e[1]]["kind"] == "W"]
            if workers:
                self.srv.send("build %d %d %d %d" % (workers[0][0], self.mode[1], mx // 1000, my // 1000))
            self.mode = None
            return
        if self.mode and self.mode[0] == "cast":
            casters = [e for e in self.my_ents() if e[0] in self.sel]
            if casters:
                tgt = self._nearest_enemy(mx, my)
                self.srv.send("cast %d %d %d %d %d" % (casters[0][0], self.mode[1], tgt, mx, my))
            self.mode = None
            return
        node = self._node_at(mx // 1000, my // 1000)
        workers = [e[0] for e in self.my_ents() if e[0] in self.sel and self.defs[e[1]]["kind"] == "W"]
        if node is not None and workers:
            self.srv.send("harvest %d %s" % (node, " ".join(map(str, workers))))
        else:
            self.order("move", mx, my)

    def _node_at(self, tx, ty):
        for i, (x, y, _k, _a) in enumerate(self.nodes):
            if abs(x - tx) <= 1 and abs(y - ty) <= 1:
                return i
        return None

    def _nearest_enemy(self, mx, my):
        best, bid = 4_000_000, 0
        for e in (self.snap or {}).get("ents", []):
            if e[2] != 1:
                continue
            d = abs(e[3] - mx) + abs(e[4] - my)
            if d < best:
                best, bid = d, e[0]
        return bid if best < 3000 else 0

    # ---- rendering ----
    def draw(self):
        thm = self.console.t
        self.console.frame(self.screen, "%s vs %s — %s opponent" % (self.r1, self.r2,
            "LEAGUE CHAMPION" if self.opponent.startswith("champion") else "coach"))
        self.screen.blit(self.terrain, (self.ox, self.oy))
        # fog: darken tiles no friendly can see
        self.seen = set()
        for e in self.my_ents():
            s = self.defs[e[1]]["sight"] // 1000
            tx, ty = e[3] // 1000, e[4] // 1000
            for dy in range(-s, s + 1):
                for dx in range(-s, s + 1):
                    if dx * dx + dy * dy <= s * s:
                        self.seen.add((tx + dx, ty + dy))
        fog = pygame.Surface((self.tile, self.tile))
        fog.set_alpha(150)
        fog.fill((6, 8, 12))
        for y in range(self.H):
            for x in range(self.W):
                if (x, y) not in self.seen:
                    self.screen.blit(fog, (self.ox + x * self.tile, self.oy + y * self.tile))
        # ghosts of enemy structures last seen
        for gid, (d, gx, gy) in self.ghosts.items():
            if any(e[0] == gid for e in (self.snap or {}).get("ents", [])):
                continue
            px, py = self.to_px(gx, gy)
            r = pygame.Rect(0, 0, self.tile * self.defs[d]["fp"], self.tile * self.defs[d]["fp"])
            r.center = (px, py)
            pygame.draw.rect(self.screen, ek.shade(P_COLS[1], 0.35), r, 1)
        # entities
        for e in (self.snap or {}).get("ents", []):
            i, d, p, mx, my, hp, sh, en, pr, o, lv, ck = e
            dd = self.defs[d]
            px, py = self.to_px(mx, my)
            col = P_COLS[p]
            if ck:
                col = ek.shade(col, 0.5)
            if dd["kind"] == "S":
                r = pygame.Rect(0, 0, self.tile * dd["fp"], self.tile * dd["fp"])
                r.center = (px, py)
                pygame.draw.rect(self.screen, ek.shade(col, 0.55 if pr else 0.9), r)
                pygame.draw.rect(self.screen, col, r, 1)
                if p == 1:
                    self.ghosts[i] = (d, mx, my)
            else:
                rad = 3 + (2 if dd["sup"] >= 8 else 0) + (lv - 6 if lv else 0)
                pygame.draw.circle(self.screen, col, (px, py), max(2, rad))
                if dd["air"]:
                    pygame.draw.circle(self.screen, ek.shade(col, 1.5), (px, py), max(2, rad), 1)
            if i in self.sel:
                pygame.draw.circle(self.screen, thm.accent, (px, py), 7 + self.tile * (dd["fp"] - 1) // 2, 1)
            mhp = max(1, dd["hp"])
            if hp < mhp:
                w = 10
                pygame.draw.rect(self.screen, (40, 20, 20), (px - w // 2, py - 9, w, 2))
                pygame.draw.rect(self.screen, (110, 220, 110), (px - w // 2, py - 9, w * hp // mhp, 2))
        if self.drag:
            x0, y0 = self.drag
            x1, y1 = pygame.mouse.get_pos()
            r = pygame.Rect(min(x0, x1), min(y0, y1), abs(x1 - x0), abs(y1 - y0))
            pygame.draw.rect(self.screen, thm.accent, r, 1)
        self._panel(thm)
        if self.snap and self.snap["winner"] >= 0:
            msg = "VICTORY" if self.snap["winner"] == 0 else "DEFEAT"
            t = self.fonts["title"].render(msg, True, thm.accent)
            self.screen.blit(t, (self.SW // 2 - t.get_width() // 2, self.SH // 2 - 40))
        pygame.display.flip()

    def _panel(self, thm):
        page = self.console.page
        x0 = self.ox + self.W * self.tile + 10
        s = self.snap or {"res": [0, 0], "sup": [0, 0], "tick": 0}
        hud = "alloy %d   flux %d   supply %d/%d   t %d" % (
            s["res"][0], s["res"][1], s["sup"][0] // 2, s["sup"][1] // 2, s["tick"])
        self.screen.blit(self.fonts["body"].render(hud, True, thm.text), (self.ox, page.y - 2))
        y = self.oy
        sel_list = [e for e in self.my_ents() if e[0] in self.sel]
        label = lambda t, yy, active=False: ek.chip(self.screen, thm, x0, yy, t, self.fonts["small"], active)
        if len(sel_list) == 1:
            e = sel_list[0]
            dd = self.defs[e[1]]
            label(dd["name"], y); y += 26
            if dd["kind"] == "S" and dd["trains"]:
                label("train:", y); y += 22
                for k, tdef in enumerate(dd["trains"][:9]):
                    td = self.defs[tdef]
                    label("%d %s %da/%df" % (k + 1, td["name"], td["cost"][0], td["cost"][1]), y)
                    y += 20
                q = (self.snap or {}).get("q", {}).get(str(e[0]), [])
                if q:
                    label("queue: " + ",".join(self.defs[d]["name"][:6] for d in q), y); y += 20
            if dd["kind"] == "W" and dd["builds"]:
                label("B then 1-9 build:", y); y += 22
                for k, bdef in enumerate(dd["builds"][:9]):
                    label("%d %s" % (k + 1, self.defs[bdef]["name"]), y); y += 20
            if dd["abil"]:
                label("Q/W/E cast: " + "/".join(dd["abil"]), y); y += 20
        if self.mode:
            label("mode: %s (RMB to place/target, Esc cancels)" % self.mode[0], y, True)

    # ---- input ----
    def key(self, k, uni):
        mp = pygame.mouse.get_pos()
        mx, my = self.to_milli(*mp)
        if k == pygame.K_a:
            self.order("amove", mx, my)
        elif k == pygame.K_s:
            self.srv.send("stop " + " ".join(map(str, self.ids())))
        elif k == pygame.K_h:
            self.srv.send("hold " + " ".join(map(str, self.ids())))
        elif k == pygame.K_r:
            for e in self.my_ents():
                if e[0] in self.sel and self.defs[e[1]]["kind"] == "S":
                    self.srv.send("rally %d %d %d" % (e[0], mx, my))
        elif k == pygame.K_t:
            self.tix = (self.tix + 1) % len(self.theme_keys)
            self.console.t = ek.THEMES[self.theme_keys[self.tix]]
        elif k == pygame.K_b:
            self.mode = ("buildpick",)
        elif uni in "123456789":
            n = int(uni) - 1
            sel = [e for e in self.my_ents() if e[0] in self.sel]
            if len(sel) == 1:
                dd = self.defs[sel[0][1]]
                if self.mode and self.mode[0] == "buildpick" and n < len(dd["builds"]):
                    self.mode = ("build", dd["builds"][n])
                elif dd["kind"] == "S" and n < len(dd["trains"]):
                    self.srv.send("train %d %d" % (sel[0][0], dd["trains"][n]))
        elif k in (pygame.K_q, pygame.K_w, pygame.K_e):
            self.mode = ("cast", {pygame.K_q: 0, pygame.K_w: 1, pygame.K_e: 2}[k])
        elif k == pygame.K_ESCAPE:
            if self.mode:
                self.mode = None
            else:
                return False
        return True

    def select_box(self, r):
        self.sel = set()
        for e in self.my_ents():
            px, py = self.to_px(e[3], e[4])
            if r.collidepoint(px, py) and self.defs[e[1]]["kind"] != "S":
                self.sel.add(e[0])
        if not self.sel:  # click: nearest own anything
            best, bid = 18, None
            for e in self.my_ents():
                px, py = self.to_px(e[3], e[4])
                d = abs(px - r.centerx) + abs(py - r.centery)
                if d < best:
                    best, bid = d, e[0]
            if bid is not None:
                self.sel = {bid}

    # ---- loops ----
    def run(self):
        clock = pygame.time.Clock()
        running = True
        while running:
            for ev in pygame.event.get():
                if ev.type == pygame.QUIT:
                    running = False
                elif ev.type == pygame.KEYDOWN:
                    running = self.key(ev.key, ev.unicode) and running
                elif ev.type == pygame.MOUSEBUTTONDOWN and ev.button == 1:
                    self.drag = ev.pos
                elif ev.type == pygame.MOUSEBUTTONUP and ev.button == 1 and self.drag:
                    x0, y0 = self.drag
                    r = pygame.Rect(min(x0, ev.pos[0]), min(y0, ev.pos[1]),
                                    max(2, abs(ev.pos[0] - x0)), max(2, abs(ev.pos[1] - y0)))
                    self.select_box(r)
                    self.drag = None
                elif ev.type == pygame.MOUSEBUTTONDOWN and ev.button == 3:
                    self.smart_rmb(*ev.pos)
            self.snap = self.srv.step(1)
            self.draw()
            clock.tick(22)
        self.srv.quit()

    def run_script(self):
        ok = True
        start_pos = {}
        for raw in open(self.script):
            t = raw.split()
            if not t or t[0].startswith("#"):
                continue
            if t[0] == "wait":
                for _ in range(int(t[1])):
                    self.snap = self.srv.step(1)
                self.draw()
            elif t[0] == "select":
                self.sel = {e[0] for e in self.my_ents()
                            if t[1] == "all" and self.defs[e[1]]["kind"] != "S"
                            or t[1] == "workers" and self.defs[e[1]]["kind"] == "W"}
                start_pos = {e[0]: (e[3], e[4]) for e in self.my_ents() if e[0] in self.sel}
            elif t[0] == "amove":
                self.order("amove", int(t[1]) * 1000, int(t[2]) * 1000)
            elif t[0] == "move":
                self.order("move", int(t[1]) * 1000, int(t[2]) * 1000)
            elif t[0] == "train_first":
                halls = [e for e in self.my_ents()
                         if self.defs[e[1]]["kind"] == "S" and self.defs[e[1]]["trains"]]
                self.srv.send("train %d %d" % (halls[0][0], self.defs[halls[0][1]]["trains"][0]))
            elif t[0] == "shot":
                self.draw()
                ek.screenshot(self.screen, "%s_%s.png" % (self.shot_prefix, t[1]))
                print("shot ->", "%s_%s.png" % (self.shot_prefix, t[1]))
            elif t[0] == "assert_moved":
                moved = any((e[3], e[4]) != start_pos.get(e[0], (e[3], e[4]))
                            for e in self.my_ents() if e[0] in start_pos)
                print("assert_moved:", moved)
                ok = ok and moved
            elif t[0] == "assert_queue":
                ok2 = bool((self.snap or {}).get("q"))
                print("assert_queue:", ok2)
                ok = ok and ok2
        self.srv.quit()
        print("DECK", "PASS" if ok else "FAIL")
        return 0 if ok else 1


if __name__ == "__main__":
    d = Deck(sys.argv[1:])
    if d.script:
        sys.exit(d.run_script())
    d.run()
