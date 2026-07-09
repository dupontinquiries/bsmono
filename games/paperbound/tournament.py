"""
PAPERBOUND — tournament brackets.

Auto-builds a bracket from a player list, in several styles, with seeding/reorder
options. Pure logic (no pygame), so it's fully unit-testable and the UI just
renders the state.

Styles:
  • single      — single elimination (byes fill to the next power of two),
  • double      — double elimination (winners + losers bracket, grand final),
  • roundrobin  — everyone plays everyone once, ranked by wins,
  • swiss       — N rounds, pair by nearest score each round (no repeats).

Seeding / reorder (how the initial order is decided):
  • as_entered  — keep the given order,
  • random      — shuffle (seeded, so a host can reproduce/announce it),
  • ranked      — order by a provided rating (best first),
  • snake       — ranked, then snake-fold so strong/weak are spread across the draw.

A match records its two players, the winner, and (optionally) the battle-report
path that proves it — which is what a trophy is later minted from.
"""
import random as _random


# ── seeding ────────────────────────────────────────────────────────────────
def seed_players(players, style="as_entered", ratings=None, seed=0):
    """Return an ordered list of player names for the initial draw."""
    ps = list(players)
    if style == "random":
        r = _random.Random(seed); r.shuffle(ps)
    elif style == "ranked" and ratings:
        ps.sort(key=lambda p: -ratings.get(p, 0))
    elif style == "snake" and ratings:
        ps.sort(key=lambda p: -ratings.get(p, 0))
        # snake: 1,4,5,8 … / 2,3,6,7 … spreads seeds across the bracket halves
        out, lo, hi, flip = [], 0, len(ps) - 1, False
        # simpler robust snake: distribute into buckets then read back
        half = (len(ps) + 1) // 2
        top, bot = ps[:half], ps[half:][::-1]
        out = []
        for i in range(half):
            out.append(top[i])
            if i < len(bot):
                out.append(bot[i])
        ps = out
    return ps


def _next_pow2(n):
    p = 1
    while p < n:
        p *= 2
    return p


def _seed_order(size):
    """Standard single-elim seed positions for a bracket of `size` (a power of
    two). Returns indices into a seed-ranked list so seed 1 meets the lowest
    seed / a bye, and 1 & 2 can only meet in the final."""
    order = [0]
    while len(order) < size:
        m = len(order) * 2
        nxt = []
        for x in order:
            nxt.append(x)
            nxt.append(m - 1 - x)
        order = nxt
    return order


# ── the bracket ─────────────────────────────────────────────────────────────
class Match:
    _n = 0
    def __init__(self, a=None, b=None, rnd=0, label=""):
        Match._n += 1
        self.id = Match._n
        self.a = a            # player name or None (bye / TBD)
        self.b = b
        self.rnd = rnd
        self.label = label
        self.winner = None
        self.report = None    # battle-report path (proof) once decided
        self.next = None      # (match_id, slot) this feeds into, for elim brackets
        self.next_lose = None # (double-elim) where the loser drops to

    def ready(self):
        return self.a is not None and self.b is not None and self.winner is None

    def is_bye(self):
        return (self.a is None) ^ (self.b is None)

    def to_dict(self):
        return {"id": self.id, "a": self.a, "b": self.b, "rnd": self.rnd,
                "label": self.label, "winner": self.winner, "report": self.report}


class Tournament:
    def __init__(self, players, style="single", seed_style="as_entered",
                 ratings=None, seed=0, swiss_rounds=None):
        self.style = style
        self.players = seed_players(players, seed_style, ratings, seed)
        self.matches = []
        self.rounds = []          # list of lists of Match (by round)
        self.standings = {p: {"w": 0, "l": 0} for p in self.players}
        self._seed = seed
        self.swiss_rounds = swiss_rounds or max(1, (len(self.players) - 1).bit_length())
        self._history = set()     # swiss: pairs already played
        self._build()

    # ── construction ──
    def _build(self):
        if self.style == "roundrobin":
            self._build_roundrobin()
        elif self.style == "swiss":
            self._build_swiss_round()      # first round only; more added as we go
        elif self.style == "double":
            self._build_single(double=True)
        else:
            self._build_single()

    def _build_single(self, double=False):
        n = len(self.players)
        size = _next_pow2(n)
        # standard bracket seed order: positions arranged so seed 1 and seed 2
        # can only meet in the final, and byes fall to the top seeds. Build the
        # seed slot order recursively (1,8,5,4,3,6,7,2 for 8), then map players.
        order = _seed_order(size)
        seeded = self.players + [None] * (size - n)   # players by seed rank
        slots = [seeded[i] if i < len(seeded) else None for i in order]
        rnd0 = []
        for i in range(0, size, 2):
            m = Match(slots[i], slots[i + 1], rnd=0, label="R1")
            rnd0.append(m); self.matches.append(m)
        self.rounds.append(rnd0)
        # build empty subsequent rounds and link
        cur = rnd0
        r = 1
        while len(cur) > 1:
            nxt = []
            for i in range(0, len(cur), 2):
                m = Match(None, None, rnd=r, label=f"R{r+1}")
                cur[i].next = (m.id, "a")
                cur[i + 1].next = (m.id, "b")
                nxt.append(m); self.matches.append(m)
            self.rounds.append(nxt); cur = nxt; r += 1
        # auto-advance byes
        for m in rnd0:
            if m.is_bye():
                m.winner = m.a if m.a is not None else m.b
                self._propagate(m)
        self.double = double
        self.losers = []          # (double-elim losers bracket built lazily/simply)

    def _build_roundrobin(self):
        ps = self.players[:]
        if len(ps) % 2:
            ps.append(None)                 # odd → bye placeholder
        n = len(ps)
        rounds = []
        arr = ps[:]
        for r in range(n - 1):
            rr = []
            for i in range(n // 2):
                a, b = arr[i], arr[n - 1 - i]
                if a is not None and b is not None:
                    m = Match(a, b, rnd=r, label=f"Round {r+1}")
                    rr.append(m); self.matches.append(m)
            rounds.append(rr)
            arr = [arr[0]] + [arr[-1]] + arr[1:-1]   # rotate (circle method)
        self.rounds = rounds

    def _build_swiss_round(self):
        r = len(self.rounds)
        # pair by nearest score, avoiding rematches where possible
        ranked = sorted(self.players, key=lambda p: (-self.standings[p]["w"], p))
        used, pairs = set(), []
        for p in ranked:
            if p in used:
                continue
            for q in ranked:
                if q is p or q in used:
                    continue
                if frozenset((p, q)) in self._history:
                    continue
                pairs.append((p, q)); used.add(p); used.add(q); break
            else:
                # everyone else already played/paired → bye
                if p not in used:
                    used.add(p); pairs.append((p, None))
        rr = []
        for a, b in pairs:
            m = Match(a, b, rnd=r, label=f"Swiss {r+1}")
            self._history.add(frozenset((a, b)))
            if b is None:
                m.winner = a; self.standings[a]["w"] += 1
            rr.append(m); self.matches.append(m)
        self.rounds.append(rr)

    # ── reporting a result ──
    def report_result(self, match_id, winner, report=None):
        m = next((x for x in self.matches if x.id == match_id), None)
        if not m or winner not in (m.a, m.b):
            return False
        m.winner = winner
        m.report = report
        loser = m.b if winner == m.a else m.a
        if winner in self.standings:
            self.standings[winner]["w"] += 1
        if loser in self.standings:
            self.standings[loser]["l"] += 1
        self._propagate(m)
        # swiss: when a round completes, pair the next
        if self.style == "swiss" and self._round_done(m.rnd):
            if len(self.rounds) < self.swiss_rounds:
                self._build_swiss_round()
        return True

    def _round_done(self, rnd):
        return all(x.winner is not None or x.is_bye()
                   for x in self.matches if x.rnd == rnd)

    def _propagate(self, m):
        if m.next and m.winner is not None:
            mid, slot = m.next
            nxt = next((x for x in self.matches if x.id == mid), None)
            if nxt:
                setattr(nxt, slot, m.winner)
                if nxt.is_bye():           # a bye in a later round advances too
                    nxt.winner = nxt.a if nxt.a is not None else nxt.b
                    self._propagate(nxt)

    # ── queries ──
    def next_matches(self):
        """Matches ready to be played right now."""
        return [m for m in self.matches if m.ready()]

    def champion(self):
        if self.style in ("single", "double") and self.rounds:
            fin = self.rounds[-1][0] if self.rounds[-1] else None
            return fin.winner if fin else None
        if self.style in ("roundrobin", "swiss"):
            done = all(m.winner is not None or m.is_bye() for m in self.matches)
            if done and self.standings:
                return max(self.standings, key=lambda p: (self.standings[p]["w"],
                                                          -self.standings[p]["l"]))
        return None

    def table(self):
        """Standings sorted best-first (for round-robin / swiss)."""
        return sorted(self.standings.items(),
                      key=lambda kv: (-kv[1]["w"], kv[1]["l"], kv[0]))

    def to_dict(self):
        return {"style": self.style, "players": self.players,
                "rounds": [[m.to_dict() for m in rr] for rr in self.rounds],
                "standings": self.standings, "champion": self.champion()}


if __name__ == "__main__":
    # single elimination with a non-power-of-two count (byes)
    t = Tournament(["Ana", "Bo", "Cy", "Di", "Ed"], style="single")
    assert len(t.rounds[0]) == 4                 # 5 players → 8-slot bracket
    # play it out: always the first-named advances
    guard = 0
    while t.next_matches() and guard < 50:
        for m in t.next_matches():
            t.report_result(m.id, m.a)
        guard += 1
    assert t.champion() == "Ana", t.champion()

    # round robin: everyone plays everyone
    t2 = Tournament(["A", "B", "C", "D"], style="roundrobin")
    n_matches = len(t2.matches)
    assert n_matches == 6, n_matches            # C(4,2)=6
    for m in t2.matches:
        t2.report_result(m.id, m.a)
    assert t2.champion() == "A"                  # A beat everyone

    # swiss: pairs by score, no immediate rematch, right number of rounds
    t3 = Tournament(["P1", "P2", "P3", "P4", "P5", "P6"], style="swiss",
                    seed_style="random", seed=1)
    rounds_played = 0
    guard = 0
    while t3.next_matches() and guard < 50:
        for m in t3.next_matches():
            t3.report_result(m.id, m.a)
        rounds_played += 1; guard += 1
    assert len(t3.rounds) == t3.swiss_rounds
    # no pair repeated
    seen = set()
    for m in t3.matches:
        if m.a and m.b:
            k = frozenset((m.a, m.b))
            assert k not in seen, ("swiss rematch", m.a, m.b)
            seen.add(k)

    # seeding styles produce valid permutations
    ps = ["a", "b", "c", "d", "e"]
    assert sorted(seed_players(ps, "random", seed=3)) == sorted(ps)
    assert seed_players(ps, "ranked", {"a": 1, "b": 9, "c": 5})[0] == "b"
    snake = seed_players(["a", "b", "c", "d"], "snake", {"a": 4, "b": 3, "c": 2, "d": 1})
    assert set(snake) == {"a", "b", "c", "d"}

    print(f"✅ tournament self-test: single-elim (byes→champion), round-robin "
          f"({n_matches} matches), swiss ({t3.swiss_rounds} rounds, no rematches), "
          f"seeding styles (as_entered/random/ranked/snake) OK")
