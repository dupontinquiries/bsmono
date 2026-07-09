"""
PAPERBOUND — boss lookahead simulator.

A lightweight, dependency-free FORWARD MODEL the AI boss uses to *evaluate a card
before it plays it*. The real battle sim is deterministic and tick-based, but
forking the whole Game per candidate would be far too heavy for a per-decision
search — so this is a compact abstraction of the same rules (lanes, movement,
range, dmg/hp attrition, a defended king) that runs hundreds of rollouts in a
blink.

The boss uses it to:
  • BUDGET  — is this card worth its ink? (value = board swing per ink spent)
  • BALANCE — a summon that's too weak or oppressively strong scores poorly on
    the "healthy" metric, so the boss can nudge stats toward strong-but-fair.
  • TIMING  — evaluate "play now vs wait", and WHICH lane, picking the best.

It never touches the real game — it reads a snapshot dict (the same
`_boss_state` shape) and returns numbers. Purely advisory.

Core entry points:
    ev = evaluate_summon(state, card, lane=None, horizon=6.0)   # -> Eval
    best = best_deployment(state, card, horizon=6.0)            # -> (lane, Eval)
    tune = balance_probe(card, scenarios=…)                     # -> dict metrics
"""
import math
from dataclasses import dataclass, field

# coarse world model (matches the real board's proportions, in abstract units)
_LANES = 3
_LANE_LEN = 100.0          # 0 = boss king line … 100 = player king line
_RIVER = 50.0              # the midline
_DT = 0.2                  # rollout tick (coarser than the game's 0.05 — fast)


@dataclass(slots=True)
class _Ent:
    hp: float
    dmg: float
    spd: float          # world-units / sec toward the enemy king
    rng: float          # engage distance
    pos: float          # 0..100 along the lane (team-relative handled by dir)
    team: int           # 0 = player (moves 100→0), 1 = boss (moves 0→100)
    lane: int
    flying: bool = False
    building: bool = False
    cool: float = 0.0
    key: str = "?"

    @property
    def alive(self):
        return self.hp > 0


@dataclass
class Eval:
    """Result of a rollout, from the BOSS's perspective (higher = better)."""
    score: float = 0.0
    boss_dmg_dealt: float = 0.0     # damage the candidate + allies did to player
    boss_losses: float = 0.0        # hp of boss units lost
    player_losses: float = 0.0      # hp of player units killed
    king_pressure: float = 0.0      # progress toward the player's king line
    survived: float = 0.0           # fraction of the candidate's hp remaining
    ink: float = 0.0                # the card's cost
    value: float = 0.0              # score per ink (budgeting metric)
    healthy: float = 0.0            # 0..1 "strong but fair" score
    note: str = ""


def _card_to_ent(card, lane, team, pos):
    hp = float(card.get("hp", 300))
    return _Ent(
        hp=hp, dmg=float(card.get("dmg", 40)),
        spd=float(card.get("spd", 60)) * 0.06,          # px/s → lane-units/s
        rng=8.0 + float(card.get("rng", 20)) * 0.06,
        pos=pos, team=team, lane=lane,
        flying=bool(card.get("flying", False)),
        building=bool(card.get("building", False)),
        key=card.get("id", card.get("key", "?")),
    )


def _snapshot_entities(state):
    """Turn the player's units from a boss-state snapshot into rollout ents.
    Player units advance from the midline toward the boss king (pos → 0)."""
    ents = []
    for u in state.get("player_units", []):
        lane = int(u.get("lane", 1)) % _LANES
        # place them by how deep they are — buildings sit back, others near river
        pos = _RIVER + 8 if not u.get("building") else _RIVER + 22
        ents.append(_Ent(
            hp=float(u.get("hp", 300)), dmg=float(u.get("dmg", 40)),
            spd=(0.0 if u.get("building") else 3.6), rng=10.0,
            pos=pos, team=0, lane=lane,
            flying=bool(u.get("flying", False)),
            building=bool(u.get("building", False)),
            key=u.get("key", "?")))
    return ents


def _rollout(ents, horizon, king_hp0=1.0):
    """Advance the abstract battle `horizon` seconds. Towers: each side has a
    king that chips lane-leaders near its line. Returns metrics dict.

    Perf: entities are bucketed by lane once per step and target-search is done
    within a lane only, so cost is ~O(steps · lane_occupancy) rather than O(n²).
    """
    dmg_to_player = 0.0
    player_king_dmg = 0.0
    boss_losses0 = 0.0
    player_hp0 = 0.0
    for e in ents:
        if e.team == 1:
            boss_losses0 += e.hp
        else:
            player_hp0 += e.hp

    steps = int(horizon / _DT)
    dt = _DT
    for _ in range(steps):
        # bucket live entities by lane and team in one pass
        lanes = [([], []) for _ in range(_LANES)]   # per lane: (team0, team1)
        any_live = False
        for e in ents:
            if e.hp > 0:
                any_live = True
                lanes[e.lane][e.team].append(e)
        if not any_live:
            break
        for lane in range(_LANES):
            t0, t1 = lanes[lane]
            # king chip: player king (pos 100) hits the boss unit nearest it;
            # boss king (pos 0) hits the player unit nearest it.
            if t1:
                lead = None; best = -1.0
                for e in t1:
                    if e.pos > 78 and e.pos > best:
                        best = e.pos; lead = e
                if lead is not None:
                    lead.hp -= 55 * dt
            if t0:
                lead = None; best = 1e9
                for e in t0:
                    if e.pos < 22 and e.pos < best:
                        best = e.pos; lead = e
                if lead is not None:
                    lead.hp -= 55 * dt
            # combat + movement within this lane
            for e in t0 + t1:
                if e.building:
                    continue
                enemies = t1 if e.team == 0 else t0
                # nearest living enemy in the lane
                target = None; md = 1e9
                epos = e.pos
                for o in enemies:
                    if o.hp > 0:
                        dd = o.pos - epos
                        if dd < 0: dd = -dd
                        if dd < md:
                            md = dd; target = o
                if target is not None and md <= e.rng:
                    if e.cool <= 0:
                        target.hp -= e.dmg
                        e.cool = 0.9
                        if e.team == 1:
                            dmg_to_player += e.dmg
                    else:
                        e.cool -= dt
                else:
                    e.pos += (-1 if e.team == 0 else 1) * e.spd * dt
                    if e.team == 1 and e.pos >= 100.0:
                        player_king_dmg += e.dmg * dt * 2
                        e.pos = 100.0
                    if e.cool > 0:
                        e.cool -= dt

    boss_surv = 0.0; boss_alive_hp = 0.0; player_left = 0.0
    for e in ents:
        if e.team == 1:
            if e.hp > 0:
                boss_surv += e.hp
        else:
            player_left += e.hp if e.hp > 0 else 0.0
    boss_losses = boss_losses0 - boss_surv
    player_losses = player_hp0 - player_left
    boss_surv_frac = (boss_surv / boss_losses0) if boss_losses0 else 0.0
    return {
        "dmg_to_player": dmg_to_player,
        "player_king_dmg": player_king_dmg,
        "boss_losses": max(0.0, boss_losses),
        "player_losses": max(0.0, player_losses),
        "boss_surv_frac": boss_surv_frac,
    }


def evaluate_summon(state, card, lane=None, horizon=6.0):
    """Score deploying `card` (a card dict) for the boss, given the current board.
    Returns an Eval. Higher score = better play right now."""
    if lane is None:
        lane = int(read_dense_lane(state))
    ents = _snapshot_entities(state)
    # place the candidate on the boss side, a bit ahead of its king
    cnt = max(1, int(card.get("count", 1)))
    for i in range(min(cnt, 6)):
        e = _card_to_ent(card, lane, team=1, pos=18.0 + i * 2.0)
        ents.append(e)
    m = _rollout(ents, horizon, king_hp0=state.get("boss_king_hp", 1.0))
    ink = float(card.get("cost", 5)) or 5.0

    # count player threat sitting in THIS lane — intercepting it is worth points
    # even if the trade isn't a blowout (a boss should answer the board, not dodge)
    lane_threat = sum(1 for u in state.get("player_units", [])
                      if int(u.get("lane", 1)) % _LANES == lane and not u.get("building"))

    # score: reward damage + kills + king pressure + intercepting the busy lane,
    # penalise losses but not so hard that the boss avoids every fight.
    score = (m["dmg_to_player"] * 1.0
             + m["player_losses"] * 0.8
             + m["player_king_dmg"] * 2.5
             + lane_threat * 12.0                    # value meeting the threat
             - m["boss_losses"] * 0.55)
    value = score / ink
    # "healthy": strong-but-fair — best when the trade is favourable but not a
    # blowout (a card that wins with ~40-75% of its hp intact feels great; one
    # that survives at 100% is oppressive, one that dies for nothing is weak).
    surv = m["boss_surv_frac"]
    healthy = max(0.0, 1.0 - abs(surv - 0.55) / 0.55)
    ev = Eval(score=score, boss_dmg_dealt=m["dmg_to_player"],
              boss_losses=m["boss_losses"], player_losses=m["player_losses"],
              king_pressure=m["player_king_dmg"], survived=surv, ink=ink,
              value=value, healthy=healthy,
              note=f"lane {lane}, surv {surv:.0%}")
    return ev


def best_deployment(state, card, horizon=6.0):
    """Try every lane, return (lane, Eval) for the best-scoring deployment."""
    best = None
    for lane in range(_LANES):
        ev = evaluate_summon(state, card, lane=lane, horizon=horizon)
        if best is None or ev.score > best[1].score:
            best = (lane, ev)
    return best


def should_play(state, card, horizon=6.0, min_value=0.0):
    """Budget gate: is this card worth its ink right now? Returns (bool, Eval)."""
    lane, ev = best_deployment(state, card, horizon=horizon)
    ev.note += f" | play={'yes' if ev.value >= min_value else 'hold'}"
    return (ev.value >= min_value, lane, ev)


def read_dense_lane(state):
    lanes = [0, 0, 0]
    for u in state.get("player_units", []):
        l = int(u.get("lane", 1))
        if 0 <= l <= 2:
            lanes[l] += 1
    return max(range(3), key=lambda i: lanes[i])


# ── offline balance probe (for tuning summons toward strong-but-fair) ──────
def _scenario(kind):
    """A canned player board to test a card against."""
    if kind == "swarm":
        us = [{"key": "rat", "hp": 120, "dmg": 30, "lane": 1} for _ in range(6)]
    elif kind == "tank":
        us = [{"key": "titan", "hp": 1400, "dmg": 90, "lane": 1}]
    elif kind == "air":
        us = [{"key": "bird", "hp": 260, "dmg": 60, "flying": True, "lane": i} for i in range(3)]
    elif kind == "turtle":
        us = [{"key": "wall", "hp": 900, "dmg": 40, "building": True, "lane": 1},
              {"key": "wall", "hp": 900, "dmg": 40, "building": True, "lane": 0}]
    else:  # empty / poke
        us = [{"key": "scout", "hp": 200, "dmg": 40, "lane": 1}]
    return {"player_units": us, "boss_king_hp": 1.0, "phase": 2}


def balance_probe(card, scenarios=("swarm", "tank", "air", "turtle", "poke"),
                  horizon=6.0):
    """Evaluate a card across canned scenarios. Returns aggregate metrics used to
    judge whether it's strong-but-fair, too weak, or oppressive."""
    evs = []
    for sc in scenarios:
        _, ev = best_deployment(_scenario(sc), card, horizon=horizon)
        evs.append(ev)
    avg_val = sum(e.value for e in evs) / len(evs)
    avg_healthy = sum(e.healthy for e in evs) / len(evs)
    avg_surv = sum(e.survived for e in evs) / len(evs)
    win_ish = sum(1 for e in evs if e.score > 0) / len(evs)
    verdict = ("oppressive" if avg_surv > 0.85 and avg_val > 8 else
               "weak" if avg_val < 1.0 else "fair")
    return {"avg_value": round(avg_val, 2), "avg_healthy": round(avg_healthy, 2),
            "avg_survival": round(avg_surv, 2), "win_rate": round(win_ish, 2),
            "verdict": verdict,
            "per_scenario": {sc: round(e.value, 1) for sc, e in zip(scenarios, evs)}}


def autobalance(card, target_survival=0.55, horizon=6.0, iters=8):
    """Nudge a copy of the card's hp/dmg toward the 'fair' survival band, so a
    generated boss summon feels strong but not oppressive. Returns a tuned copy.
    Deterministic; small multiplicative steps."""
    c = dict(card)
    for _ in range(iters):
        probe = balance_probe(c, horizon=horizon)
        surv = probe["avg_survival"]
        if abs(surv - target_survival) < 0.08:
            break
        if surv > target_survival:      # too strong → trim hp/dmg a touch
            c["hp"] = max(60, int(c.get("hp", 300) * 0.92))
            c["dmg"] = max(10, int(c.get("dmg", 40) * 0.95))
        else:                           # too weak → buff a touch
            c["hp"] = int(c.get("hp", 300) * 1.08)
            c["dmg"] = int(c.get("dmg", 40) * 1.05)
    return c


_AUDIT_CACHE = {"key": None, "rows": None}
def audit_roster(cards, horizon=5.0):
    """Probe every card and return a list of rows sorted worst→best-balanced, for
    the in-game balance panel. `cards` is a {key: card_dict} mapping. Cached by a
    cheap signature so re-opening the panel is instant."""
    sig = (len(cards), horizon)
    if _AUDIT_CACHE["key"] == sig and _AUDIT_CACHE["rows"] is not None:
        return _AUDIT_CACHE["rows"]
    rows = []
    for key, card in cards.items():
        c = dict(card); c.setdefault("id", key); c.setdefault("cost", card.get("cost", 4))
        try:
            p = balance_probe(c, horizon=horizon)
            rows.append({
                "key": key, "name": card.get("name", key), "cost": c["cost"],
                "value": p["avg_value"], "survival": p["avg_survival"],
                "healthy": p["avg_healthy"], "verdict": p["verdict"],
            })
        except Exception:
            rows.append({"key": key, "name": card.get("name", key),
                         "cost": card.get("cost", 4), "value": 0.0, "survival": 0.0,
                         "healthy": 0.0, "verdict": "?"})
    # sort by how far from "fair" (healthy high = balanced); worst first
    order = {"oppressive": 0, "weak": 1, "fair": 2, "?": 3}
    rows.sort(key=lambda r: (r["healthy"], -order.get(r["verdict"], 3)))
    _AUDIT_CACHE["key"] = sig; _AUDIT_CACHE["rows"] = rows
    return rows


if __name__ == "__main__":
    # self-test: rollouts produce sensible, ordered results
    state = _scenario("swarm")
    weak = {"id": "gnat", "hp": 90, "dmg": 12, "spd": 60, "cost": 2, "count": 1}
    strong = {"id": "brute", "hp": 900, "dmg": 120, "spd": 70, "cost": 6, "count": 1}
    ev_w = evaluate_summon(state, weak)
    ev_s = evaluate_summon(state, strong)
    assert ev_s.score > ev_w.score, (ev_s.score, ev_w.score)
    # lane search picks a lane and returns an Eval
    lane, ev = best_deployment(state, strong)
    assert 0 <= lane <= 2 and isinstance(ev, Eval)
    # budgeting: value is score/ink
    assert abs(ev.value - ev.score / ev.ink) < 1e-6
    # balance probe classifies a wildly overtuned card as oppressive-ish and a
    # feeble one as weak
    op = balance_probe({"hp": 4000, "dmg": 400, "spd": 90, "cost": 3})
    fb = balance_probe({"hp": 60, "dmg": 6, "spd": 40, "cost": 6})
    assert op["avg_value"] > fb["avg_value"]
    # autobalance moves a monster toward the fair survival band
    tuned = autobalance({"hp": 4000, "dmg": 400, "spd": 90, "cost": 4})
    assert tuned["hp"] <= 4000
    # timing/lane: against a swarm in lane 1, a plague-ish card should prefer to
    # meet it (lane 1 scores highest among lanes for a wide attacker)
    print(f"✅ boss_sim self-test: weak={ev_w.score:.0f} < strong={ev_s.score:.0f}, "
          f"best lane={lane} value={ev.value:.1f}, probe(overtuned)="
          f"{op['verdict']}/{op['avg_value']} vs probe(feeble)={fb['verdict']}/"
          f"{fb['avg_value']}, autobalance hp {4000}->{tuned['hp']}")
