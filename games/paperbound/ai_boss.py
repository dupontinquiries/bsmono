"""
PAPERBOUND — AI Bosses ("Arch-Scribes").

A layer on top of ai_director that gives bosses a *mind*: each Arch-Scribe reads
a live JSON read-out of the battle and chooses tactics that specifically answer
what the player is doing — punishing a swarm with area blots, countering a tank
rush by mirroring it, zoning a turtling player off their favourite lane — so the
player is pushed to keep changing their plan.

Two brains, same interface (mirrors ai_director):
  • heuristic  — a smart, offline counter-picker. Reads the player's board
                 composition and picks the move that best answers it. Always
                 available; fully testable; deterministic-ish.
  • llm        — the boss "thinks aloud" through an OpenAI/Ollama/Anthropic
                 endpoint. Its PERSONA is injected as a system prompt so the
                 encounter has a distinct voice and style, and it's handed the
                 battle JSON to reason over. Output is parsed + validated the
                 same way director events are, so it can never break the sim.

Every action a boss emits is one of a small, safe vocabulary the game knows how
to apply (see ACTIONS). Custom minions the boss "writes into being" are run
through ai_director.sanitize_card(), then tagged as boss-owned.

Public surface (used by game.py):
    persona = BOSS_PERSONAS["inkwright"]
    mind = BossMind(persona, cfg)          # cfg = ai_director.DirectorConfig
    mind.start(get_state)                  # background thread, like the Director
    for act in mind.drain(): apply(act)    # {"kind": "...", ...}
    mind.on_player_move(card_id)           # feed it the player's plays (optional)
"""
import os, json, time, queue, random, threading

try:
    import ai_director
    _AID = True
except Exception:
    _AID = False
    ai_director = None

try:
    import boss_sim                    # forward-model card evaluator (advisory)
    _SIM = True
except Exception:
    _SIM = False
    boss_sim = None


# ── the safe action vocabulary a boss may emit ────────────────────────────
# The game only ever applies these; anything else from a model is dropped.
ACTIONS = (
    "summon",      # deploy a (possibly custom) minion on the boss side
    "counter",     # a targeted answer to the player's biggest threat
    "zone",        # deny a lane / area with a hazard or ink blot
    "mirror",      # copy the player's strongest card back at them
    "buff",        # empower the boss's own forces briefly
    "taunt",       # pure flavour — an in-voice line, no mechanical effect
)
COUNTERS = ("freeze_biggest", "blot_densest_lane", "knockback_front",
            "summon_hardcounter", "swarm_flanks")


class BossPersona:
    """The stylistic identity of an Arch-Scribe: how it talks and how it fights.
    `voice` is injected into the LLM system prompt; `bias` nudges the heuristic
    toward this boss's signature tactics; `palette` tints the minions it writes."""
    __slots__ = ("id", "name", "title", "voice", "bias", "palette",
                 "signature", "counter_pref", "taunts")

    def __init__(self, id, name, title, voice, bias, palette,
                 signature, counter_pref, taunts):
        self.id = id
        self.name = name
        self.title = title
        self.voice = voice
        self.bias = bias                    # {action: weight}
        self.palette = palette              # [r,g,b] tint for its minions
        self.signature = signature          # preferred abilities for its minions
        self.counter_pref = counter_pref    # ordered COUNTERS it likes
        self.taunts = taunts                # fallback lines when offline


BOSS_PERSONAS = {
    "inkwright": BossPersona(
        "inkwright", "The Inkwright", "who unwrites what you make",
        voice=("You are The Inkwright, a cold, precise arch-scribe who believes "
               "every creature the player folds is a mistake to be corrected. You "
               "speak in short, cutting proclamations about ink, erasure and "
               "revision. You fight by blotting out the player's crowded lanes and "
               "erasing their strongest piece."),
        bias={"zone": 3, "counter": 3, "summon": 2, "taunt": 1, "buff": 1, "mirror": 1},
        palette=[40, 20, 70],
        signature=["inksplash", "plague", "toxic", "artillery"],
        counter_pref=["blot_densest_lane", "freeze_biggest", "summon_hardcounter"],
        taunts=["A blot corrects your crowded lane.", "Your strongest line — unwritten.",
                "I revise. You erase.", "The margin is mine to redraw."]),
    "mirrorbind": BossPersona(
        "mirrorbind", "The Mirrorbinder", "who returns your work against you",
        voice=("You are The Mirrorbinder, a mischievous arch-scribe who copies the "
               "player's own creations and turns them back on their maker. You are "
               "playful and mocking, delighting in symmetry and reflection. You "
               "prefer to mirror the player's best card and to hard-counter their "
               "composition rather than brute force."),
        bias={"mirror": 4, "counter": 3, "summon": 1, "taunt": 1, "buff": 1, "zone": 1},
        palette=[120, 120, 160],
        signature=["copycat", "mirror", "blink", "doublestrike"],
        counter_pref=["summon_hardcounter", "knockback_front", "freeze_biggest"],
        taunts=["A fine design — I'll keep it.", "Meet yourself, folded backward.",
                "Every stroke you make, I return.", "Symmetry always wins."]),
    "swarmmother": BossPersona(
        "swarmmother", "The Swarm-Mother", "who drowns the page in numbers",
        voice=("You are The Swarm-Mother, a seething arch-scribe who answers "
               "everything with overwhelming numbers of small folded things. You "
               "speak in a buzzing, plural, hungry voice. You flood flanks and "
               "punish slow, expensive pieces with tides of cheap swarms."),
        bias={"summon": 4, "zone": 2, "buff": 2, "counter": 1, "taunt": 1, "mirror": 1},
        palette=[90, 120, 60],
        signature=["relentless", "leap", "plague", "rush"],
        counter_pref=["swarm_flanks", "knockback_front", "blot_densest_lane"],
        taunts=["We are many. You are one.", "The page crawls with us.",
                "Count them if you can.", "More. Always more."]),
    "chronoscribe": BossPersona(
        "chronoscribe", "The Chrono-Scribe", "who edits the tempo of the page",
        voice=("You are The Chrono-Scribe, a stern arch-scribe who manipulates "
               "time and initiative. You freeze the player's advances, hasten your "
               "own, and speak in clock and calendar metaphors — beats, measures, "
               "revisions. You punish over-commitment by stalling it."),
        bias={"counter": 4, "buff": 3, "zone": 1, "summon": 1, "taunt": 1, "mirror": 1},
        palette=[70, 90, 150],
        signature=["freeze", "chill", "beacon", "bulwark"],
        counter_pref=["freeze_biggest", "knockback_front", "summon_hardcounter"],
        taunts=["Your tempo is mine to edit.", "Hold. The measure is not yours.",
                "A beat too slow.", "Time favours the patient scribe."]),
}


def list_personas():
    return list(BOSS_PERSONAS.keys())


# ── Boss VARIANTS: a rare twist on a fight ────────────────────────────────
# Most fights are the plain persona. Rarely, a boss shows up as a *variant* — a
# titled version that mostly changes HOW and WHERE it fights: its placement style
# and the troop entourage it favours (and opens with). This adds 1v1/boss variety
# without new personas. Each variant is a small, data-only modifier.
class BossVariant:
    __slots__ = ("id", "title", "placement", "entourage", "escort",
                 "palette_shift", "summon_bias", "blurb")

    def __init__(self, id, title, placement, entourage, escort=2,
                 palette_shift=(0, 0, 0), summon_bias=1.0, blurb=""):
        self.id = id
        self.title = title              # prefix shown before the boss name
        self.placement = placement      # "front"|"back"|"flanks"|"lane"|"spread"
        self.entourage = entourage      # preferred minion archetypes/abilities
        self.escort = escort            # how many minions it opens the fight with
        self.palette_shift = palette_shift   # rgb delta on the boss's minions
        self.summon_bias = summon_bias  # multiplier on how often it summons
        self.blurb = blurb              # one-line flavour on the announce card


BOSS_VARIANTS = {
    "relentless": BossVariant(
        "relentless", "Relentless", placement="front",
        entourage=["rush", "leap", "relentless", "charge"], escort=3,
        palette_shift=(40, -10, -20), summon_bias=1.4,
        blurb="deploys at the river's edge and never stops pushing"),
    "hollow": BossVariant(
        "hollow", "Hollow", placement="back",
        entourage=["bulwark", "shield", "regen", "siege"], escort=2,
        palette_shift=(-30, -30, -10), summon_bias=0.8,
        blurb="hangs back behind a wall of patient, folded husks"),
    "manysouled": BossVariant(
        "manysouled", "Many-Souled", placement="flanks",
        entourage=["swarm", "manyfold", "relentless", "rush"], escort=4,
        palette_shift=(10, 30, -20), summon_bias=1.6,
        blurb="pours escorts down both flanks at once"),
    "surgeon": BossVariant(
        "surgeon", "Surgeon", placement="lane",
        entourage=["artillery", "toxic", "plague", "sniper"], escort=2,
        palette_shift=(-20, 10, 40), summon_bias=1.0,
        blurb="commits everything to your busiest lane with clinical focus"),
    "wandering": BossVariant(
        "wandering", "Wandering", placement="spread",
        entourage=["builder", "tunnel", "carrier", "siege"], escort=2,
        palette_shift=(20, 20, 30), summon_bias=1.1,
        blurb="scatters odd, industrious folds across the whole page"),
}


def roll_boss_variant(rng=None, chance=0.16):
    """Rarely (default ~16%) return a BossVariant to twist this fight; else None.
    Deterministic when given a seeded rng."""
    import random as _r
    rng = rng or _r
    if rng.random() < chance:
        return rng.choice(list(BOSS_VARIANTS.values()))
    return None


def variant_lane_x(placement, dense_lane, board_lo, board_w, rng):
    """Given a placement style, return an x within [board_lo, board_lo+board_w]
    where the boss should drop its next minion. Pure function of the style."""
    lanes = (board_lo + board_w * 0.2, board_lo + board_w * 0.5, board_lo + board_w * 0.8)
    if placement == "lane":
        return lanes[dense_lane] + rng.uniform(-30, 30)
    if placement == "flanks":
        return rng.choice((lanes[0], lanes[2])) + rng.uniform(-30, 30)
    if placement == "spread":
        return board_lo + rng.uniform(40, board_w - 40)
    # front / back use the same x spread; the Y is handled by the caller
    return board_lo + board_w * 0.5 + rng.uniform(-160, 160)


# ── reading the player's board so tactics can *answer* it ─────────────────
def read_threat(state):
    """Summarise the player's composition into tags the brain can react to."""
    comp = state.get("player_units", [])   # list of {key,hp,dmg,flying,building,x,y,size}
    tags = set()
    n = len(comp)
    if n >= 5:
        tags.add("swarm")
    if any(u.get("flying") for u in comp):
        tags.add("air")
    if any(u.get("hp", 0) >= 900 for u in comp):
        tags.add("tank")
    if any(u.get("building") for u in comp):
        tags.add("turtle")
    # densest lane: split the board into 3 columns, find the busiest
    lanes = [0, 0, 0]
    for u in comp:
        lx = u.get("lane", 1)
        if 0 <= lx <= 2:
            lanes[lx] += 1
    dense_lane = max(range(3), key=lambda i: lanes[i]) if n else 1
    biggest = max(comp, key=lambda u: u.get("hp", 0), default=None)
    return {"tags": tags, "count": n, "dense_lane": dense_lane,
            "biggest": (biggest or {}).get("key")}


# ── the boss mind ─────────────────────────────────────────────────────────
class BossMind:
    def __init__(self, persona, cfg=None, seed=None, variant=None):
        self.persona = persona
        self.variant = variant              # optional BossVariant twist for this fight
        self.cfg = cfg or (ai_director.DirectorConfig() if _AID else None)
        self.llm = ai_director.LLMClient(self.cfg) if _AID else None
        self.rng = random.Random(seed)
        self._q = queue.Queue()
        self._stop = threading.Event()
        self._thread = None
        self.recent_player_cards = []       # last few things the player played
        self.phase = 1
        self.period = float(os.environ.get("PAPERBOUND_BOSS_PERIOD", "9"))
        if variant is not None:
            # a variant summons more/less often and drops on a placement style
            self.period = max(4.0, self.period / max(0.5, variant.summon_bias))

    # -- lifecycle (mirrors ai_director.Director) --------------------------
    def start(self, get_state):
        if self._thread and self._thread.is_alive():
            return
        self._stop.clear()
        self._thread = threading.Thread(target=self._loop, args=(get_state,), daemon=True)
        self._thread.start()

    def stop(self):
        self._stop.set()

    def drain(self):
        out = []
        while True:
            try:
                out.append(self._q.get_nowait())
            except queue.Empty:
                break
        return out

    def on_player_move(self, card_id):
        """Feed the boss the player's plays so it can react to their style."""
        if card_id:
            self.recent_player_cards.append(card_id)
            self.recent_player_cards = self.recent_player_cards[-8:]

    # -- loop --------------------------------------------------------------
    def _loop(self, get_state):
        time.sleep(1.5)
        while not self._stop.is_set():
            try:
                state = {}
                try:
                    state = get_state() or {}
                except Exception:
                    state = {}
                self.phase = int(state.get("phase", 1))
                act = self.decide(state)
                if act:
                    self._q.put(act)
            except Exception:
                pass
            # bosses act faster as their king weakens (phase up)
            wait = max(4.0, self.period - (self.phase - 1) * 2.0)
            self._stop.wait(wait)

    # -- decision ----------------------------------------------------------
    def decide(self, state):
        if self.llm and self.llm.available():
            act = self._decide_llm(state)
            if act:
                return act
        return self._decide_heuristic(state)

    def _weighted_action(self):
        bias = self.persona.bias
        acts = list(bias.keys())
        weights = [bias[a] for a in acts]
        return self.rng.choices(acts, weights=weights, k=1)[0]

    def _decide_heuristic(self, state):
        """Smart offline counter-picker. Reads the player's composition and, most
        of the time, answers it directly; otherwise leans on the persona's bias."""
        threat = read_threat(state)
        tags = threat["tags"]
        taken = tuple(state.get("card_ids", ()))
        # 65% of the time, answer the board directly (this is the "smart" part)
        if tags and self.rng.random() < 0.65:
            if "swarm" in tags:
                # crowds → area denial, or a plague-bringer of our own
                if self.rng.random() < 0.5:
                    return self._act_zone(threat, reason="swarm")
                return self._act_summon(taken, want="plague", reason="answer the swarm", state=state)
            if "tank" in tags:
                # a big tank → freeze it or summon a hard counter (swarm/toxic)
                if self.persona.id == "mirrorbind":
                    return self._act_mirror(state, reason="tank")
                if self.rng.random() < 0.5:
                    return {"kind": "counter", "counter": "freeze_biggest",
                            "text": self._line("Your colossus — halted mid-stroke."),
                            "persona": self.persona.id}
                return self._act_summon(taken, want="swarm", reason="dogpile the tank", state=state)
            if "air" in tags:
                return self._act_summon(taken, want="air", reason="contest the sky", state=state)
            if "turtle" in tags:
                # a turtling player → zone their favourite lane / artillery
                return self._act_summon(taken, want="artillery", reason="crack the shell", state=state)
        # otherwise fall back to the persona's signature style
        act = self._weighted_action()
        if act == "summon":
            return self._act_summon(taken, state=state)
        if act == "zone":
            return self._act_zone(threat)
        if act == "mirror":
            return self._act_mirror(state)
        if act == "counter":
            c = self.rng.choice(self.persona.counter_pref)
            return {"kind": "counter", "counter": c,
                    "text": self._line(), "persona": self.persona.id}
        if act == "buff":
            return {"kind": "buff", "value": self.rng.choice([1.3, 1.5]),
                    "text": self._line("The ink thickens in my favour."),
                    "persona": self.persona.id}
        return {"kind": "taunt", "text": self._line(), "persona": self.persona.id}

    # -- action builders ---------------------------------------------------
    def _minion(self, taken, want=None):
        """Write a boss-flavoured minion into being (validated card dict)."""
        rng = self.rng
        ability = None
        if want == "swarm":
            raw = {"count": rng.randint(3, 5), "hp": rng.randint(120, 220),
                   "dmg": rng.randint(34, 60), "spd": rng.randint(70, 95),
                   "size": 16, "ability": "relentless"}
        elif want == "air":
            raw = {"flying": True, "hp": rng.randint(220, 380), "dmg": rng.randint(50, 84),
                   "spd": rng.randint(72, 96), "size": 20, "ability": "feather"}
        elif want == "plague":
            raw = {"hp": rng.randint(320, 520), "dmg": rng.randint(40, 70),
                   "spd": rng.randint(50, 70), "size": 22, "ability": "plague"}
        elif want == "artillery":
            raw = {"building": True, "hp": rng.randint(700, 1100), "dmg": rng.randint(70, 120),
                   "rng": rng.randint(150, 210), "spd": 0, "size": 28, "ability": "artillery"}
        else:
            # a variant biases the boss toward its entourage's abilities
            if self.variant is not None and rng.random() < 0.6:
                ability = rng.choice(self.variant.entourage)
            else:
                ability = rng.choice(self.persona.signature)
            raw = {"ability": ability}
        raw.setdefault("ability", rng.choice(self.persona.signature))
        pal = list(self.persona.palette)
        if self.variant is not None:                 # tint the entourage
            ps = self.variant.palette_shift
            pal = [max(0, min(255, pal[i] + ps[i])) for i in range(3)]
        raw["paper"] = pal
        raw["set"] = f"boss:{self.persona.id}"
        raw["name"] = f"{self.persona.name.split()[-1]}'s {rng.choice(ai_director._NOUN)}"
        # give the summon a fitting origami fold so boss creatures look distinct
        if _AID:
            arche = {"swarm": "swarm", "air": "flyer", "plague": "skirmish",
                     "artillery": "tower"}.get(want, "skirmish")
            try:
                raw["model"] = ai_director.procedural_model(arche, rng)
            except Exception:
                pass
            return ai_director.sanitize_card(raw, taken_ids=taken, rng=rng)
        return raw

    def _act_summon(self, taken, want=None, reason=None, state=None):
        card = self._minion(taken, want)
        lane = None
        ev = None
        # ── use the forward-sim tool to make the summon strong-but-fair, pick the
        #    best lane, and record its evaluated value (for budgeting). Advisory —
        #    if anything fails, we just summon as before. ──
        if _SIM and state is not None and isinstance(card, dict):
            try:
                # tune raw stats toward the fair survival band, then re-fold/validate
                tuned = boss_sim.autobalance(card)
                if _AID:
                    tuned = ai_director.sanitize_card(tuned, taken_ids=taken, rng=self.rng)
                    if isinstance(tuned.get("model"), (list, tuple)) or "model" in card:
                        tuned.setdefault("model", card.get("model"))
                card = tuned
                lane, ev = boss_sim.best_deployment(state, card)
            except Exception:
                lane, ev = None, None
        out = {"kind": "summon", "card": card, "persona": self.persona.id,
               "text": self._line(f"I fold {card['name']} to {reason or 'answer you'}."
                                   if reason else None)}
        if lane is not None:
            out["lane"] = lane
        # a variant overrides WHERE the boss drops (placement style) so its fights
        # feel spatially different — front line, backline, flanks, etc.
        if self.variant is not None:
            out["placement"] = self.variant.placement
        if ev is not None:
            out["eval"] = {"score": round(ev.score, 1), "value": round(ev.value, 2),
                           "healthy": round(ev.healthy, 2), "survival": round(ev.survived, 2)}
        return out

    def _act_zone(self, threat, reason=None):
        return {"kind": "zone", "lane": threat.get("dense_lane", 1),
                "persona": self.persona.id,
                "text": self._line("A blot spreads across your busiest lane."
                                    if reason == "swarm" else None)}

    def _act_mirror(self, state, reason=None):
        # copy the player's strongest card back at them
        target = None
        threat = read_threat(state)
        target = threat.get("biggest") or (self.recent_player_cards[-1]
                                           if self.recent_player_cards else None)
        return {"kind": "mirror", "target": target, "persona": self.persona.id,
                "text": self._line("Meet your own work, folded backward.")}

    def _line(self, override=None):
        if override:
            return override
        return self.rng.choice(self.persona.taunts)

    # -- LLM brain ---------------------------------------------------------
    def _decide_llm(self, state):
        threat = read_threat(state)
        sys = (
            self.persona.voice + "\n\n"
            "You are fighting a live duel. Given a JSON snapshot of the battle, "
            "choose ONE action that best answers what the player is doing and stays "
            "in your voice. Respond with ONLY a JSON object, one of:\n"
            '{"kind":"summon","card":{full card json, may include a "model" '
            'origami fold recipe},"text":"in-voice line"}\n'
            '{"kind":"counter","counter":"freeze_biggest|blot_densest_lane|'
            'knockback_front|summon_hardcounter|swarm_flanks","text":"..."}\n'
            '{"kind":"zone","lane":0|1|2,"text":"..."}\n'
            '{"kind":"mirror","target":"<player card id>","text":"..."}\n'
            '{"kind":"buff","value":1.3,"text":"..."}\n'
            '{"kind":"taunt","text":"..."}\n'
            "Be tactically smart: punish swarms with zone/area, answer tanks with "
            "freeze or swarms, contest air with air. Keep it fair. Text is short."
        )
        payload = dict(state)
        payload["threat"] = {"tags": sorted(threat["tags"]),
                             "dense_lane": threat["dense_lane"],
                             "biggest": threat["biggest"]}
        payload["recent_player_cards"] = self.recent_player_cards[-6:]
        txt = self.llm.complete(sys, json.dumps(payload)[:2200], max_tokens=700)
        obj = ai_director._extract_json(txt)
        return self._sanitize_action(obj, state)

    def _sanitize_action(self, obj, state):
        """Force any model output into the safe ACTIONS vocabulary, or None."""
        if not isinstance(obj, dict):
            return None
        kind = obj.get("kind")
        text = str(obj.get("text") or self._line())[:90]
        taken = tuple(state.get("card_ids", ()))
        if kind == "summon":
            card = ai_director.sanitize_card(obj.get("card"), taken_ids=taken, rng=self.rng) if _AID else None
            if not card:
                card = self._minion(taken)
            card["paper"] = list(self.persona.palette)   # keep the boss's look
            out = {"kind": "summon", "card": card, "text": text, "persona": self.persona.id}
            # run the LLM's summon through the same forward-sim: honour its lane if
            # given, else pick the best; attach the evaluated value for budgeting.
            if _SIM:
                try:
                    llm_lane = obj.get("lane")
                    if isinstance(llm_lane, int) and 0 <= llm_lane <= 2:
                        ev = boss_sim.evaluate_summon(state, card, lane=llm_lane)
                        out["lane"] = llm_lane
                    else:
                        llm_lane, ev = boss_sim.best_deployment(state, card)
                        out["lane"] = llm_lane
                    out["eval"] = {"score": round(ev.score, 1), "value": round(ev.value, 2),
                                   "healthy": round(ev.healthy, 2)}
                except Exception:
                    pass
            return out
        if kind == "counter":
            c = obj.get("counter")
            if c not in COUNTERS:
                c = self.rng.choice(self.persona.counter_pref)
            return {"kind": "counter", "counter": c, "text": text, "persona": self.persona.id}
        if kind == "zone":
            lane = obj.get("lane", 1)
            lane = lane if lane in (0, 1, 2) else 1
            return {"kind": "zone", "lane": lane, "text": text, "persona": self.persona.id}
        if kind == "mirror":
            return {"kind": "mirror", "target": obj.get("target"), "text": text,
                    "persona": self.persona.id}
        if kind == "buff":
            try:
                v = max(1.1, min(1.6, float(obj.get("value", 1.3))))
            except Exception:
                v = 1.3
            return {"kind": "buff", "value": v, "text": text, "persona": self.persona.id}
        if kind == "taunt":
            return {"kind": "taunt", "text": text, "persona": self.persona.id}
        return None


if __name__ == "__main__":
    # offline self-test — heuristic must produce only safe actions and actually
    # *react* to the player's composition.
    rng = random.Random(3)
    personas = list(BOSS_PERSONAS.values())
    n = 0
    kinds = set()
    for p in personas:
        mind = BossMind(p, seed=7)
        # a swarm-heavy board should draw zone/summon-plague answers often
        swarm_state = {"phase": 2, "card_ids": ["sabertooth"],
                       "player_units": [{"key": "frog", "hp": 180, "lane": 0} for _ in range(7)]}
        answers = [mind._decide_heuristic(swarm_state)["kind"] for _ in range(40)]
        assert all(k in ACTIONS for k in answers)
        # a tank board should draw freeze/summon/mirror
        tank_state = {"phase": 3, "card_ids": ["sabertooth"],
                      "player_units": [{"key": "polarbear", "hp": 1650, "lane": 1}]}
        for _ in range(40):
            a = mind._decide_heuristic(tank_state)
            assert a["kind"] in ACTIONS
            kinds.add(a["kind"])
        # minions are valid cards
        for _ in range(30):
            m = mind._minion(("sabertooth",), want=rng.choice(["swarm", "air", "plague", "artillery", None]))
            assert m and "id" in m and 1 <= m["cost"] <= 9
            n += 1
        # sanitize_action survives garbage
        for bad in [None, {}, {"kind": "NUKE"}, {"kind": "summon", "card": {"hp": 1e9}},
                    {"kind": "zone", "lane": 99}, {"kind": "buff", "value": "x"}]:
            a = mind._sanitize_action(bad, tank_state)
            if a: assert a["kind"] in ACTIONS
    print(f"✅ ai_boss self-test: {len(personas)} personas, {n} minions valid, "
          f"heuristic kinds={sorted(kinds)}, all actions safe")
