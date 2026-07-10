# webhost-kit — shared browser/LAN host for the games

Paperbound shipped a genuinely nice capability: a host device runs a tiny stdlib web server, prints a
LAN URL, and anyone on the Wi-Fi opens it in a **browser** to spectate a live match in 3-D, *claim a
seat* to play head-to-head, or (from a phone) run a **tournament** — build a bracket, tap the winner,
rounds advance, and a pairing can be "cast" live with the result auto-reported back.

Almost none of that is paperbound-specific. This kit lifts the game-agnostic bones so **every game can
offer the same thing** — spectate, seat, and tournament — while each game supplies only the parts that
actually differ: its live snapshot, its actions, and its renderer.

## What's in here
- **`tourney.py`** — the bracket engine (single/double elim, round-robin, swiss; seeding
  as_entered/random/ranked/snake). Pure logic, lifted verbatim from paperbound, self-tested
  (`python3 tourney.py`). 100% game-agnostic.
- **`webserve.py`** — the HTTP host: `WebHost(adapter).serve(port)`. Owns the server, the
  `/state` poll + `/api` action endpoints, spectator counting, two-seat head-to-head claiming, the LAN
  share URL, and the whole tournament control page + `/api/tournament/*`. Stdlib only. Self-tested
  (`python3 webserve.py --selftest`) and runnable (`python3 webserve.py` → a demo game).

## The seam: `GameAdapter`
A game plugs in by implementing a small adapter. Only `title`, `snapshot`, `apply_action`, `step` are
required; the rest have defaults (`_AdapterDefaults`).

```python
class MyGame(_AdapterDefaults):
    title = "mygame"
    def snapshot(self) -> dict:                 # the live state browsers render
        return {"ok": True, "units": [...]}
    def apply_action(self, action, seat) -> dict:  # a browser deploy/move from seat 0/1 (or None)
        ...
    def step(self) -> None:                     # advance the authoritative sim one tick (~10 Hz)
        ...
    # optional: catalog() (palette), play_page() (game's own /play renderer), reset(**opts)

WebHost(MyGame()).serve(8080)
```

Everything else — spectators, seat claiming, the tournament bracket + phone control page, the share
URL — comes for free.

## Plugging in each kind of game
- **Python games (paperbound):** implement `GameAdapter` directly over the sim (paperbound's
  `game.Game` already exposes `snapshot()` / `deploy()` / `update()` — a ~20-line adapter). Its
  folded-paper three.js renderer stays paperbound-specific and is returned from `play_page()`.
- **Rust/C games (unendlich, soldier, blackstar, clawbound):** they can't be imported into Python, so
  they plug in via a thin **socket adapter**: the game exposes its snapshot + accepts actions on a
  local UNIX/TCP socket (or via the fleet), and a `SocketAdapter` (thin `GameAdapter` that forwards
  `snapshot`/`apply_action`/`step` over that socket) fronts it. The browser/tournament layer then works
  identically. The snapshot is just JSON, so the game side is small.

## What stays game-specific (by design)
The **snapshot shape**, the **actions**, and the **`/play` renderer** (paperbound draws folded-paper
units; a shooter would draw something else). The kit ships a generic JSON `/play` fallback so a game
is watchable before it has a bespoke renderer.

## Relation to the fleet
This is the *presentation/organise* layer; `netcode` (`games/shared/netcode`) is the *discovery* layer.
They compose: a host announces itself to the fleet (see `games/paperbound/fleet.py`) so friends find
the web URL from the games centre, then the browser layer here takes over.
