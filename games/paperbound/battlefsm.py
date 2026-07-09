"""
Paperbound battle finite-state machine.

Formalizes the shared lifecycle every game mode runs through, so mode-specific
behavior hooks into predictable phases instead of ad-hoc flags scattered across
the update loop. The FSM is deliberately tiny and pure (no pygame), so it's
unit-testable and deterministic — critical for lockstep LAN.

Phases:
    INTRO   - brief "chapter opens" beat before play begins
    PLAYING - normal open-ended play (no time limit)
    FRENZY  - triggered when either king drops below FRENZY_HP; the arena "goes
              crazy" with per-map chaos. Reversible if the king is healed above
              the exit threshold (hysteresis avoids flicker).
    ENDED   - a king fell or someone forfeit; the result banner shows.

Transitions are driven by feed(kings_hp_frac, forfeited) each tick. The owning
Game reads .phase and .frenzy to modulate the sim + render.
"""

INTRO, PLAYING, FRENZY, ENDED = "intro", "playing", "frenzy", "ended"

FRENZY_ENTER = 0.40     # a king at/under 40% HP tips the arena into frenzy
FRENZY_EXIT = 0.55      # must recover past 55% to calm down (hysteresis)
INTRO_TICKS = 24        # ~1.2s at 20 TPS


class BattleFSM:
    def __init__(self, intro_ticks=INTRO_TICKS):
        self.phase = INTRO
        self.intro_left = intro_ticks
        self.frenzy = False
        self.result = None            # None | "win" | "lose" (from team-0's view)
        self.frenzy_since = 0         # ticks spent in frenzy (for escalation)
        self._ticks = 0

    def feed(self, p_frac, e_frac, forfeited=False, dead=None):
        """Advance one tick.
          p_frac / e_frac : player/enemy king HP fractions (0..1)
          forfeited       : True if the local player forfeit
          dead            : "player" | "enemy" | None — a king was destroyed
        Returns the current phase.
        """
        self._ticks += 1
        if self.phase == ENDED:
            return self.phase

        # terminal conditions first — they win over everything
        if forfeited:
            self.result = "lose"; self.phase = ENDED; return self.phase
        if dead == "enemy":
            self.result = "win"; self.phase = ENDED; return self.phase
        if dead == "player":
            self.result = "lose"; self.phase = ENDED; return self.phase

        if self.phase == INTRO:
            self.intro_left -= 1
            if self.intro_left <= 0:
                self.phase = PLAYING
            return self.phase

        # PLAYING <-> FRENZY with hysteresis on the lowest king
        low = min(p_frac, e_frac)
        if not self.frenzy and low <= FRENZY_ENTER:
            self.frenzy = True; self.phase = FRENZY; self.frenzy_since = 0
        elif self.frenzy and low >= FRENZY_EXIT:
            self.frenzy = False; self.phase = PLAYING
        if self.frenzy:
            self.frenzy_since += 1
        return self.phase

    def forfeit(self):
        self.result = "lose"; self.phase = ENDED

    @property
    def playing(self):
        return self.phase in (PLAYING, FRENZY)


if __name__ == "__main__":
    fsm = BattleFSM(intro_ticks=3)
    # intro: first two feeds stay in intro, third transitions to playing
    assert fsm.feed(1.0, 1.0) == INTRO
    assert fsm.feed(1.0, 1.0) == INTRO
    assert fsm.feed(1.0, 1.0) == PLAYING
    # drop enemy king to trigger frenzy
    assert fsm.feed(1.0, 0.35) == FRENZY and fsm.frenzy
    # heal a bit but not enough -> still frenzy
    assert fsm.feed(1.0, 0.50) == FRENZY
    # heal past exit -> calm
    assert fsm.feed(1.0, 0.60) == PLAYING and not fsm.frenzy
    # kill enemy king -> win
    assert fsm.feed(1.0, 0.0, dead="enemy") == ENDED and fsm.result == "win"
    # forfeit path
    f2 = BattleFSM(intro_ticks=0); f2.feed(1,1)
    assert f2.feed(1,1, forfeited=True) == ENDED and f2.result == "lose"
    print("✅ battlefsm self-test passed")
