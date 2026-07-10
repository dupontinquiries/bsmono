"""
Paperbound — Auto-Caster.

Turns a match into a story. The caster observes the battle tick by tick (either a
live Game or a replayed one) and emits short prose entries as things happen:

  • a summoning       — "Turn 3: The Silverback is folded onto the west bank."
  • an engagement     — "The Wolverine tears into the Paper Crane at the spine."
  • a fall            — "The Crane falls, mid-lane, after dealing 214 damage —
                          its last strike aimed at the Wolverine."

It works purely by WATCHING game state each tick and diffing it: new units are
summons, dropped HP is an engagement (attributed to the nearest enemy), and a
unit that vanishes has fallen. None of this touches the simulation, so it's safe
to run alongside a live match or over a deterministic replay — it only reads.

The caster keeps its OWN bookkeeping (cumulative damage each unit has dealt, and
who it was last trading blows with), so a fall can be narrated with real context:
how much it dealt, where on the page it died, and what it was fighting.

Public API:
    caster = Caster()
    caster.observe(game)      # call once per sim tick; appends any new entries
    caster.entries            # list of dicts: {tick, kind, text}
    caster.transcript()       # the whole story as one string
"""
import math
import random


# ── flavour vocabulary ───────────────────────────────────────────────────────
_LANES = [(0.0, 0.33, "west bank"), (0.33, 0.66, "centre"), (0.66, 1.01, "east bank")]
_DEPTHS = [(0.0, 0.25, "deep in enemy ground"), (0.25, 0.45, "in the far field"),
           (0.45, 0.55, "at the spine"), (0.55, 0.75, "on home soil"),
           (0.75, 1.01, "guarding the back line")]

_SUMMON_VERBS = ["is folded onto", "unfurls at", "takes shape on", "is creased into being at",
                 "rises at", "is loosed upon"]
_ENGAGE_VERBS = ["tears into", "trades blows with", "crashes against", "presses",
                 "harries", "closes on", "grinds against"]
_FALL_VERBS = ["falls", "is unmade", "crumples", "comes apart", "is folded flat",
               "returns to pulp"]

# a caster keeps its tone consistent per match via a seeded picker
class _Voice:
    def __init__(self, seed):
        self.r = random.Random(seed & 0xffffffff)
    def pick(self, opts):
        return opts[self.r.randrange(len(opts))]


def _lane(x, left, width):
    f = (x - left) / max(1.0, width)
    for lo, hi, name in _LANES:
        if lo <= f < hi:
            return name
    return "centre"


def _depth(y, top, height, team):
    f = (y - top) / max(1.0, height)
    if team == 1:                       # flip perspective for the top team
        f = 1.0 - f
    for lo, hi, name in _DEPTHS:
        if lo <= f < hi:
            return name
    return "mid-lane"


def _side(team):
    return {0: "the bottom army", 1: "the top army", 2: "the risen horde"}.get(team, "a third force")


class Caster:
    def __init__(self, seed=0, name_of=None):
        """`name_of(key)->str` maps a card key to a display name; if omitted the
        key is titled as-is. `seed` keeps the flavour stable for a given match."""
        self.voice = _Voice(seed or 1)
        self.name_of = name_of or (lambda k: str(k).title())
        self.entries = []            # {tick, kind, text}
        self._prev = {}              # uid -> snapshot {hp, x, y, team, key}
        self._dmg = {}               # uid -> cumulative damage this unit has dealt
        self._last_foe = {}          # uid -> name of who it last traded blows with
        self._announced_deploy = set()
        self._geo = None
        self._started = False

    # ── main entry: call once per sim tick ──
    def observe(self, game):
        geo = self._grab_geo(game)
        cur = {}
        for u in getattr(game, "units", []):
            uid = getattr(u, "uid", id(u))
            cur[uid] = dict(hp=float(getattr(u, "hp", 0)), x=float(u.x), y=float(u.y),
                            team=int(u.team), key=getattr(u, "key", "?"),
                            maxhp=float(getattr(u, "maxhp", 1)),
                            zombie=bool(getattr(u, "is_zombie", False)))
        tick = int(getattr(game, "tick", len(self.entries)))

        if not self._started:
            self._started = True
            self._emit(tick, "open",
                       f"The page turns. {_side(0).capitalize()} and {_side(1)} take the field.")

        # summons — units present now that weren't before. Swarms (many units of
        # the same card summoned the same tick) collapse into one line.
        fresh = [(uid, s) for uid, s in cur.items()
                 if uid not in self._prev and uid not in self._announced_deploy]
        summon_groups = {}
        for uid, s in fresh:
            self._announced_deploy.add(uid)
            self._dmg[uid] = 0.0
            gkey = (s["key"], s["team"], s.get("zombie", False))
            summon_groups.setdefault(gkey, []).append(s)
        for (key, team, zomb), members in summon_groups.items():
            self._narrate_summon(tick, members[0], geo, count=len(members))

        # engagements — attribute each unit's HP loss to its nearest enemy, and
        # credit that enemy with the damage (our own tally, not the sim's)
        for uid, s in cur.items():
            prev = self._prev.get(uid)
            if not prev:
                continue
            lost = prev["hp"] - s["hp"]
            if lost > 0.5:
                attacker = self._nearest_enemy(uid, s, cur)
                if attacker is not None:
                    aid, asnap = attacker
                    self._dmg[aid] = self._dmg.get(aid, 0.0) + lost
                    aname = self.name_of(asnap["key"])
                    dname = self.name_of(s["key"])
                    self._last_foe[aid] = dname
                    self._last_foe[uid] = aname
                    self._maybe_narrate_engage(tick, aid, asnap, uid, s, geo)

        # falls — units that were present last tick and are gone now
        for uid, prev in self._prev.items():
            if uid not in cur:
                self._narrate_fall(tick, uid, prev, geo)

        self._prev = cur

    # ── narration helpers ──
    def _narrate_summon(self, tick, s, geo, count=1):
        name = self.name_of(s["key"])
        lane = _lane(s["x"], geo["left"], geo["width"])
        if s.get("zombie"):
            self._emit(tick, "summon",
                       f"A fallen shape claws back up on the {lane} — {name}, risen and hostile to all.")
            return
        verb = self.voice.pick(_SUMMON_VERBS)
        if count > 1:
            self._emit(tick, "summon",
                       f"{self._turn(tick)}A flight of {name} ({count}) {verb} the {lane}, "
                       f"called up by {_side(s['team'])}.")
        else:
            self._emit(tick, "summon",
                       f"{self._turn(tick)}{name} {verb} the {lane}, called up by {_side(s['team'])}.")

    _engage_cooldown = {}
    def _maybe_narrate_engage(self, tick, aid, asnap, did, dsnap, geo):
        # throttle: at most one engage line per attacker per ~2s, so the story
        # isn't a wall of every blow
        last = self._engage_cooldown.get(aid, -999)
        if tick - last < 40:
            return
        self._engage_cooldown[aid] = tick
        aname = self.name_of(asnap["key"])
        dname = self.name_of(dsnap["key"])
        verb = self.voice.pick(_ENGAGE_VERBS)
        lane = _lane(dsnap["x"], geo["left"], geo["width"])
        self._emit(tick, "engage", f"{aname} {verb} {dname} on the {lane}.")

    def _narrate_fall(self, tick, uid, prev, geo):
        name = self.name_of(prev["key"])
        dealt = int(self._dmg.get(uid, 0.0))
        lane = _lane(prev["x"], geo["left"], geo["width"])
        depth = _depth(prev["y"], geo["top"], geo["height"], prev["team"])
        foe = self._last_foe.get(uid)
        verb = self.voice.pick(_FALL_VERBS)
        clauses = [f"{depth} on the {lane}"]
        if dealt > 0:
            clauses.append(f"after dealing {dealt} damage")
        if foe:
            clauses.append(f"its last strikes aimed at the {foe}")
        self._emit(tick, "fall", f"The {name} {verb}, " + ", ".join(clauses) + ".")

    def _narrate_result(self, tick, result):
        pass

    # ── result / close ──
    def close(self, result=None, ticks=None):
        tick = int(ticks if ticks is not None else (self._prev and max(self._prev) or 0))
        if result == "win":
            txt = f"{_side(0).capitalize()} holds the page. Victory to the bottom."
        elif result == "lose":
            txt = f"{_side(1).capitalize()} overruns the spine. The top army wins."
        else:
            txt = "The page falls still."
        self._emit(tick, "close", txt)

    # ── plumbing ──
    def _emit(self, tick, kind, text):
        self.entries.append({"tick": int(tick), "kind": kind, "text": text})

    def _turn(self, tick):
        # a light "Turn N" prefix on summons, paced at ~one per 3 seconds of play
        n = max(1, tick // 60 + 1)
        return f"Turn {n}: "

    def _nearest_enemy(self, uid, s, cur):
        best = None
        bd = 1e9
        for oid, o in cur.items():
            if oid == uid or o["team"] == s["team"]:
                continue
            d = (o["x"] - s["x"]) ** 2 + (o["y"] - s["y"]) ** 2
            if d < bd:
                bd = d
                best = (oid, o)
        # only count it as "the attacker" if plausibly in reach
        if best and bd <= (180 ** 2):
            return best
        return None

    def _grab_geo(self, game):
        if self._geo:
            return self._geo
        # try the game's BOOK rect via module globals; fall back to sane defaults
        g = None
        try:
            import game as _gm
            b = _gm.BOOK
            g = dict(left=b.left, top=b.top, width=b.width, height=b.height)
        except Exception:
            g = dict(left=300, top=40, width=680, height=620)
        self._geo = g
        return g

    def transcript(self):
        return "\n".join(e["text"] for e in self.entries)


def narrate_replay(doc, name_of=None, max_ticks=None):
    """Re-simulate a replay doc deterministically and return a Caster with the
    full story. Requires the game module (uses its Game + recorded deploys)."""
    import game as gm
    rep = _load_events(doc)
    seed = int(doc.get("meta", {}).get("seed", doc.get("seed", 1)) or 1)
    caster = Caster(seed=seed, name_of=name_of or (lambda k: gm.CARDS.get(k, {}).get("name", str(k).title())))
    # build a game seeded identically and inject the deploys on their ticks
    g = gm.Game(deck=gm.DEFAULT_DECK[:], seed=seed)
    by_tick = {}
    for (tk, key, x, y, team) in rep:
        by_tick.setdefault(int(tk), []).append((key, x, y, team))
    total = int(doc.get("ticks", max(by_tick) if by_tick else 0)) or 0
    if max_ticks:
        total = min(total, max_ticks)
    for t in range(total + 1):
        for (key, x, y, team) in by_tick.get(t, []):
            try:
                g.deploy(key, x, y, team)
            except Exception:
                pass
        g.update()
        caster.observe(g)
    caster.close(result=doc.get("result"), ticks=total)
    return caster


def story_for_replay_file(path, name_of=None):
    """Load a saved replay from disk and return its narrated story as a string.
    Convenience wrapper around replay.load + narrate_replay."""
    import replay as _r
    doc = _r.load(path)
    if not doc:
        return ""
    cast = narrate_replay(doc, name_of=name_of)
    return cast.transcript()


def _load_events(doc):
    """Pull (tick,key,x,y,team) tuples out of a replay doc in either packed
    (dict with keys/rows/n) or plain-list form."""
    ev = doc.get("events")
    if isinstance(ev, dict):
        # packed form → defer to replay.py's unpacker
        try:
            import replay
            return replay._unpack_events(ev)
        except Exception:
            return []
    if ev and isinstance(ev, (list, tuple)) and isinstance(ev[0], (list, tuple)) and len(ev[0]) == 5:
        return [tuple(e) for e in ev]
    return []


if __name__ == "__main__":
    # offline self-test: drive a short synthetic battle and check the caster
    # produces sensible story beats without touching the sim.
    import os
    os.environ.setdefault("SDL_VIDEODRIVER", "dummy")
    os.environ.setdefault("SDL_AUDIODRIVER", "dummy")
    import pygame
    import sys
    sys.argv = ["t"]
    import game as gm
    pygame.init()
    gm.relayout(1280, 800)
    gm.set_arena("study")
    g = gm.Game(deck=["sabertooth", "crane", "gorilla"])
    cast = Caster(seed=7, name_of=lambda k: gm.CARDS.get(k, {}).get("name", k.title()))
    g.deploy("sabertooth", 600, 450, 0)
    g.deploy("crane", 620, 360, 1)
    for i in range(220):
        if i == 40:
            g.deploy("gorilla", 640, 460, 0)
        g.update()
        cast.observe(g)
    cast.close(result="win", ticks=220)
    kinds = {}
    for e in cast.entries:
        kinds[e["kind"]] = kinds.get(e["kind"], 0) + 1
    print("── sample transcript ──")
    for e in cast.entries[:8]:
        print("  " + e["text"])
    assert any(e["kind"] == "summon" for e in cast.entries), "no summons narrated"
    assert any(e["kind"] == "fall" for e in cast.entries), "no falls narrated"
    assert kinds.get("open") == 1 and kinds.get("close") == 1
    print(f"\n✅ caster self-test: {len(cast.entries)} entries "
          f"({', '.join(f'{k}:{v}' for k, v in sorted(kinds.items()))})")
