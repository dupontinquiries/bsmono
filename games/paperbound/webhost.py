"""
Paperbound Web Host — strap the game to the LAN.

Runs a small stdlib-only HTTP server (no pip deps) that:
  • runs the authoritative Paperbound simulation server-side (headless),
  • streams the live board state to any device on the LAN as JSON,
  • serves a three.js client that renders that state in 3D in the browser,
  • serves a tournament CONTROL page (backed by tournament.py) so an organiser
    can build a bracket, seat players, report results and advance rounds.

Design notes / honest scope
  - This is a SPECTATE + ORGANISE layer, plus lightweight browser deploys. It is
    NOT a full re-port of the pygame renderer into JS — the desktop/mobile client
    stays the definitive way to *play*. The browser shows a faithful 3D view of
    the same simulation and lets an organiser run tournaments from a phone.
  - Stdlib only (http.server + threading + json). Works on a pure LAN. three.js
    loads from a CDN in the browser, with a Canvas-2D fallback baked in so the
    board still renders if the CDN is unreachable.
  - State is delivered by short-poll (GET /state every ~100ms). On a LAN this is
    smooth and far more robust than hand-rolling WebSocket framing in stdlib.

Run:
    python webhost.py                # starts on 0.0.0.0:8080, prints LAN URL
    python webhost.py --port 9000
    python webhost.py --demo         # run a self-playing demo match to watch
"""
import json, socket, threading, time, os, sys, argparse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

# ── optional imports: the sim + tournament. Kept lazy so the control page and
#    static serving still work even in a headless env where pygame is odd. ──
try:
    import tournament as _tourn
    TOURN_OK = True
except Exception:
    TOURN_OK = False


# ════════════════════════════════════════════════════════════════════════════
#  GAME SERVER — wraps a headless Game and exposes JSON snapshots
# ════════════════════════════════════════════════════════════════════════════
class GameServer:
    """Owns one running match. Steps the sim on a background thread and hands out
    JSON snapshots. Thread-safe via a single lock around the sim + snapshot."""
    def __init__(self, arena="study", deck=None, seed=0, ai=True):
        self.lock = threading.Lock()
        self.arena = arena
        self.seed = seed
        self.ai = ai
        self._deck = deck
        self.running = False
        self.thread = None
        self.g = None
        self.geo = {}          # board geometry sent once to clients
        self.title = "Paperbound"
        self._pending = []     # browser-requested deploys: (key,x,y,team)
        # tournament linkage: when this match is cast from the bracket, remember
        # which match it is and the two seat names, so we can auto-report the
        # winner back to the bracket when the game ends.
        self.linked_match = None      # bracket match id, or None
        self.seat_names = ("Bottom", "Top")
        self._reported = False
        self._on_result = None        # callback(match_id, winner_name)
        # head-to-head seats: two browsers can each claim a team and play against
        # each other (no AI). seat token -> team (0=bottom, 1=top).
        self.seats = {}               # token -> team
        self.seat_deck = {0: None, 1: None}
        self.spectators = {}          # token -> last-seen time (for a live count)
        self._make_game()

    def note_spectator(self, token):
        """Record a browser as active (called on each /state poll) and return the
        current count of browsers seen in the last few seconds."""
        now = time.time()
        if token:
            self.spectators[token] = now
        # prune anyone not seen for 5s (they closed the tab / walked away)
        self.spectators = {t: ts for t, ts in self.spectators.items() if now - ts < 5.0}
        return len(self.spectators)

    # -- head-to-head seats ---------------------------------------------------
    def claim_seat(self, token):
        """Assign a browser a free team. Returns team index or None if full.
        Claiming the second seat turns the built-in AI off (real head-to-head)."""
        with self.lock:
            if token in self.seats:
                return self.seats[token]
            taken = set(self.seats.values())
            for team in (0, 1):
                if team not in taken:
                    self.seats[token] = team
                    if len(self.seats) >= 2:
                        self.ai = False        # two humans → drop the AI
                    return team
            return None

    def release_seat(self, token):
        with self.lock:
            self.seats.pop(token, None)

    # -- sim lifecycle --------------------------------------------------------
    def _make_game(self):
        import pygame, game
        if not pygame.get_init():
            os.environ.setdefault("SDL_VIDEODRIVER", "dummy")
            os.environ.setdefault("SDL_AUDIODRIVER", "dummy")
            pygame.init()
        self._game_mod = game
        game.relayout(1280, 800)
        game.set_arena(self.arena)
        deck = self._deck or game.DEFAULT_DECK[:]
        self.g = game.Game(deck=deck, seed=self.seed)
        b = game.BOOK
        self.geo = {"left": b.left, "top": b.top, "right": b.right,
                    "bottom": b.bottom, "w": b.width, "h": b.height,
                    "river": game.RIVER_Y, "arena": self.arena}
        # date-driven event (same for every device today) → theme the web view too
        try:
            import events as _ev
            e = _ev.active_event()
            self.event = {"key": e["key"], "name": e["name"], "emoji": e.get("emoji", ""),
                          "accent": list(e.get("accent", (200, 168, 96))),
                          "palette": list(e.get("palette", (230, 220, 200))),
                          "particles": e.get("particles"), "banner": e.get("banner", "")} if e else None
        except Exception:
            self.event = None

    def start(self):
        if self.running:
            return
        self.running = True
        self.thread = threading.Thread(target=self._loop, daemon=True)
        self.thread.start()

    def stop(self):
        self.running = False

    def reset(self, arena=None, seed=None):
        with self.lock:
            if arena is not None:
                self.arena = arena
            if seed is not None:
                self.seed = seed
            self._make_game()

    def cast_match(self, match_id, seat_a, seat_b, arena=None, on_result=None, ai=True):
        """Start a fresh live match tied to a bracket match. When it ends, the
        winner is reported back to the tournament via on_result(match_id, name)."""
        with self.lock:
            if arena is not None:
                self.arena = arena
            self.linked_match = match_id
            self.seat_names = (seat_a or "Bottom", seat_b or "Top")
            self.title = f"{self.seat_names[0]}  vs  {self.seat_names[1]}"
            self._reported = False
            self._on_result = on_result
            self.ai = ai
            self._make_game()
        self.start()

    def _check_result(self):
        """If a linked match just ended, report its winner to the bracket once."""
        if self.linked_match is None or self._reported or self.g is None:
            return
        res = self.g.result
        if res in ("win", "lose"):
            self._reported = True
            winner = self.seat_names[0] if res == "win" else self.seat_names[1]
            cb = self._on_result
            mid = self.linked_match
            if cb:
                try:
                    cb(mid, winner)
                except Exception:
                    pass

    def queue_deploy(self, key, x, y, team):
        with self.lock:
            self._pending.append((key, float(x), float(y), int(team)))

    def _loop(self):
        game = self._game_mod
        DT = 1.0 / 20.0            # sim runs at the fixed 20 TPS
        nxt = time.time()
        ai_cd = 0.0
        while self.running:
            now = time.time()
            if now >= nxt:
                nxt += DT
                with self.lock:
                    if self.g and not self.g.result:
                        # apply any browser-requested deploys, each paying from
                        # the right team's ink pool (bottom=ink, top=ai_ink)
                        for (key, x, y, team) in self._pending:
                            try:
                                cost = self.g.cost_of(key, team)
                                pool = self.g.ink if team == 0 else self.g.ai_ink
                                if pool >= cost:
                                    self.g.deploy(key, x, y, team)
                                    if team == 0: self.g.ink -= cost
                                    else: self.g.ai_ink -= cost
                            except Exception:
                                pass
                        self._pending.clear()
                        # a light built-in AI plays team 1 ONLY while no human has
                        # claimed the top seat (so head-to-head has no AI meddling)
                        top_claimed = 1 in self.seats.values()
                        if self.ai and not top_claimed:
                            ai_cd -= DT
                            if ai_cd <= 0 and self.g.ai_ink >= 3:
                                ai_cd = 2.0
                                self._ai_move()
                        self.g.update()
                        self._check_result()
            else:
                time.sleep(min(0.005, max(0, nxt - now)))

    def _ai_move(self):
        import random as _r
        g = self.g
        deck = getattr(g, "ai_deck", None) or g.deck
        affordable = [k for k in deck if self._game_mod.CARDS[k]["cost"] <= g.ai_ink]
        if not affordable:
            return
        key = _r.choice(affordable)
        b = self._game_mod.BOOK
        x = _r.uniform(b.left + 60, b.right - 60)
        y = self._game_mod.RIVER_Y - _r.uniform(20, 120)
        try:
            g.deploy(key, x, y, 1)
        except Exception:
            pass

    # -- snapshot -------------------------------------------------------------
    def snapshot(self):
        with self.lock:
            g = self.g
            if g is None:
                return {"ok": False}
            CARDS = self._game_mod.CARDS
            fam_of = getattr(self._game_mod, "_card_family", lambda k: "figure")
            def col(k):
                c = CARDS.get(k, {}).get("paper", (170, 150, 110))
                return list(c) if not isinstance(c, list) else c
            units = [{
                "id": getattr(u, "uid", 0),
                "x": round(u.x, 1), "y": round(u.y, 1),
                "hp": max(0, round(u.hp)), "max": u.maxhp,
                "team": u.team, "key": u.key, "size": u.size,
                "fly": bool(getattr(u, "flying", False)),
                "fam": fam_of(u.key),
                "fire": 1 if getattr(u, "fire_flash", 0) > 0 else 0,
                "face": int(getattr(u, "face", 1)),
                "col": col(u.key),
            } for u in g.units if u.alive]
            towers = [{
                "x": round(t.x, 1), "y": round(t.y, 1),
                "hp": max(0, round(t.hp)), "max": t.maxhp, "team": t.team,
            } for t in g.towers if t.alive]
            # display-only dying corpses so web spectators see the crush too
            dying = [{
                "x": round(dc["x"], 1), "y": round(dc["y"], 1),
                "team": dc["team"], "fam": dc["fam"], "size": dc["size"],
                "col": (list(dc["paper"]) if not isinstance(dc["paper"], list) else dc["paper"]),
                "p": round(min(1.0, dc["t"] / dc["dur"]), 3),
                "tip": dc.get("tip", 1), "fly": dc.get("fly", False),
            } for dc in getattr(g, "dying", [])]
            return {
                "ok": True, "geo": self.geo, "title": self.title,
                "tick": g.tick, "ink": round(getattr(g, "ink", 0), 1),
                "aiink": round(getattr(g, "ai_ink", 0), 1),
                "result": g.result,
                "linked": self.linked_match,
                "seats": list(self.seat_names),
                "units": units, "towers": towers, "dying": dying,
                "event": getattr(self, "event", None),
                "hand": list(getattr(g, "hand", lambda: [])() ) if callable(getattr(g, "hand", None)) else [],
            }


# ════════════════════════════════════════════════════════════════════════════
#  TOURNAMENT STATE — thin wrapper around tournament.py
# ════════════════════════════════════════════════════════════════════════════
class TournamentState:
    def __init__(self):
        self.lock = threading.Lock()
        self.t = None
        self.style = "single"

    def create(self, players, style="single", seed_style="as_entered", seed=0):
        if not TOURN_OK:
            return {"ok": False, "err": "tournament module unavailable"}
        players = [p.strip() for p in players if p and p.strip()]
        if len(players) < 2:
            return {"ok": False, "err": "need at least 2 players"}
        with self.lock:
            self.style = style
            self.t = _tourn.Tournament(players, style=style, seed_style=seed_style, seed=seed)
        return self.state()

    def report(self, match_id, winner):
        with self.lock:
            if not self.t:
                return {"ok": False, "err": "no tournament"}
            try:
                self.t.report_result(match_id, winner)
            except Exception as e:
                return {"ok": False, "err": str(e)}
        return self.state()

    def state(self):
        with self.lock:
            if not self.t:
                return {"ok": True, "active": False}
            d = self.t.to_dict()
            d["ok"] = True
            d["active"] = True
            d["next"] = [m.to_dict() for m in self.t.next_matches()]
            return d


GAME = None
TSTATE = TournamentState()


# ════════════════════════════════════════════════════════════════════════════
#  HTTP HANDLER
# ════════════════════════════════════════════════════════════════════════════
class Handler(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass   # quiet

    def _send(self, code, body, ctype="text/html; charset=utf-8"):
        if isinstance(body, str):
            body = body.encode("utf-8")
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Access-Control-Allow-Origin", "*")
        self.end_headers()
        try:
            self.wfile.write(body)
        except (BrokenPipeError, ConnectionResetError):
            pass

    def _json(self, obj, code=200):
        self._send(code, json.dumps(obj), "application/json")

    def do_GET(self):
        path = self.path.split("?")[0]
        if path == "/":
            self._send(200, PAGE_INDEX)
        elif path == "/play":
            self._send(200, PAGE_PLAY)
        elif path == "/control":
            self._send(200, PAGE_CONTROL)
        elif path == "/state":
            snap = GAME.snapshot() if GAME else {"ok": False}
            # count this poller as a live spectator (token passed as ?t=)
            if GAME is not None:
                from urllib.parse import urlparse, parse_qs
                q = parse_qs(urlparse(self.path).query)
                tok = (q.get("t", [""])[0])
                snap["watching"] = GAME.note_spectator(tok)
            self._json(snap)
        elif path == "/api/tournament":
            self._json(TSTATE.state())
        elif path == "/api/cards":
            self._json(_card_catalog())
        elif path == "/api/status":
            # a light landing-page summary: is a match live, its title, unit counts
            st = {"ok": True, "tournament": TSTATE.state().get("active", False)}
            if GAME is not None:
                snap = GAME.snapshot()
                st.update({
                    "live": bool(snap.get("ok")),
                    "title": snap.get("title", ""),
                    "units": len(snap.get("units", [])),
                    "result": snap.get("result"),
                    "linked": snap.get("linked"),
                    "seats_taken": len(GAME.seats),
                })
            self._json(st)
        else:
            self._send(404, "not found")

    def do_POST(self):
        path = self.path.split("?")[0]
        length = int(self.headers.get("Content-Length", 0))
        raw = self.rfile.read(length) if length else b"{}"
        try:
            data = json.loads(raw.decode("utf-8") or "{}")
        except Exception:
            data = {}
        if path == "/api/tournament/create":
            self._json(TSTATE.create(
                data.get("players", []), data.get("style", "single"),
                data.get("seed_style", "as_entered"), int(data.get("seed", 0))))
        elif path == "/api/tournament/report":
            self._json(TSTATE.report(data.get("match"), data.get("winner")))
        elif path == "/api/tournament/cast":
            # launch a live match for a bracket pairing; when it finishes the
            # winner is auto-reported back to the bracket.
            mid = data.get("match")
            a, b = data.get("a"), data.get("b")
            arena = data.get("arena", "study")
            if GAME is None:
                self._json({"ok": False, "err": "no game server"}, 400); return
            GAME.cast_match(mid, a, b, arena=arena,
                            on_result=lambda m, w: TSTATE.report(m, w),
                            ai=bool(data.get("ai", True)))
            self._json({"ok": True, "cast": mid})
        elif path == "/api/game/reset":
            if GAME:
                GAME.reset(arena=data.get("arena"), seed=data.get("seed"))
                GAME.title = data.get("title", GAME.title)
            self._json({"ok": True})
        elif path == "/api/game/deploy":
            if GAME:
                GAME.queue_deploy(data.get("key"), data.get("x", 0),
                                  data.get("y", 0), data.get("team", 0))
            self._json({"ok": True})
        elif path == "/api/game/seat":
            # a browser claims a team to play head-to-head. Returns its team, or
            # null if both seats are taken (they can still spectate).
            if GAME is None:
                self._json({"ok": False}, 400); return
            token = data.get("token", "")
            team = GAME.claim_seat(token) if token else None
            self._json({"ok": True, "team": team,
                        "seats_taken": len(GAME.seats), "ai": GAME.ai})
        else:
            self._json({"ok": False, "err": "unknown endpoint"}, 404)


def _card_catalog():
    try:
        import pygame, game
        if not pygame.get_init():
            os.environ.setdefault("SDL_VIDEODRIVER", "dummy")
            os.environ.setdefault("SDL_AUDIODRIVER", "dummy")
            pygame.init()
        out = []
        for k in game.DEFAULT_DECK:
            d = game.CARDS[k]
            c = d.get("paper", (170, 150, 110))
            out.append({"key": k, "name": d["name"], "cost": d["cost"],
                        "col": list(c) if not isinstance(c, list) else c})
        return {"ok": True, "cards": out}
    except Exception as e:
        return {"ok": False, "err": str(e)}


def lan_ip():
    """Best-effort primary LAN IP (the address other devices can reach)."""
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        s.connect(("10.255.255.255", 1))   # no packets sent; just picks the iface
        ip = s.getsockname()[0]
    except Exception:
        ip = "127.0.0.1"
    finally:
        s.close()
    return ip


def serve(port=8080, arena="study", demo=False):
    global GAME
    GAME = GameServer(arena=arena, ai=demo)
    GAME.start()          # always run the sim loop (deploys/seats need it live)
    httpd = ThreadingHTTPServer(("0.0.0.0", port), Handler)
    ip = lan_ip()
    print("┌───────────────────────────────────────────────")
    print("│  PAPERBOUND — web host is live")
    print(f"│  On this device : http://localhost:{port}/")
    print(f"│  On the LAN     : http://{ip}:{port}/")
    print(f"│  Control page   : http://{ip}:{port}/control")
    print(f"│  Spectate/play  : http://{ip}:{port}/play")
    print("│  (Ctrl-C to stop)")
    print("└───────────────────────────────────────────────")
    try:
        httpd.serve_forever()
    except KeyboardInterrupt:
        print("\nshutting down…")
    finally:
        if GAME:
            GAME.stop()
        httpd.server_close()


# The web pages are large; kept in a companion module to keep this file readable.
from webpages import PAGE_INDEX, PAGE_PLAY, PAGE_CONTROL


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=8080)
    ap.add_argument("--arena", default="study")
    ap.add_argument("--demo", action="store_true", help="run a self-playing match")
    args = ap.parse_args()
    serve(args.port, args.arena, args.demo)
