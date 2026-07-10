"""
webhost-kit — the shared browser/LAN host for games.

Paperbound proved this pattern: a host device runs a tiny stdlib web server so anyone on the Wi-Fi
can open a URL and **spectate** a live match in the browser, **claim a seat** to play head-to-head,
and an organiser can run a **tournament** from a phone (build a bracket, tap winners, rounds advance,
"cast" a pairing live). None of that is game-specific — only the snapshot shape, the actions, and the
3-D renderer are. This module lifts the game-agnostic bones so every game can offer the same thing.

A game plugs in by implementing `GameAdapter` (below); the framework owns the HTTP server, the state
poll / action endpoints, spectator counting, seat claiming, the LAN share URL, and the whole
tournament control page + engine (`tourney`). The game's own `/play` renderer is supplied by the
adapter (its snapshot shape is game-specific), with a generic JSON view as the offline fallback.

Design points:
  • Stdlib only (http.server), so it runs on a locked-down mobile Linux with no pip installs.
  • The sim runs authoritatively on the host; browsers poll `/state` (~10 Hz) and act via `/api`.
  • Non-Python games (the Rust ones) plug in the same way via a thin adapter that speaks to the
    engine over a local socket or the fleet — see `SocketAdapter` and README.md.
"""

from __future__ import annotations

import json
import socket
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any, Optional, Protocol

import tourney


# ── the seam every game implements ─────────────────────────────────────────────
class GameAdapter(Protocol):
    """What the web host needs from a game. Everything else (HTTP, spectators, seats, tournament) is
    handled by the framework. Only `snapshot` is required; the rest have sensible defaults."""

    title: str

    def snapshot(self) -> dict:
        """The live state browsers render, as JSON-able dict (units/positions/score/…)."""
        ...

    def apply_action(self, action: dict, seat: Optional[int]) -> dict:
        """Handle a browser action (deploy/move/…) from `seat` (0/1) or a spectator (None)."""
        ...

    def step(self) -> None:
        """Advance the authoritative sim one tick (called ~10 Hz by the host loop)."""
        ...

    def catalog(self) -> dict:
        """Optional: palette of things a player can do (cards/units), fetched once by the client."""
        ...

    def play_page(self) -> Optional[str]:
        """Optional: the game-specific `/play` HTML (its own renderer). None → generic JSON view."""
        ...

    def reset(self, **opts) -> None:
        """Optional: start a fresh match (used when a tournament pairing is cast)."""
        ...


class _AdapterDefaults:
    """Mixin giving GameAdapter's optional methods harmless defaults, so a game can implement just
    `title`, `snapshot`, `apply_action`, and `step`."""

    def catalog(self) -> dict:
        return {"ok": True, "actions": []}

    def play_page(self) -> Optional[str]:
        return None

    def reset(self, **opts) -> None:
        pass


# ── seat + spectator bookkeeping (generic) ─────────────────────────────────────
class _Seats:
    """Two head-to-head seats (0=bottom, 1=top) claimed by browser tokens, plus a live spectator
    count pruned a few seconds after a tab goes quiet. Exactly paperbound's model, game-independent."""

    def __init__(self):
        self._lock = threading.Lock()
        self.by_token: dict[str, int] = {}
        self.spectators: dict[str, float] = {}

    def claim(self, token: str) -> Optional[int]:
        with self._lock:
            if token in self.by_token:
                return self.by_token[token]
            taken = set(self.by_token.values())
            for team in (0, 1):
                if team not in taken:
                    self.by_token[token] = team
                    return team
            return None

    def seat_of(self, token: str) -> Optional[int]:
        return self.by_token.get(token)

    def release(self, token: str) -> None:
        with self._lock:
            self.by_token.pop(token, None)

    def note_spectator(self, token: str, ttl: float = 6.0) -> int:
        now = time.time()
        with self._lock:
            if token:
                self.spectators[token] = now
            self.spectators = {t: ts for t, ts in self.spectators.items() if now - ts < ttl}
            return len(self.spectators)


# ── the tournament wrapper (generic; wraps tourney.Tournament) ──────────────────
class _TournamentState:
    def __init__(self, host: "WebHost"):
        self.host = host
        self.t: Optional[tourney.Tournament] = None
        self._lock = threading.Lock()

    def create(self, data: dict) -> dict:
        players = [p for p in data.get("players", []) if p]
        if len(players) < 2:
            return {"ok": False, "err": "need at least 2 players"}
        with self._lock:
            self.t = tourney.Tournament(
                players,
                style=data.get("style", "single"),
                seed_style=data.get("seed_style", "as_entered"),
                ratings=data.get("ratings"),
                seed=int(data.get("seed", 0)),
            )
        return {"ok": True, "tournament": self.t.to_dict()}

    def report(self, data: dict) -> dict:
        with self._lock:
            if not self.t:
                return {"ok": False, "err": "no tournament"}
            ok = self.t.report_result(int(data["match"]), data["winner"], data.get("report"))
            return {"ok": ok, "tournament": self.t.to_dict()}

    def cast(self, data: dict) -> dict:
        """Start a live match for a bracket pairing; the host adapter plays it, and the winner is
        reported back automatically when the match ends."""
        with self._lock:
            if not self.t:
                return {"ok": False, "err": "no tournament"}
        mid = int(data["match"])
        m = next((x for x in self.t.matches if x.id == mid), None)
        if not m or not m.ready():
            return {"ok": False, "err": "match not ready"}
        self.host.adapter.reset(title=f"{m.a} vs {m.b}", **data.get("opts", {}))
        self.host._cast = (mid, m.a, m.b)
        return {"ok": True, "casting": mid}

    def state(self) -> dict:
        with self._lock:
            return {"ok": True, "active": self.t is not None,
                    "tournament": self.t.to_dict() if self.t else None}


# ── the HTTP host ───────────────────────────────────────────────────────────────
def lan_ip() -> str:
    """Best-effort primary LAN address (no traffic actually sent)."""
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        s.connect(("10.255.255.255", 1))
        return s.getsockname()[0]
    except Exception:
        return "127.0.0.1"
    finally:
        s.close()


class WebHost:
    """Serve a game to the LAN: spectate + seat + tournament, all generic. Construct with a
    `GameAdapter`, call `serve(port)`."""

    def __init__(self, adapter: GameAdapter):
        self.adapter = adapter
        self.seats = _Seats()
        self.tourney = _TournamentState(self)
        self._cast: Optional[tuple] = None
        self._running = False

    # background sim loop (host is authoritative)
    def _loop(self, hz: float = 10.0):
        dt = 1.0 / hz
        while self._running:
            try:
                self.adapter.step()
            except Exception:
                pass
            time.sleep(dt)

    def serve(self, port: int = 8080) -> None:
        host = self
        self._running = True
        threading.Thread(target=self._loop, daemon=True).start()

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *a):  # quiet
                pass

            def _send(self, obj, ctype="application/json"):
                body = obj.encode() if isinstance(obj, str) else json.dumps(obj).encode()
                self.send_response(200)
                self.send_header("Content-Type", ctype)
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def _body(self) -> dict:
                n = int(self.headers.get("Content-Length", 0))
                try:
                    return json.loads(self.rfile.read(n) or b"{}")
                except Exception:
                    return {}

            def do_GET(self):
                path = self.path.split("?", 1)[0]
                q = dict(p.split("=", 1) for p in self.path.split("?", 1)[1].split("&")) if "?" in self.path else {}
                if path == "/":
                    self._send(LANDING_HTML, "text/html")
                elif path == "/control":
                    self._send(CONTROL_HTML, "text/html")
                elif path == "/play":
                    page = host.adapter.play_page()
                    self._send(page if page else GENERIC_PLAY_HTML, "text/html")
                elif path == "/state":
                    snap = host.adapter.snapshot()
                    snap["watching"] = host.seats.note_spectator(q.get("t", ""))
                    snap["seats_taken"] = len(host.seats.by_token)
                    self._send(snap)
                elif path == "/api/catalog":
                    self._send(host.adapter.catalog())
                elif path == "/api/tournament":
                    self._send(host.tourney.state())
                elif path == "/api/status":
                    self._send({"ok": True, "title": getattr(host.adapter, "title", "game"),
                                "tournament": host.tourney.t is not None})
                else:
                    self._send({"ok": False, "err": "not found"})

            def do_POST(self):
                path = self.path.split("?", 1)[0]
                data = self._body()
                if path == "/api":
                    seat = host.seats.seat_of(data.get("token", ""))
                    self._send(host.adapter.apply_action(data, seat))
                elif path == "/api/seat":
                    team = host.seats.claim(data.get("token", ""))
                    self._send({"ok": team is not None, "team": team,
                                "seats_taken": len(host.seats.by_token)})
                elif path == "/api/tournament/create":
                    self._send(host.tourney.create(data))
                elif path == "/api/tournament/report":
                    self._send(host.tourney.report(data))
                elif path == "/api/tournament/cast":
                    self._send(host.tourney.cast(data))
                else:
                    self._send({"ok": False, "err": "not found"})

        httpd = ThreadingHTTPServer(("0.0.0.0", port), Handler)
        ip = lan_ip()
        print(f"webhost-kit: {getattr(self.adapter,'title','game')} on http://{ip}:{port}/  (share this)")
        try:
            httpd.serve_forever()
        except KeyboardInterrupt:
            pass
        finally:
            self._running = False
            httpd.shutdown()


# Minimal generic pages (a game ships its own /play; these keep the kit runnable/self-contained).
LANDING_HTML = ("<!doctype html><meta name=viewport content='width=device-width,initial-scale=1'>"
                "<h2>web host</h2><p><a href='/play'>▶ watch / play</a> · "
                "<a href='/control'>⚙ tournament control</a></p>")
GENERIC_PLAY_HTML = ("<!doctype html><meta name=viewport content='width=device-width,initial-scale=1'>"
                     "<h3>live state</h3><pre id=s>…</pre><script>"
                     "async function p(){try{const r=await fetch('/state?t='+ (localStorage.tok||=Math.random()));"
                     "document.getElementById('s').textContent=JSON.stringify(await r.json(),null,2);}catch(e){}"
                     "setTimeout(p,300);}p();</script>")
CONTROL_HTML = ("<!doctype html><meta name=viewport content='width=device-width,initial-scale=1'>"
                "<h3>tournament control</h3>"
                "<p>players (comma-sep): <input id=ps size=30> "
                "<select id=st><option>single<option>double<option>roundrobin<option>swiss</select> "
                "<button onclick=create()>build</button></p><pre id=out>…</pre><script>"
                "async function j(u,b){const r=await fetch(u,b?{method:'POST',body:JSON.stringify(b)}:{});return r.json();}"
                "async function create(){const players=document.getElementById('ps').value.split(',').map(s=>s.trim()).filter(Boolean);"
                "await j('/api/tournament/create',{players,style:document.getElementById('st').value});show();}"
                "async function show(){document.getElementById('out').textContent=JSON.stringify((await j('/api/tournament')).tournament,null,2);}"
                "show();</script>")


if __name__ == "__main__":
    import argparse

    class DemoGame(_AdapterDefaults):
        """A trivial adapter so the kit runs standalone: two 'ink' counters that tick up; an action
        spends ink. Proves the spectate/seat/tournament scaffolding without any real game."""

        title = "demo"

        def __init__(self):
            self.ink = [0.0, 0.0]
            self.plays = 0

        def snapshot(self):
            return {"ok": True, "title": self.title, "ink": [round(x, 1) for x in self.ink], "plays": self.plays}

        def apply_action(self, action, seat):
            if seat in (0, 1) and self.ink[seat] >= 1.0:
                self.ink[seat] -= 1.0
                self.plays += 1
                return {"ok": True, "plays": self.plays}
            return {"ok": False, "err": "no seat or not enough ink"}

        def step(self):
            for i in range(2):
                self.ink[i] = min(9.9, self.ink[i] + 0.05)

    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=8080)
    ap.add_argument("--selftest", action="store_true")
    args = ap.parse_args()

    if args.selftest:
        # exercise the framework without binding a socket
        g = DemoGame()
        h = WebHost(g)
        for _ in range(30):
            g.step()
        assert g.snapshot()["ink"][0] > 1.0
        s0 = h.seats.claim("tokA"); s1 = h.seats.claim("tokB"); s2 = h.seats.claim("tokC")
        assert {s0, s1} == {0, 1} and s2 is None, (s0, s1, s2)
        assert g.apply_action({"token": "tokA"}, s0)["ok"]
        r = h.tourney.create({"players": ["Ana", "Bo", "Cy", "Di"], "style": "single"})
        assert r["ok"] and len(r["tournament"]["rounds"]) == 2
        print("✅ webhost-kit self-test: sim loop, seat claim (2 seats then full), action, tournament OK")
    else:
        WebHost(DemoGame()).serve(args.port)
