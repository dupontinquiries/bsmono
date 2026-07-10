"""
Paperbound → webhost-kit adapter.

Serves the actual paperbound sim over the shared browser/LAN host (spectate + seat + tournament).
This is the whole game-specific part: wrap `game.Game`'s snapshot/deploy/update behind the kit's
GameAdapter seam. Everything else (HTTP, spectators, seats, brackets) comes from webhost-kit.

Run:  python3 webadapter.py            # serves paperbound on http://<lan>:8080/
(paperbound's own three.js renderer would be returned from play_page(); omitted here so the kit's
generic JSON view is used, keeping this to just the data seam.)
"""
import os
import sys

# the shared kit (this integration layer is allowed to depend on the monorepo path; core gameplay
# in game.py/tournament.py is not).
sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "..", "shared", "webhost-kit"))

from webserve import WebHost, _AdapterDefaults  # noqa: E402
import game as _g  # noqa: E402


class PaperboundAdapter(_AdapterDefaults):
    title = "Paperbound"

    def __init__(self, seed: int = 1):
        self.g = _g.Game(seed=seed)

    def snapshot(self) -> dict:
        g = self.g
        units = [
            {"id": getattr(u, "uid", 0), "x": round(u.x, 1), "y": round(u.y, 1),
             "hp": max(0, round(u.hp)), "team": u.team, "key": u.key}
            for u in g.units if u.alive
        ]
        towers = [
            {"x": round(t.x, 1), "y": round(t.y, 1), "hp": max(0, round(t.hp)), "team": t.team}
            for t in g.towers if t.alive
        ]
        return {"ok": True, "title": self.title, "tick": g.tick,
                "ink": round(getattr(g, "ink", 0.0), 1), "units": units, "towers": towers}

    def apply_action(self, action: dict, seat):
        try:
            self.g.deploy(action["key"], float(action["x"]), float(action["y"]),
                          seat if seat in (0, 1) else 0)
            return {"ok": True}
        except Exception as e:
            return {"ok": False, "err": str(e)}

    def step(self) -> None:
        self.g.update()

    def catalog(self) -> dict:
        return {"ok": True, "actions": list(getattr(_g, "CARDS", {}).keys())}


if __name__ == "__main__":
    WebHost(PaperboundAdapter()).serve(8080)
