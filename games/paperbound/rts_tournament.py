"""
Foldworks tournament — a Halo Wars-flavoured skirmish ladder.

Every faction in the current book (core + enabled expansion races) is seeded
into a single-elimination bracket alongside the player. The player fights
their own matches in the RTS; the other pairings are resolved by the herald
(a seeded power heuristic over each faction's roster — documented below, and
deliberately a *flavourful* simulation, not a fair one: upsets happen).

State lives in tournament.json beside this module, so a run survives quitting
to the menu or the desktop — the next RTS launch resumes at your next match.

Bracket shape: the entrant list is padded to the next power of two with byes.
The player is seeded last so their first match is always real.

API (used by rts.py):
    new(player_fac, all_facs)   → start a run, returns the state
    load() / save(state)        → persistence
    active()                    → state | None (finished runs are inactive)
    current_match(state)        → dict(round, idx, a, b) — the player's next
    report_player(state, won)   → record the player's result, resolve the rest
                                  of the round via the herald, advance
    champion(state)             → faction key | None
    bracket_lines(state, names) → printable bracket summary for the end screen
"""

import json
import os
import random

_PATH = os.path.join(os.path.dirname(os.path.abspath(__file__)), "tournament.json")

PLAYER = "__player__"      # marker: the player's slot carries their faction too


def _power_of_two_at_least(n):
    p = 1
    while p < n:
        p *= 2
    return p


def new(player_fac, all_facs, seed=None):
    """Seed a fresh single-elim bracket: every faction enters once; the player
    enters as themselves (their faction fights twice — once in your hands,
    once in the herald's, which is half the fun)."""
    rng = random.Random(seed)
    entrants = list(all_facs)
    rng.shuffle(entrants)
    entrants.append(PLAYER)                     # player seeded last
    size = _power_of_two_at_least(len(entrants))
    while len(entrants) < size:
        entrants.insert(rng.randrange(max(1, len(entrants) - 1)), None)  # byes
    state = {
        "player_fac": player_fac,
        "seed": rng.randrange(1 << 30),
        "rounds": [entrants],                   # rounds[i] = slot list
        "results": [],                          # per round: list of winners
        "done": False,
    }
    save(state)
    return state


def save(state):
    tmp = _PATH + ".tmp"
    with open(tmp, "w") as f:
        json.dump(state, f, indent=1)
    os.replace(tmp, _PATH)


def load():
    try:
        with open(_PATH) as f:
            return json.load(f)
    except Exception:
        return None


def clear():
    try:
        os.remove(_PATH)
    except FileNotFoundError:
        pass


def active():
    st = load()
    return st if st and not st.get("done") else None


def _pairs(slots):
    return [(slots[i], slots[i + 1]) for i in range(0, len(slots), 2)]


def current_match(state):
    """The player's pending pairing in the newest round, or None."""
    slots = state["rounds"][-1]
    for i, (a, b) in enumerate(_pairs(slots)):
        if PLAYER in (a, b):
            other = b if a == PLAYER else a
            if other is None:
                return None                     # bye — resolved on report
            return {"round": len(state["rounds"]) - 1, "idx": i,
                    "a": state["player_fac"] if a == PLAYER else a,
                    "b": state["player_fac"] if b == PLAYER else b,
                    "player_side": 0 if a == PLAYER else 1}
    return None


def _power(fac_key, factions, rng):
    """The herald's estimate of a roster: economy, average combat stats and a
    seeded roll. Flavour, not fairness — a ~25% swing keeps upsets alive."""
    fac = factions.get(fac_key)
    if not fac:
        return rng.uniform(0.8, 1.2)
    units = [u for u in fac.get("units", {}).values() if not u.get("worker")]
    if not units:
        return rng.uniform(0.8, 1.2)
    avg = sum(u.get("hp", 100) * 0.004 + u.get("dmg", 10) * 0.05
              + u.get("spd", 60) * 0.004 for u in units) / len(units)
    econ = fac.get("econ", 1.0)
    return avg * econ * rng.uniform(0.75, 1.25)


def report_player(state, won, factions):
    """Record the player's match, let the herald resolve every other pairing
    of this round, and either advance to a fresh round or crown a champion."""
    rng = random.Random(state["seed"] + len(state["rounds"]) * 977)
    slots = state["rounds"][-1]
    winners = []
    for a, b in _pairs(slots):
        if a is None and b is None:
            winners.append(None)
        elif a is None or b is None:
            winners.append(a if b is None else b)      # bye
        elif PLAYER in (a, b):
            other = b if a == PLAYER else a
            winners.append(PLAYER if won else other)
        else:
            winners.append(a if _power(a, factions, rng)
                           >= _power(b, factions, rng) else b)
    state["results"].append(winners)
    if len(winners) == 1:
        state["done"] = True
        state["champion"] = winners[0]
    else:
        state["rounds"].append(winners)
        # the player may have drawn a bye next round: auto-advance through it
        nxt = state["rounds"][-1]
        for i, (a, b) in enumerate(_pairs(nxt)):
            pass
    save(state)
    return state


def champion(state):
    c = state.get("champion")
    if c == PLAYER:
        return state.get("player_fac")
    return c


def player_alive(state):
    return PLAYER in state["rounds"][-1] and not state.get("done")


def round_name(state):
    n = len(state["rounds"][-1])
    return {2: "GRAND FINAL", 4: "SEMI-FINAL", 8: "QUARTER-FINAL"}.get(
        n, f"ROUND OF {n}")


def bracket_lines(state, names):
    """A printable summary of the newest round for the end/setup screens."""
    def nm(k):
        if k is None:
            return "—"
        if k == PLAYER:
            return f"YOU ({names.get(state['player_fac'], state['player_fac'])})"
        return names.get(k, k)
    out = [round_name(state)]
    for a, b in _pairs(state["rounds"][-1]):
        out.append(f"{nm(a)}  vs  {nm(b)}")
    return out


if __name__ == "__main__":
    # selftest: full runs with a fake faction book
    facs = {k: {"econ": 1.0, "units": {"s": {"hp": 200, "dmg": 15, "spd": 70}}}
            for k in ["court", "ember", "tide", "press", "swarm", "windwrit",
                      "rootbound"]}
    clear()
    st = new("court", list(facs.keys()), seed=42)
    assert len(st["rounds"][0]) == 8, "7 factions + player pad to 8"
    n_rounds = 0
    while not st["done"]:
        m = current_match(st)
        if m is not None:
            assert st["player_fac"] in (m["a"], m["b"])
        st = report_player(st, won=True, factions=facs)
        n_rounds += 1
        assert n_rounds < 10, "runaway bracket"
    assert champion(st) == "court", "player won every match → player champion"
    assert n_rounds == 3, f"8-bracket is 3 rounds, got {n_rounds}"
    # losing run: champion is some AI faction, player drops out
    clear()
    st = new("ember", list(facs.keys()), seed=7)
    st = report_player(st, won=False, factions=facs)
    assert not player_alive(st)
    while not st["done"]:
        # herald resolves the rest without a player match
        st = report_player(st, won=False, factions=facs)
    assert champion(st) in facs, champion(st)
    # persistence round-trips
    st2 = load()
    assert st2["done"] and st2.get("champion") == st.get("champion")
    clear()
    # determinism: same seed → same bracket
    a = new("tide", list(facs.keys()), seed=99)["rounds"][0]
    clear()
    b = new("tide", list(facs.keys()), seed=99)["rounds"][0]
    clear()
    assert a == b
    print("[tournament] selftest OK — full winning run, losing run, persistence, determinism")
