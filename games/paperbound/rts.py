"""
Paperbound — Foldworks RTS (a game inside the game).

A StarCraft-flavoured real-time-strategy mode that reuses Paperbound's card
roster as its unit types and its origami look. It is a SELF-CONTAINED, single-
player skirmish — it deliberately does NOT touch the lane-battler's deterministic
netcode; it's the "other side" of the game you swap into from the menu, an
Inscryption-style game-within-the-game.

What's here:
  • Resource economy   — Scribes harvest INK from ink-wells back to your Scriptorium.
  • Production          — the Scriptorium trains Scribes; a Folding Table trains
                          combat units drawn from the card roster.
  • Build              — select a Scribe, open the build palette, place a structure.
  • Selection & command— click / drag-box select, right-click move & attack-move,
                          control groups 1–9, a command palette with hotkeys.
  • Minimap            — a corner overview of the whole map with fog-free scouting.
  • A simple AI        — the enemy expands, trains an army, and attacks.

This module owns its own little world (grid coords in "world units"); the host
game supplies the screen surface, fonts, and the origami unit renderer via a thin
context passed to draw(). Everything is plain floats and lists — no determinism
guarantees are needed because it never runs over the wire.

Entry points used by game.py:
    rts = RTS(ctx)          # ctx gives screen size, fonts, draw helpers, cards
    rts.handle_event(ev)    # feed pygame events
    rts.update(dt)          # advance the sim
    rts.draw(screen)        # render the world + HUD + minimap
    rts.done                # True when the match is over
"""
import math
import random
import copy
import os
import json
import pygame
try:
    import rts_tournament as tourney
except Exception:
    tourney = None


# ── world constants ──────────────────────────────────────────────────────────
# A large map that you scroll and zoom across — no longer a single screenful.
WORLD_W, WORLD_H = 4200, 2800          # world units (the full map)
INK_PER_TRIP = 8
HARVEST_TIME = 2.2                     # seconds a scribe spends mining a node
ZOOM_MIN, ZOOM_MAX = 0.45, 1.6         # how far out / in the camera can go
ISO = 0.62                             # vertical squash: the page is seen at an
                                       # angle, like the main game's board, not
                                       # flat from directly above
HG_LIFT = 26.0                         # world units of visual lift for high ground
LONGPRESS_T = 0.4                      # seconds a touch/hold takes to open the wheel

# structure + unit catalogues (kept tiny & self-describing). `card` maps a combat
# unit to a roster key so we can reuse its origami model & stats flavour.
STRUCTURES = {
    "scriptorium": dict(name="Scriptorium", cost=0, hp=1400, r=54, build=6.0,
                        makes=["scribe"], key="S", desc="Town hall. Trains Scribes; drop-off for ink."),
    "table":       dict(name="Folding Table", cost=120, hp=800, r=42, build=8.0,
                        makes=["soldier", "archer", "brute"], key="T",
                        desc="Trains folded soldiers."),
    "well":        dict(name="Ink Reservoir", cost=90, hp=500, r=34, build=6.0,
                        makes=[], key="R", desc="Raises your ink cap and income."),
    "spire":       dict(name="Watch Spire", cost=150, hp=900, r=38, build=9.0,
                        makes=[], key="P", desc="A defensive tower that fires on foes."),
}

UNITS = {
    "scribe":  dict(name="Scribe", cost=25, hp=60, dmg=4, spd=95, rng=14, r=11,
                    card="beekeeper", worker=True, build_t=3.0, key="E",
                    desc="Harvests ink; can raise structures."),
    "soldier": dict(name="Folded Soldier", cost=50, hp=140, dmg=14, spd=88, rng=16, r=13,
                    card="gorilla", build_t=4.0, key="A", desc="Sturdy melee line."),
    "archer":  dict(name="Quill Archer", cost=65, hp=90, dmg=16, spd=92, rng=150, r=12,
                    card="crane", build_t=5.0, key="D", desc="Ranged; fragile."),
    "brute":   dict(name="Paper Brute", cost=110, hp=340, dmg=34, spd=70, rng=18, r=18,
                    card="mammoth", build_t=8.0, key="F", desc="Slow, heavy shock unit."),
}

TEAM_COL = {0: (90, 150, 220), 1: (216, 100, 92)}
TEAM_DARK = {0: (54, 92, 150), 1: (150, 62, 58)}


# ── FACTIONS ─────────────────────────────────────────────────────────────────
# Three playable binderies, each with its own structure/unit catalogue, a tech
# tree (structures gate units & upgrades via `requires`/`tier`), and a trait.
# Structure fields: name, cost, hp, r, build, makes[], requires[], researches[],
#   tier, key, desc.  Unit fields: name, cost, hp, dmg, spd, rng, r, card,
#   worker?, build_t, tier, key, desc.  Upgrade fields: name, cost, time,
#   stat, mult, applies (unit kinds or "all"/"army"/"workers"), desc.
FACTIONS = {
    "court": dict(
        name="The Folded Court", blurb="Versatile and balanced — no glaring weakness.",
        color=(90, 150, 220), dark=(54, 92, 150),
        trait="Balanced: standard economy, a full roster at every tier.",
        econ=1.0,
        structures={
            "scriptorium": dict(name="Scriptorium", cost=0, hp=1400, r=54, build=6.0,
                                makes=["scribe"], requires=[], researches=[], tier=1, key="S",
                                desc="Town hall. Trains Scribes; ink drop-off."),
            "table": dict(name="Folding Table", cost=120, hp=800, r=42, build=8.0,
                          makes=["soldier", "archer"], requires=[], researches=["sharp"], tier=1, key="T",
                          desc="Trains soldiers & archers."),
            "well": dict(name="Ink Reservoir", cost=90, hp=500, r=34, build=6.0,
                         makes=[], requires=[], researches=[], tier=1, key="R",
                         desc="Raises ink cap & income."),
            "spire": dict(name="Watch Spire", cost=150, hp=900, r=38, build=9.0,
                          makes=[], requires=[], researches=[], tier=1, key="P",
                          desc="Defensive tower."),
            "bindery": dict(name="Grand Bindery", cost=200, hp=1000, r=46, build=12.0,
                            makes=["brute"], requires=["table"], researches=["thick", "swift"], tier=2, key="B",
                            desc="Tier 2. Unlocks Brutes & fold upgrades."),
            "menagerie": dict(name="Paper Menagerie", cost=320, hp=1300, r=52, build=16.0,
                              makes=["colossus"], requires=["bindery"], researches=["colossal"], tier=3, key="M",
                              desc="Tier 3. Folds the Origami Colossus."),
        },
        units={
            "scribe": dict(name="Scribe", cost=25, hp=60, dmg=4, spd=95, rng=14, r=11,
                           card="beekeeper", worker=True, build_t=3.0, tier=1, key="E",
                           desc="Harvests ink; raises structures."),
            "soldier": dict(name="Folded Soldier", cost=50, hp=150, dmg=14, spd=88, rng=16, r=13,
                            card="gorilla", build_t=4.0, tier=1, key="A", desc="Sturdy melee line."),
            "archer": dict(name="Quill Archer", cost=65, hp=90, dmg=16, spd=92, rng=150, r=12,
                           card="crane", build_t=5.0, tier=1, key="D", desc="Ranged; fragile."),
            "brute": dict(name="Paper Brute", cost=110, hp=360, dmg=34, spd=70, rng=18, r=18,
                          card="mammoth", build_t=8.0, tier=2, key="F", desc="Heavy shock unit."),
            "colossus": dict(name="Origami Colossus", cost=240, hp=900, dmg=70, spd=58, rng=22, r=26,
                             card="foldwright", build_t=14.0, tier=3, key="C", desc="Towering elite."),
        },
        upgrades={
            "sharp": dict(name="Sharper Folds", cost=100, time=18, stat="dmg", mult=1.25,
                          applies="army", desc="+25% army attack."),
            "thick": dict(name="Thicker Paper", cost=120, time=22, stat="maxhp", mult=1.3,
                          applies="army", desc="+30% army health."),
            "swift": dict(name="Swift Creases", cost=90, time=16, stat="spd", mult=1.2,
                          applies="army", desc="+20% army speed."),
            "colossal": dict(name="Colossal Folds", cost=160, time=26, stat="dmg", mult=1.3,
                             applies=["colossus", "brute"], desc="+30% brute & colossus attack."),
        },
    ),
    "ember": dict(
        name="The Ember Bindery", blurb="Aggressive glass cannon — cheap, fast, fragile, burns.",
        color=(224, 132, 70), dark=(150, 78, 40),
        trait="Aggro: cheaper & faster units, weaker economy. Strike early.",
        econ=0.85,
        structures={
            "scriptorium": dict(name="Kiln-Hall", cost=0, hp=1200, r=52, build=6.0,
                                makes=["scribe"], requires=[], researches=[], tier=1, key="S",
                                desc="Town hall. Trains Ashscribes; ink drop-off."),
            "table": dict(name="Forge", cost=100, hp=680, r=40, build=7.0,
                          makes=["soldier", "archer"], requires=[], researches=["hot"], tier=1, key="T",
                          desc="Trains ember troops."),
            "well": dict(name="Ember Well", cost=90, hp=460, r=34, build=6.0,
                         makes=[], requires=[], researches=[], tier=1, key="R",
                         desc="Raises ink cap & income."),
            "spire": dict(name="Flame Spire", cost=140, hp=780, r=36, build=8.0,
                          makes=[], requires=[], researches=[], tier=1, key="P",
                          desc="Defensive tower; burns attackers."),
            "bindery": dict(name="Pyre", cost=180, hp=850, r=44, build=11.0,
                            makes=["brute"], requires=["table"], researches=["ash", "regrow"], tier=2, key="B",
                            desc="Tier 2. Unlocks Molten Brutes & fire upgrades."),
            "menagerie": dict(name="Inferno Gate", cost=300, hp=1100, r=50, build=15.0,
                              makes=["colossus"], requires=["bindery"], researches=["wild"], tier=3, key="M",
                              desc="Tier 3. Looses the Wildfire."),
        },
        units={
            "scribe": dict(name="Ashscribe", cost=22, hp=52, dmg=4, spd=102, rng=14, r=11,
                           card="beekeeper", worker=True, build_t=2.6, tier=1, key="E",
                           desc="Harvests ink; raises structures."),
            "soldier": dict(name="Ember Soldier", cost=42, hp=110, dmg=16, spd=98, rng=16, r=12,
                            card="gorilla", build_t=3.2, tier=1, key="A", desc="Fast, fragile melee."),
            "archer": dict(name="Cinder Archer", cost=58, hp=70, dmg=19, spd=100, rng=155, r=11,
                           card="crane", build_t=4.0, tier=1, key="D", desc="Glass-cannon ranged."),
            "brute": dict(name="Molten Brute", cost=95, hp=250, dmg=42, spd=82, rng=18, r=17,
                          card="mammoth", build_t=6.5, tier=2, key="F", desc="Fast heavy hitter."),
            "colossus": dict(name="Wildfire", cost=210, hp=620, dmg=88, spd=72, rng=20, r=24,
                             card="phoenix", build_t=11.0, tier=3, key="C", desc="Blazing elite."),
        },
        upgrades={
            "hot": dict(name="Hotter Flames", cost=90, time=15, stat="dmg", mult=1.3,
                        applies="army", desc="+30% army attack."),
            "ash": dict(name="Ashen Speed", cost=80, time=14, stat="spd", mult=1.25,
                        applies="army", desc="+25% army speed."),
            "regrow": dict(name="Ember Regrowth", cost=110, time=20, stat="maxhp", mult=1.18,
                           applies="army", desc="+18% army health."),
            "wild": dict(name="Wildfire Spread", cost=150, time=24, stat="dmg", mult=1.35,
                         applies=["colossus"], desc="+35% Wildfire attack."),
        },
    ),
    "tide": dict(
        name="The Deepfold Tide", blurb="Defensive & economic — tanky, slow, out-lasts foes.",
        color=(70, 170, 168), dark=(40, 108, 106),
        trait="Turtle: tankier units & richer economy, but slower. Grind them down.",
        econ=1.2,
        structures={
            "scriptorium": dict(name="Tidehall", cost=0, hp=1650, r=56, build=6.0,
                                makes=["scribe"], requires=[], researches=[], tier=1, key="S",
                                desc="Town hall. Trains Pearlscribes; ink drop-off."),
            "table": dict(name="Coral Table", cost=130, hp=950, r=44, build=9.0,
                          makes=["soldier", "archer"], requires=[], researches=["plate"], tier=1, key="T",
                          desc="Trains tide troops."),
            "well": dict(name="Deep Reservoir", cost=80, hp=560, r=36, build=6.0,
                         makes=[], requires=[], researches=["current"], tier=1, key="R",
                         desc="Raises ink cap & income (rich)."),
            "spire": dict(name="Tide Spire", cost=150, hp=1100, r=40, build=10.0,
                          makes=[], requires=[], researches=[], tier=1, key="P",
                          desc="Sturdy defensive tower."),
            "bindery": dict(name="Abyssal Bindery", cost=210, hp=1200, r=48, build=13.0,
                            makes=["brute"], requires=["table"], researches=["might"], tier=2, key="B",
                            desc="Tier 2. Unlocks Bulwarks & tide upgrades."),
            "menagerie": dict(name="Leviathan Pool", cost=340, hp=1500, r=54, build=17.0,
                              makes=["colossus"], requires=["bindery"], researches=["deep"], tier=3, key="M",
                              desc="Tier 3. Raises the Leviathan."),
        },
        units={
            "scribe": dict(name="Pearlscribe", cost=25, hp=70, dmg=4, spd=88, rng=14, r=11,
                           card="beekeeper", worker=True, build_t=3.0, tier=1, key="E",
                           desc="Harvests ink (richer hauls); raises structures."),
            "soldier": dict(name="Tide Guard", cost=54, hp=210, dmg=13, spd=76, rng=16, r=14,
                            card="gorilla", build_t=4.6, tier=1, key="A", desc="Very sturdy line."),
            "archer": dict(name="Coral Slinger", cost=68, hp=120, dmg=15, spd=80, rng=150, r=12,
                           card="crane", build_t=5.4, tier=1, key="D", desc="Durable ranged."),
            "brute": dict(name="Deepfold Bulwark", cost=120, hp=480, dmg=32, spd=60, rng=18, r=19,
                          card="mammoth", build_t=9.0, tier=2, key="F", desc="Immense wall of paper."),
            "colossus": dict(name="Ink Leviathan", cost=260, hp=1200, dmg=64, spd=52, rng=24, r=28,
                             card="leviathan", build_t=16.0, tier=3, key="C", desc="Colossal sea-beast."),
        },
        upgrades={
            "plate": dict(name="Coral Plating", cost=110, time=20, stat="maxhp", mult=1.35,
                          applies="army", desc="+35% army health."),
            "current": dict(name="Deep Currents", cost=90, time=16, stat="harvest", mult=1.35,
                            applies="workers", desc="+35% ink per haul."),
            "might": dict(name="Tidal Might", cost=120, time=22, stat="dmg", mult=1.25,
                          applies="army", desc="+25% army attack."),
            "deep": dict(name="Deep Awakening", cost=170, time=28, stat="maxhp", mult=1.25,
                         applies=["colossus", "brute"], desc="+25% bulwark & leviathan health."),
        },
    ),
}
DEFAULT_FACTION = "court"


# ── MAP POOL ─────────────────────────────────────────────────────────────────
# Each map: name, size (w,h), a symmetric pair of base spawns, and extra neutral
# ink-node clusters. Bases each get two flanking nodes automatically.
MAPS = {
    "leaflock": dict(name="Leaflock Basin", size=(4200, 2800),
                     spawns=[(340, 2460), (3860, 340)], seed=1234, nodes=16,
                     desc="A broad open basin. Classic macro map, room to maneuver."),
    "creases": dict(name="The Creases", size=(3400, 3400),
                    spawns=[(340, 3060), (3060, 340)], seed=77, nodes=12,
                    desc="Square & compact. Fast fights, short rushes."),
    "spine": dict(name="Foldspine Pass", size=(4800, 2400),
                  spawns=[(320, 1200), (4480, 1200)], seed=909, nodes=14,
                  desc="A long horizontal corridor — a lane to hold or break."),
    "quill": dict(name="Quillmarsh", size=(3800, 3200),
                  spawns=[(360, 2840), (3440, 360)], seed=451, nodes=20,
                  desc="Node-rich wetland. Reward map for greedy expanders."),
    "corners": dict(name="Four Corners", size=(4000, 4000),
                    spawns=[(360, 3640), (3640, 360)], seed=2025, nodes=18,
                    desc="A large square with rich corners far from home."),
    "sliver": dict(name="Paper Sliver", size=(4600, 2000),
                   spawns=[(320, 1000), (4280, 1000)], seed=13, nodes=10,
                   desc="Thin & tense. Little room; expect early aggression."),
}
DEFAULT_MAP = "leaflock"

# ── JSON data overlay ────────────────────────────────────────────────────────
# The Python dicts above are the built-in fallback. If rts_data.json is present
# beside this module it REPLACES them, and each map may carry an "overlay" with
# custom units/structures and a list of "objects" (obstacles, rich nodes, relics)
# placed on that map. This lets maps ship bespoke rosters and hand-placed props
# without touching code. _MAP_OBJECTS[map_id] holds the parsed objects.
_MAP_OBJECTS = {}
_MAP_HIGH = {}                    # map_id -> list of (x,y,r) high-ground plateaus


def _load_palette_scripts():
    """User-authored command-palette scripts (palette_scripts.json beside this
    module; authored in the map editor's 'Palette scripts' window). Each script
    is a name + a list of ops the palette can run as one command. Bad scripts
    are reported and skipped — one typo never voids the file."""
    path = os.path.join(os.path.dirname(os.path.abspath(__file__)),
                        "palette_scripts.json")
    try:
        with open(path) as f:
            doc = json.load(f)
    except FileNotFoundError:
        return []
    except Exception as ex:
        print(f"[palette] bad palette_scripts.json: {ex}")
        return []
    out = []
    for i, s in enumerate(doc.get("scripts", [])):
        try:
            name = str(s["name"]); ops = list(s.get("ops", []))
            assert all(isinstance(o, dict) and "op" in o for o in ops)
            out.append({"name": name, "desc": str(s.get("desc", "")), "ops": ops})
        except Exception:
            print(f"[palette] skipping bad script #{i}")
    return out

def _deep_merge(base, over):
    """Recursively merge dict `over` onto a copy of `base` (over wins)."""
    out = dict(base)
    for k, v in over.items():
        if k in out and isinstance(out[k], dict) and isinstance(v, dict):
            out[k] = _deep_merge(out[k], v)
        else:
            out[k] = v
    return out

def load_data(path=None):
    """Load factions & maps from rts_data.json if present, applying per-map
    overlays. Falls back to the built-in Python dicts."""
    global FACTIONS, MAPS, STRUCTURES, UNITS, DEFAULT_FACTION, DEFAULT_MAP, _MAP_OBJECTS, _MAP_HIGH
    import os, json
    if path is None:
        path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "rts_data.json")
    try:
        with open(path) as fh:
            doc = json.load(fh)
    except Exception:
        return False                        # keep the built-in defaults
    facs = doc.get("factions") or FACTIONS
    maps = doc.get("maps") or MAPS
    _MAP_OBJECTS = {}
    for mk, m in maps.items():
        if "size" in m: m["size"] = tuple(m["size"])
        if "spawns" in m: m["spawns"] = [tuple(s) for s in m["spawns"]]
        _MAP_OBJECTS[mk] = list(m.get("objects", []))
        _MAP_HIGH[mk] = [tuple(h) for h in m.get("highground", [])]
        ov = m.get("overlay")
        if ov:
            add_units = ov.get("units", {})
            add_structs = ov.get("structures", {})
            m["_factions"] = {}
            for fk, fac in facs.items():
                merged = dict(fac)
                merged["units"] = _deep_merge(fac.get("units", {}), add_units)
                merged["structures"] = _deep_merge(fac.get("structures", {}), add_structs)
                m["_factions"][fk] = merged
            m["_add_structures"] = ov.get("faction_add_structures", [])
            _MAP_OBJECTS[mk] += list(ov.get("objects", []))
    FACTIONS = facs
    MAPS = maps
    DEFAULT_FACTION = doc.get("default_faction", DEFAULT_FACTION)
    DEFAULT_MAP = doc.get("default_map", DEFAULT_MAP)
    STRUCTURES = FACTIONS[DEFAULT_FACTION]["structures"]
    UNITS = FACTIONS[DEFAULT_FACTION]["units"]
    return True

# Module-level catalogue aliases point at the PLAYER faction, so existing code
# (and the HUD) can keep saying STRUCTURES[...]/UNITS[...] for team 0. Per-team
# lookups for the AI go through helper methods on RTS.
STRUCTURES = FACTIONS[DEFAULT_FACTION]["structures"]
UNITS = FACTIONS[DEFAULT_FACTION]["units"]

# load the JSON overlay at import (falls back silently to the Python defaults)
load_data()


# ── entities ─────────────────────────────────────────────────────────────────
class Unit:
    _UID = 0
    def __init__(self, kind, x, y, team, spec=None):
        Unit._UID += 1
        self.uid = Unit._UID
        self.kind = kind
        spec = spec if spec is not None else UNITS[kind]
        self.x, self.y = float(x), float(y)
        self.team = team
        self.hp = self.maxhp = spec["hp"]
        self.spd = spec["spd"]
        self.dmg = spec["dmg"]
        self.rng = spec["rng"]
        self.r = spec["r"]
        self.worker = spec.get("worker", False)
        self.card = spec.get("card", "gorilla")
        # spellcaster energy (0 for non-casters)
        self.max_energy = spec.get("energy", 0)
        self.energy = spec.get("energy_start", 0)
        self.ability = spec.get("ability")
        self.slow_t = 0.0           # snare/slow timer (Tide Undertow)
        self.buff_dmg = 1.0; self.buff_dmg_t = 0.0   # composable-ability buffs
        self.buff_spd = 1.0; self.buff_spd_t = 0.0
        self.shield = 0.0; self.shield_t = 0.0       # absorbing paper-ward
        self.stun_t = 0.0                            # frozen (no move/attack)
        self.shred = 1.0; self.shred_t = 0.0         # takes extra damage
        self.cast_cd = 0.0                           # ability cooldown timer
        self.cloak = spec.get("cloak", False)   # invisible unless enemy-detected
        self.momentum = 0.0         # Gilded Press: winds up on a sustained target
        self._mom_target = None
        self.trail = []             # recent positions for a cloak shimmer trail
        self.stance = "aggressive"  # aggressive | hold | patrol
        self.patrol = None          # (x,y) other end of a patrol route
        # orders
        self.goal = None            # (x,y) move target
        self.waypoints = []         # queued move targets (shift-click)
        self.attack_move = False
        self.target = None          # a Unit or Structure to attack
        self.cool = 0.0
        # worker economy state
        self.carrying = 0
        self.harvest_node = None
        self.harvest_t = 0.0
        self.state = "idle"         # idle|move|attack|toNode|mining|toBase
        self.face = 1
        self.selected = False
        self.spawn_t = 0.35
        self.burn = 0.0             # Ember mechanic: burning DoT time remaining
        self.burn_dps = 0.0
        self.atk_anim = 0.0         # attack-lunge animation timer (display)
        self.vet_xp = 0             # veterancy: kills credited to this unit
        self.vet_rank = 0           # 0..3; each rank buffs dmg & hp

    @property
    def alive(self):
        return self.hp > 0


class Structure:
    _UID = 0
    def __init__(self, kind, x, y, team, complete=False, spec=None):
        Structure._UID += 1
        self.uid = Structure._UID
        self.kind = kind
        spec = spec if spec is not None else STRUCTURES[kind]
        self.x, self.y = float(x), float(y)
        self.team = team
        self.hp = self.maxhp = spec["hp"]
        self.r = spec["r"]
        self.build_total = spec["build"]
        self.build_left = 0.0 if complete else spec["build"]
        self.queue = []             # unit kinds queued to train
        self.train_t = 0.0
        self.rally = None
        self.selected = False
        self.cool = 0.0             # for the spire's attacks
        self.research = None        # (upgrade_key, time_left) while researching

    @property
    def alive(self):
        return self.hp > 0

    @property
    def complete(self):
        return self.build_left <= 0


class InkNode:
    def __init__(self, x, y, amount=1500):
        self.x, self.y = float(x), float(y)
        self.amount = amount
        self.r = 26


class MapObject:
    """A hand-placed map prop from the JSON overlay. Kinds:
      • ruin/obstacle — blocks building placement (and optionally pathing)
      • richnode       — a high-yield ink node (spawns a real InkNode)
      • relic          — a bonus point; holding the tile trickles ink to whoever
                         has units nearby
      • decoration     — pure visual."""
    def __init__(self, spec):
        self.kind = spec.get("kind", "decoration")
        self.x = float(spec.get("x", 0)); self.y = float(spec.get("y", 0))
        self.r = float(spec.get("r", 30))
        self.blocks = bool(spec.get("blocks", self.kind in ("ruin", "obstacle", "rock")))
        self.color = tuple(spec.get("color", (110, 100, 86)))
        self.label = spec.get("label", self.kind.title())
        self.bonus = spec.get("bonus")
        self.amount = spec.get("amount", 0)
        self.owner = None                   # relic/watchtower: team currently holding it
        # destructible rock: a neutral blocker with HP that can be cleared
        self.destructible = bool(spec.get("destructible", False))
        self.hp = float(spec.get("hp", 600)); self.maxhp = self.hp
        self.alive = True
        self.team = None                    # neutral — attackable by anyone
        # watchtower: controlling it reveals a wide area
        self.watchtower = bool(spec.get("watchtower", False))
        self.sight = float(spec.get("sight", 560))
        self.spec = spec


# ── composable ability effects ───────────────────────────────────────────────
# An ability is a bundle of effect primitives the engine runs generically. This
# lets the map editor author custom spells from a fixed vocabulary of effects
# without new engine code. Each effect: {"effect": <name>, "target": <mode>, ...}.
#
# Effects:
#   damage   amount, dot(bool)       — hurt targets (instant or /sec)
#   heal     amount, dot(bool)       — mend targets (instant or /sec)
#   slow     duration                — snare targets (slow movement)
#   stun     duration                — freeze targets (no move or attack)
#   burn     dps, duration           — set targets alight (stacking DoT)
#   buff     stat(dmg|spd), mult, duration — strengthen targets
#   shield   amount, duration        — grant absorbing paper-ward HP
#   shred    mult, duration          — targets take more damage (armour shred)
#   drain    amount, dot(bool)       — damage enemies, heal the caster for it
#   knockback amount                 — shove targets away from the spell centre
#   pull      amount                 — draw targets toward the spell centre
#   spawn    unit, count             — summon units for the caster's team
#   reveal   (radius from ability)   — reveal fog in the area for team 0
#
# Targeting modes (per effect, via "target"):
#   enemy (default for damage/slow/stun/burn/shred/drain/knockback/pull)
#   ally  (default for heal/buff/shield)   ·   all   ·   self (caster only)
EFFECT_PRIMITIVES = ("damage", "heal", "slow", "stun", "burn", "buff", "shield",
                     "shred", "drain", "knockback", "pull", "spawn", "reveal")

# visual themes an effect can wear (its particle colour + upward drift), so a
# custom fire spell *looks* like fire regardless of what it does
VFX_THEMES = {
    "fire":   ((250, 130, 45), 40),
    "ice":    ((150, 215, 240), 6),
    "ink":    ((70, 60, 110), 8),
    "holy":   ((250, 240, 180), 30),
    "shadow": ((120, 70, 140), 4),
    "nature": ((120, 200, 110), 16),
    "arcane": ((210, 110, 220), 20),
    "water":  ((90, 180, 210), 10),
    "gold":   ((240, 200, 90), 24),
    "blood":  ((200, 60, 70), 6),
}


def _ability_effects(ability):
    """Return the ability's effect list. New abilities carry an explicit
    'effects' list; legacy ones (kind = storm/heal/snare) are mapped on the fly
    so old data keeps working."""
    if ability.get("effects"):
        return ability["effects"]
    kind = ability.get("kind")
    if kind == "storm":
        return [dict(effect="damage", amount=ability.get("dps", 20), dot=True)]
    if kind == "heal":
        return [dict(effect="heal", amount=ability.get("amount", 100), dot=True)]
    if kind == "snare":
        return [dict(effect="slow", factor=0.5, duration=0.4),
                dict(effect="damage", amount=ability.get("dps", 10), dot=True)]
    if kind == "buff":
        return [dict(effect="buff", mult=ability.get("amount", 1.4),
                     duration=ability.get("dur", 5.0), target="ally")]
    return []


# ── the RTS match ────────────────────────────────────────────────────────────
class RTS:
    def __init__(self, ctx, map_id=None, factions=None):
        self.ctx = ctx                      # {W,H,font,sfont,bfont,draw_unit,cards}
        self.W = ctx["W"]; self.H = ctx["H"]
        self.map_id = map_id or DEFAULT_MAP
        self.mapdef = MAPS[self.map_id]
        # per-team faction ("court"/"ember"/"tide"); [player, enemy]
        factions = factions or [DEFAULT_FACTION, "ember"]
        self.fac = {0: factions[0], 1: factions[1]}
        # point the module aliases at the player faction so the HUD reads right
        global STRUCTURES, UNITS
        _fb = self.mapdef.get("_factions") or FACTIONS
        STRUCTURES = _fb[self.fac[0]]["structures"]
        UNITS = _fb[self.fac[0]]["units"]
        # researched upgrades + in-progress research per team
        self.upgrades = {0: set(), 1: set()}
        # world size from the chosen map
        global WORLD_W, WORLD_H
        WORLD_W, WORLD_H = self.mapdef["size"]
        self.units = []
        self.structs = []
        self.nodes = []
        self.objects = []                   # MapObjects from the JSON overlay
        self._blockers = []                 # blocking obstacles (for avoidance)
        self._high = []                     # high-ground plateaus (x,y,r)
        self._ramps = []                    # resolved ramps (x,y,w,len,ddx,ddy)
        self.fauna = []                     # ambient living-map agents
        self.surge = None                   # active ink surge
        self.surge_timer = 999.0
        self.dying = []                     # display-only crumpling corpses
        base_ink = 120
        self.ink = {0: base_ink, 1: base_ink}
        self.cap = {0: 400, 1: 400}
        self.done = False
        self.result = None
        self.exit_requested = False         # ESC-to-menu signal for the host (game.py)
        # Edge-pan gating: don't pan until the mouse has actually entered the
        # window AND a short grace period has passed, so the camera doesn't fly
        # off toward wherever the OS cursor happened to be at scene start.
        self.edge_pan_armed = False
        self.pan_grace = 1.5                # seconds before edge panning arms
        self.msg = ""
        self.msg_t = 0.0
        self.show_tech = False              # tech-tree overlay toggle
        self.stats = {0: dict(built=0, lost=0, killed=0, harvested=0.0, structs=0, vets=0),
                      1: dict(built=0, lost=0, killed=0, harvested=0.0, structs=0, vets=0)}
        self.match_t = 0.0                  # elapsed match time (for the summary)
        self.fog_on = True                  # fog of war enabled
        self.fog = None                     # grid: 0 unseen, 1 explored, 2 visible
        self.fog_cell = 120                 # world units per fog cell
        self.fog_t = 0.0                    # throttle vision recompute
        self.pings = []                     # {x,y,t} under-attack minimap pings
        self.alert_t = 0.0                  # throttle for the "under attack" alert
        self.cast_mode = None               # (ability, [casters]) while targeting a spell
        self.spells = []                    # active spell effects on the field
        self.particles = []                 # transient visual particles
        self.projectiles = []               # in-flight paper darts / ink bolts
        self.spell_bolts = []               # travelling spell projectiles (bolt delivery)
        self.phase = "setup"                # "setup" (pick faction/map) → "play"
        self.setup_fac = 0                  # index into faction list
        self.setup_map = 0                  # index into map list
        self.setup_rects = []
        # camera (world coords of the top-left of the viewport)
        self.cam_x = 0.0
        self.cam_y = 0.0
        self.zoom = 0.7                     # start zoomed out to see the big map
        self.mid_drag = None                # (mouse_x, mouse_y, cam_x, cam_y) while middle-drag panning
        self.mouse_pos = None               # last pointer pos in CANVAS space (from translated events)
        self._ink_anim = {}                 # ui key -> 0..1 ink-pop expansion state
        self._frame_dt = 1 / 60.0           # last update dt, drives ui pops in drawers
        self._pan_ramp = 0.0                # edge-pan ease-in (0..1) so panning never jerks
        self._grp_tap = (None, -9.0)        # (digit, time) for double-tap → centre camera
        self.pal = None                     # command palette: None | {"q": str, "sel": int}
        self.pal_scripts = _load_palette_scripts()
        # ── gamepad (Halo Wars-style console controls) ──
        # All state is fed by JOY* events (never polled), so it's testable and
        # works with whatever stick the host inits. Reticle rides the middle of
        # the view; the left stick steers the camera, the right stick fine-aims.
        self.pad = dict(active=False, ax={}, cx=self.W / 2, cy=(self.H - 118) / 2,
                        a_down=0.0, painting=False, paint_r=0.0, base_i=0)
        self.tournament = None              # active tournament state (dict)
        self._t_reported = False            # this match's result recorded?
        # selection & control
        self.selected = []
        self.groups = {i: [] for i in range(1, 10)}
        self.box = None                     # (x0,y0,x1,y1) in screen space while dragging
        self.build_mode = None              # a structure kind pending placement
        self.palette_rects = []             # (rect, action) hot regions
        self.mm_rect = None                 # minimap rect on screen
        self.buttons = []                   # (rect, action) quick-command buttons
        # ── touch / hybrid input state ──
        self.fingers = {}                   # fid -> {x,y,x0,y0,t,moved}
        self.wheel = None                   # active radial command wheel {x,y,slices}
        self.wheel_hot = None               # slice index under the cursor
        self.pinch0 = None                  # (dist, zoom) when a two-finger pinch begins
        self.pan_touch = None               # (cx,cy,cam_x,cam_y) two-finger pan anchor
        # AI
        self.ai_t = 4.0
        self.ai_phase = "grow"
        self.difficulty = 1                 # 0 easy, 1 normal, 2 hard
        self._diff_names = ["Easy", "Normal", "Hard"]
        self.spectate = False               # AI-vs-AI observer mode (no player)
        # If factions/map were passed explicitly (e.g. from a menu), skip the
        # in-game picker; otherwise start in the setup screen.
        if factions is not None and map_id is not None:
            self.phase = "play"
            self._setup()

    def _begin_match(self, spectate=False):
        """Commit the setup-screen choices and start the skirmish."""
        fac_keys = list(FACTIONS.keys())
        map_keys = list(MAPS.keys())
        self.fac[0] = fac_keys[self.setup_fac % len(fac_keys)]
        # enemy picks a different faction for variety — unless a tournament
        # (or a test) has forced the pairing
        forced = getattr(self, "forced_enemy", None)
        if forced and forced in fac_keys:
            self.fac[1] = forced
        else:
            others = [k for k in fac_keys if k != self.fac[0]] or fac_keys
            self.fac[1] = random.choice(others)
        self.map_id = map_keys[self.setup_map % len(map_keys)]
        self.mapdef = MAPS[self.map_id]
        global STRUCTURES, UNITS, WORLD_W, WORLD_H
        fb = self.mapdef.get("_factions") or FACTIONS
        STRUCTURES = fb[self.fac[0]]["structures"]
        UNITS = fb[self.fac[0]]["units"]
        WORLD_W, WORLD_H = self.mapdef["size"]
        self.phase = "play"
        self.spectate = spectate
        self._setup()
        if spectate:
            self.fog_on = False          # observer sees the whole battlefield
            self.flash(f"Observing: {self.facdata(0)['name']} vs {self.facdata(1)['name']}")

    def _sfx(self, name, gain=0.6):
        fn = self.ctx.get("sfx")
        if fn:
            try: fn(name, gain)
            except Exception: pass

    # ── faction catalogue lookups (per team, map-overlay aware) ──
    def _factbook(self):
        """The faction catalogue for THIS match — the map's overlay set if it has
        one, else the global FACTIONS."""
        return self.mapdef.get("_factions") or FACTIONS
    def sspec(self, team, kind):
        return self._factbook()[self.fac[team]]["structures"][kind]
    def uspec(self, team, kind):
        return self._factbook()[self.fac[team]]["units"][kind]
    def facdata(self, team):
        return self._factbook()[self.fac[team]]

    def _struct_available(self, team, kind):
        """Tech gate: prerequisites built & complete, and any required research
        finished — trees can branch through upgrades, not just buildings."""
        spec = self.sspec(team, kind)
        for r in spec.get("requires", []):
            if not any(s.team == team and s.kind == r and s.complete for s in self.structs):
                return False
        need = spec.get("requires_research")
        if need and need not in self.upgrades[team]:
            return False
        return True

    def _unit_available(self, team, kind):
        """A unit is trainable if some COMPLETE structure of ours makes it AND
        any research it demands is finished."""
        spec = self.uspec(team, kind) or {}
        need = spec.get("requires_research")
        if need and need not in self.upgrades[team]:
            return False
        for s in self.structs:
            if s.team == team and s.complete and kind in self.sspec(team, s.kind).get("makes", []):
                return True
        return False

    def _apply_upgrades(self, u):
        """Bake this team's researched upgrades into a freshly-built unit."""
        ups = self._factbook()[self.fac[u.team]]["upgrades"]
        for uk in self.upgrades[u.team]:
            up = ups.get(uk)
            if not up:
                continue
            applies = up["applies"]
            hit = (applies == "all"
                   or (applies == "army" and not u.worker)
                   or (applies == "workers" and u.worker)
                   or (isinstance(applies, (list, tuple)) and u.kind in applies))
            if not hit:
                continue
            st = up["stat"]
            if st == "maxhp":
                u.maxhp *= up["mult"]; u.hp = u.maxhp
            elif st == "dmg":
                u.dmg *= up["mult"]
            elif st == "spd":
                u.spd *= up["mult"]
            elif st == "rng":
                u.rng *= up["mult"]
            # "harvest" handled at drop-off time via _harvest_mult

    def _retro_upgrade(self, team, uk):
        """Apply a just-finished upgrade to all EXISTING units of that team."""
        up = self._factbook()[self.fac[team]]["upgrades"].get(uk)
        if not up or up["stat"] == "harvest":
            return
        for u in self.units:
            if u.team != team:
                continue
            applies = up["applies"]
            hit = (applies == "all"
                   or (applies == "army" and not u.worker)
                   or (applies == "workers" and u.worker)
                   or (isinstance(applies, (list, tuple)) and u.kind in applies))
            if not hit:
                continue
            frac = u.hp / u.maxhp if u.maxhp else 1.0
            if up["stat"] == "maxhp":
                u.maxhp *= up["mult"]; u.hp = u.maxhp * frac
            elif up["stat"] == "dmg":
                u.dmg *= up["mult"]
            elif up["stat"] == "spd":
                u.spd *= up["mult"]
            elif up["stat"] == "rng":
                u.rng *= up["mult"]

    def _start_research(self, s, uk):
        if not s.complete or s.research is not None:
            return
        if uk in self.upgrades[s.team]:
            if s.team == 0: self.flash("Already researched.")
            return
        up = self._factbook()[self.fac[s.team]]["upgrades"][uk]
        if self.ink[s.team] < up["cost"]:
            if s.team == 0: self.flash("Not enough ink.")
            return
        self.ink[s.team] -= up["cost"]
        s.research = (uk, up["time"])
        if s.team == 0: self.flash(f"Researching {up['name']}…")

    def _harvest_mult(self, team):
        m = 1.0
        ups = self._factbook()[self.fac[team]]["upgrades"]
        for uk in self.upgrades[team]:
            up = ups.get(uk)
            if up and up.get("stat") == "harvest":
                m *= up["mult"]
        return m

    # ── supply / population (StarCraft-style) ──
    def _supply_cap(self, team):
        cap = 0
        for s in self.structs:
            if s.team == team and s.complete:
                cap += self.sspec(team, s.kind).get("supply", 0)
        return min(cap, 200)                 # a hard ceiling like SC's 200
    def _supply_used(self, team):
        used = 0
        for u in self.units:
            if u.alive and u.team == team:
                used += self.uspec(team, u.kind).get("supply", 1)
        # count queued + in-progress units too, so you can't over-queue
        for s in self.structs:
            if s.team == team:
                for uk in s.queue:
                    used += self.uspec(team, uk).get("supply", 1)
        return used
    def _supply_free(self, team, uk):
        cost = self.uspec(team, uk).get("supply", 1)
        return self._supply_used(team) + cost <= self._supply_cap(team)

    def _tcol(self, team):
        return FACTIONS[self.fac[team]]["color"]
    def _tdark(self, team):
        return FACTIONS[self.fac[team]]["dark"]

    # ── map setup ──
    def _setup(self):
        spawns = self.mapdef["spawns"]
        self._make_base(0, *spawns[0])
        self._make_base(1, *spawns[1])
        # scatter neutral ink nodes, mirrored across the map centre so both
        # spawns get identical resource access (fairness). We place half at
        # random and reflect each through the centre point.
        rng = random.Random(self.mapdef["seed"])
        cx, cy = WORLD_W / 2, WORLD_H / 2
        half = max(1, self.mapdef["nodes"] // 2)
        for _ in range(half):
            x = rng.uniform(300, WORLD_W - 300)
            y = rng.uniform(300, WORLD_H - 300)
            amt = rng.randint(1200, 2000)
            # skip if this random point sits almost on top of a base (both sides)
            near_base = any(math.hypot(x - bx, y - by) < 240 or
                            math.hypot((2 * cx - x) - bx, (2 * cy - y) - by) < 240
                            for bx, by in self.mapdef["spawns"])
            if near_base:
                continue
            self.nodes.append(InkNode(x, y, amt))
            self.nodes.append(InkNode(2 * cx - x, 2 * cy - y, amt))   # mirror
        # hand-placed map objects from the JSON overlay (obstacles, rich nodes,
        # relics). Rich nodes become real harvestable InkNodes.
        for spec in _MAP_OBJECTS.get(self.map_id, []):
            obj = MapObject(spec)
            self.objects.append(obj)
            if obj.kind == "richnode":
                self.nodes.append(InkNode(obj.x, obj.y, obj.amount or 3000))
        # authored ink FIELDS: clusters of wells packed around a point —
        # [x, y, count, spread, amount_each]; mirrored like the scatter so both
        # players face the same economy.
        for cl in self.mapdef.get("node_clusters", []):
            try:
                fx, fy, cnt, spread = cl[0], cl[1], int(cl[2]), cl[3]
                amt = cl[4] if len(cl) > 4 else 1600
            except Exception:
                continue
            crng = random.Random(int(self.mapdef.get("seed", 1)) ^ (int(fx) * 7919) ^ int(fy))
            for _ in range(cnt):
                a = crng.uniform(0, math.tau)
                d = spread * math.sqrt(crng.uniform(0.05, 1.0))
                self.nodes.append(InkNode(fx + math.cos(a) * d,
                                          fy + math.sin(a) * d * 0.8, amt))
        # cache the blocking obstacles for movement avoidance
        self._blockers = [o for o in self.objects if o.blocks]
        self.rocks = [o for o in self.objects if o.destructible]
        self.watchtowers = [o for o in self.objects if o.watchtower]
        self._high = list(_MAP_HIGH.get(self.map_id, []))
        # ramps: authored runs up onto the plateaus — [x, y, width, length, dir]
        # where (x,y) is the FOOT of the ramp and dir ("n"/"s"/"e"/"w") points UP
        # toward the plateau. Stored resolved as (x, y, w, len, ddx, ddy).
        self._ramps = []
        _DIRS = {"n": (0, -1), "s": (0, 1), "e": (1, 0), "w": (-1, 0)}
        for r in self.mapdef.get("ramps", []):
            try:
                rx, ry, rw, rl, d = r
                ddx, ddy = _DIRS.get(str(d).lower(), (0, -1))
                self._ramps.append((float(rx), float(ry), float(rw), float(rl),
                                    float(ddx), float(ddy)))
            except Exception:
                pass
        # art theme (paper palette) for this map, with sane defaults
        self.theme = self.mapdef.get("theme", dict(
            ground=[196, 178, 148], grid=[186, 168, 138], ruled=[205, 150, 120],
            crease=[170, 150, 120], accent=[150, 120, 80]))
        self.stains = [tuple(s) for s in self.mapdef.get("stains", [])]
        self.props = [dict(p) for p in self.mapdef.get("props", [])]   # themed decor
        self.bands = [tuple(b) for b in self.mapdef.get("bands", [])]
        self.pools = [tuple(p) for p in self.mapdef.get("pools", [])]
        # ── living map: ambient fauna + dynamic ink surges ──
        # fauna is authored per map ("fauna": [{"kind","count"}...]) with
        # sensible defaults derived from the terrain: paper birds always cross
        # the page, ink fish live wherever there are pools, beetles skitter
        # where there's enough decor to hide under.
        fauna_spec = self.mapdef.get("fauna")
        if fauna_spec is None:
            fauna_spec = [{"kind": "birds", "count": 6}]
            if self.pools:
                fauna_spec.append({"kind": "fish", "count": 3 * len(self.pools)})
            if len(self.props) >= 6:
                fauna_spec.append({"kind": "beetles", "count": 4})
        self.fauna = []
        frng = random.Random(int(self.mapdef.get("seed", 1)) ^ 0xFA0A)
        for fs in fauna_spec:
            kind = fs.get("kind"); cnt = int(fs.get("count", 4))
            if kind == "birds":
                # one flock: a heading + per-bird offsets; loops around the map
                hx = frng.uniform(-1, 1); hy = frng.uniform(-0.4, 0.4)
                mag = math.hypot(hx, hy) or 1.0
                self.fauna.append(dict(kind="birds",
                    x=frng.uniform(0, WORLD_W), y=frng.uniform(0, WORLD_H),
                    vx=hx / mag * 46, vy=hy / mag * 46,
                    offs=[(frng.uniform(-70, 70), frng.uniform(-40, 40),
                           frng.uniform(0, math.tau)) for _ in range(cnt)]))
            elif kind == "fish" and self.pools:
                for _ in range(cnt):
                    px, py, pr = self.pools[frng.randrange(len(self.pools))]
                    self.fauna.append(dict(kind="fish", pool=(px, py, pr),
                        a=frng.uniform(0, math.tau), spd=frng.uniform(0.3, 0.7),
                        r=frng.uniform(0.3, 0.75) * pr, ph=frng.uniform(0, 9)))
            elif kind == "beetles":
                for _ in range(cnt):
                    x = frng.uniform(200, WORLD_W - 200)
                    y = frng.uniform(200, WORLD_H - 200)
                    self.fauna.append(dict(kind="beetle", x=x, y=y, tx=x, ty=y,
                                           rest=frng.uniform(1, 4)))
        self.surge = None                    # active ink surge: dict | None
        self.surge_timer = frng.uniform(18, 30)
        # event scripts authored in the map editor (trigger → actions)
        self.events = copy.deepcopy(self.mapdef.get("events", []))
        for e in self.events:
            e["_fired"] = False
        self._build_paper_tile()
        # ambient drifting paper motes for atmosphere (world-space, seeded)
        import random as _rnd
        _mr = _rnd.Random(self.mapdef.get("seed", 1) + 7)
        self.motes = [dict(x=_mr.uniform(0, WORLD_W), y=_mr.uniform(0, WORLD_H),
                           drift=_mr.uniform(6, 18), ang=_mr.uniform(0, math.tau),
                           ph=_mr.uniform(0, math.tau), sz=_mr.uniform(1.0, 2.4))
                      for _ in range(70)]
        # weather / lighting mood for this map
        self.weather = self.mapdef.get("weather", dict(kind="clear", tint=[255, 245, 220], strength=0.08))
        self._build_weather_overlay()
        # weather drops (rain streaks / drifting mist), screen-space, seeded
        self.wdrops = [dict(x=_mr.uniform(0, self.W), y=_mr.uniform(0, self.H),
                            sp=_mr.uniform(0.6, 1.4), ph=_mr.uniform(0, math.tau))
                       for _ in range(90 if self.weather["kind"] in ("storm", "mist") else 0)]
        self.lightning_t = 0.0
        # fog of war grid
        self.fog_cols = int(WORLD_W // self.fog_cell) + 1
        self.fog_rows = int(WORLD_H // self.fog_cell) + 1
        self.fog = [[0] * self.fog_cols for _ in range(self.fog_rows)]
        self._update_fog(force=True)
        # center the camera on the player's base
        bx, by = spawns[0]
        self.cam_x = bx - self.W / (2 * self.zoom)
        self.cam_y = by - (self.H - 118) / (2 * self.zoom)
        self._clamp_cam()
        self.flash(f"{self.mapdef['name']} — {self.facdata(0)['name']} vs {self.facdata(1)['name']}.")

    def _make_base(self, team, bx, by):
        hall = Structure("scriptorium", bx, by, team, complete=True,
                         spec=self.sspec(team, "scriptorium"))
        self.structs.append(hall)
        for i in range(4):
            u = Unit("scribe", bx + math.cos(i) * 70, by + math.sin(i) * 70, team,
                     spec=self.uspec(team, "scribe"))
            u.spawn_t = 0
            self.units.append(u)
        # two ink nodes flanking each base
        off = 150 if team == 0 else -150
        self.nodes.append(InkNode(bx + off, by - 40, 1800))
        self.nodes.append(InkNode(bx - 40, by + off, 1800))

    def flash(self, text):
        self.msg = text
        self.msg_t = 4.0

    # ── coordinate transforms ──
    # The page is viewed at an angle (like the main game's board): world y is
    # foreshortened by ISO, and points on high ground / ramps additionally rise
    # by a screen-vertical lift. w2s/s2w are the pure GROUND-PLANE mapping (all
    # picking runs through them, so clicks land on the ground point); wpos() is
    # the elevation-aware position drawers use.
    def w2s(self, x, y):
        return (x - self.cam_x) * self.zoom, (y - self.cam_y) * self.zoom * ISO
    def s2w(self, sx, sy):
        return sx / self.zoom + self.cam_x, sy / (self.zoom * ISO) + self.cam_y
    def _lift(self, x, y):
        """Visual elevation (world units) at a point: full on a plateau, a smooth
        run up along a ramp, zero on the flat page."""
        for hx, hy, hr in self._high:
            if (x - hx) ** 2 + (y - hy) ** 2 < hr * hr:
                return HG_LIFT
        for (rx, ry, rw, rl, ddx, ddy) in getattr(self, "_ramps", []):
            # ramp-local coords: `along` runs 0 (foot) → rl (top edge)
            lx, ly = x - rx, y - ry
            along = lx * ddx + ly * ddy
            across = abs(lx * -ddy + ly * ddx)
            if 0.0 <= along <= rl and across <= rw * 0.5:
                return HG_LIFT * (along / rl)
        return 0.0
    def wpos(self, x, y):
        sx, sy = self.w2s(x, y)
        lf = self._lift(x, y)
        if lf:
            sy -= lf * self.zoom
        return sx, sy
    def disc(self, screen, col, x, y, wr, width=0, lift=True):
        """A disc lying ON the page: an ellipse foreshortened by ISO, lifted if
        it sits on high ground. The workhorse for rings/footprints/pools."""
        sx, sy = (self.wpos if lift else self.w2s)(x, y)
        rr = max(1, int(wr * self.zoom))
        ry = max(1, int(rr * ISO))
        rect = pygame.Rect(int(sx - rr), int(sy - ry), rr * 2, ry * 2)
        pygame.draw.ellipse(screen, col, rect, width)
        return rect
    def _on_screen(self, wx, wy, margin=40):
        sx, sy = self.w2s(wx, wy)
        return -margin < sx < self.W + margin and -margin < sy < self.H - 118 + margin

    # ── input ──
    def handle_event(self, ev):
        if ev.type in (pygame.JOYAXISMOTION, pygame.JOYBUTTONDOWN,
                       pygame.JOYBUTTONUP, pygame.JOYHATMOTION):
            if self.phase == "setup":
                self._pad_setup_input(ev)
            else:
                self._on_joy(ev)
            return
        if self.phase == "setup":
            self._setup_input(ev)
            return
        if ev.type == pygame.KEYDOWN:
            self._on_key(ev)
        elif ev.type == pygame.MOUSEBUTTONDOWN:
            self._on_mouse_down(ev)
        elif ev.type == pygame.MOUSEBUTTONUP:
            self._on_mouse_up(ev)
        elif ev.type == pygame.MOUSEMOTION:
            self._on_mouse_motion(ev)
        elif ev.type == pygame.MOUSEWHEEL:
            self._zoom_at(pygame.mouse.get_pos(), 1.12 if ev.y > 0 else 0.89)
        elif ev.type == pygame.FINGERDOWN:
            self._on_finger_down(ev)
        elif ev.type == pygame.FINGERUP:
            self._on_finger_up(ev)
        elif ev.type == pygame.FINGERMOTION:
            self._on_finger_motion(ev)

    def _zoom_at(self, screen_pt, factor):
        """Zoom toward the given screen point, keeping the world spot under the
        cursor fixed (StarCraft/Halo Wars-style focal zoom)."""
        sx, sy = screen_pt
        wx, wy = self.s2w(sx, sy)
        self.zoom = max(ZOOM_MIN, min(ZOOM_MAX, self.zoom * factor))
        # re-anchor so (wx,wy) stays under (sx,sy)
        self.cam_x = wx - sx / self.zoom
        self.cam_y = wy - sy / self.zoom
        self._clamp_cam()

    def _on_mouse_motion(self, ev):
        # ev.pos is already translated into canvas space by the host, unlike
        # pygame.mouse.get_pos() which is window space — remember it for edge-pan.
        self.mouse_pos = ev.pos
        if self.box:
            self.box = (self.box[0], self.box[1], ev.pos[0], ev.pos[1])
        if self.mid_drag:
            mx0, my0, cx0, cy0 = self.mid_drag
            self.cam_x = cx0 - (ev.pos[0] - mx0) / self.zoom
            self.cam_y = cy0 - (ev.pos[1] - my0) / (self.zoom * ISO)
            self._clamp_cam()
        if self.wheel:
            self.wheel_hot = self._wheel_slice_at(ev.pos)

    def _group(self, n):
        """Group n with the fallen pruned out (kept pruned in place)."""
        g = [u for u in self.groups.get(n, []) if u.alive]
        self.groups[n] = g
        return g

    def _centre_on(self, x, y):
        self.cam_x = x - self.W / (2 * self.zoom)
        self.cam_y = y - (self.H - 118) / (2 * self.zoom * ISO)
        self._clamp_cam()

    # ── gamepad: Halo Wars-style console controls ───────────────────────────
    #   Left stick   steer the camera (reticle rides the view)
    #   Right stick  fine-aim the reticle
    #   A            select at reticle;  HOLD A grows a paint-select ball
    #   B            cancel (wheel / build / cast / selection)
    #   X            select the whole army (global)
    #   Y            radial command wheel at the reticle (A confirms, B cancels)
    #   LB / RB      zoom out / in
    #   RT           smart order at the reticle (move / attack / harvest)
    #   D-pad        up: base camera · down: idle worker · left/right: cycle bases
    #   Start        tech tree
    _PAD_DEAD = 0.18

    def _pad_pos(self):
        return self.pad["cx"], self.pad["cy"]

    def _on_joy(self, ev):
        p = self.pad
        if ev.type == pygame.JOYAXISMOTION:
            p["ax"][ev.axis] = ev.value
            if abs(ev.value) > 0.3:
                p["active"] = True
            # RT edge → smart order (axis 5 or 2 by driver; triggers rest at -1)
            if ev.axis in (5, 2) and ev.value > 0.6 and not p.get("rt_held"):
                p["rt_held"] = True
                wx, wy = self.s2w(*self._pad_pos())
                self._command(wx, wy)
                self._emit(wx, wy, 3, "ink", (240, 230, 120),
                           spread=10, life=0.3, up=30)
            elif ev.axis in (5, 2) and ev.value < 0.3:
                p["rt_held"] = False
            return
        if ev.type == pygame.JOYHATMOTION:
            p["active"] = True
            hx, hy = ev.value
            if hy > 0:
                self._quick_action("base")
            elif hy < 0:
                self._quick_action("idle")
            elif hx != 0:
                bases = [s for s in self.structs if s.team == 0 and s.complete
                         and self.sspec(0, s.kind).get("makes")]
                if bases:
                    p["base_i"] = (p["base_i"] + (1 if hx > 0 else -1)) % len(bases)
                    b = bases[p["base_i"]]
                    self._centre_on(b.x, b.y)
                    self._select([b])
            return
        if ev.type == pygame.JOYBUTTONDOWN:
            p["active"] = True
            b = ev.button
            if b == 0:                       # A — arm select / confirm wheel
                if self.wheel:
                    self._wheel_click(self._pad_pos()); return
                p["a_down"] = 0.0001; p["painting"] = False; p["paint_r"] = 0.0
            elif b == 1:                     # B — cancel, HW priority order
                if self.wheel:
                    self.wheel = None; self.wheel_hot = None
                elif self.cast_mode:
                    self.cast_mode = None
                elif self.build_mode:
                    self.build_mode = None
                elif self.pal is not None:
                    self.pal = None
                elif self.selected:
                    self.selected = []; self._resync_selected()
            elif b == 2:                     # X — the classic "everyone!"
                self._quick_action("army")
            elif b == 3:                     # Y — radial wheel at the reticle
                if self.wheel:
                    self.wheel = None; self.wheel_hot = None
                else:
                    self._open_wheel(*self._pad_pos())
            elif b == 4:                     # LB — zoom out
                self._zoom_step(-1)
            elif b == 5:                     # RB — zoom in
                self._zoom_step(1)
            elif b == 7:                     # Start — tech tree
                self.show_tech = not self.show_tech
            return
        if ev.type == pygame.JOYBUTTONUP and ev.button == 0:
            if p["painting"]:
                wx, wy = self.s2w(*self._pad_pos())
                wr = p["paint_r"] / self.zoom
                picked = [u for u in self.units
                          if u.team == 0 and u.alive
                          and math.hypot(u.x - wx, u.y - wy) <= wr]
                if picked:
                    self._select(picked)
                    self.flash(f"Selected {len(picked)}")
            elif p["a_down"] > 0:
                self._click_select(*self._pad_pos(), add=False)
            p["a_down"] = 0.0; p["painting"] = False; p["paint_r"] = 0.0

    def _pad_setup_input(self, ev):
        """Setup-screen navigation on a pad: d-pad/stick picks faction & map,
        A starts, B backs out."""
        if ev.type == pygame.JOYHATMOTION:
            hx, hy = ev.value
            if hx:
                self.setup_fac = (self.setup_fac + hx) % max(1, len(FACTIONS))
            if hy:
                self.setup_map = (self.setup_map - hy) % max(1, len(MAPS))
        elif ev.type == pygame.JOYBUTTONDOWN:
            if ev.button == 0:
                self._begin_match()
            elif ev.button == 1:
                self.exit_requested = True

    def _zoom_step(self, direction):
        self.zoom = max(ZOOM_MIN, min(ZOOM_MAX, self.zoom * (1.18 ** direction)))
        self._clamp_cam()

    def _pad_tick(self, dt):
        """Per-frame stick integration: camera pan, reticle aim, paint growth,
        wheel highlight tracking. Idle pads cost nothing."""
        p = self.pad
        if not p["active"]:
            return
        ax = p["ax"]
        d = self._PAD_DEAD
        lx = ax.get(0, 0.0); ly = ax.get(1, 0.0)
        rx = ax.get(3, 0.0); ry = ax.get(4, 0.0)
        if abs(lx) > d or abs(ly) > d:
            sp = 900 * dt / max(0.55, self.zoom)
            self.cam_x += lx * sp
            self.cam_y += ly * sp
            self._clamp_cam()
        if abs(rx) > d or abs(ry) > d:
            p["cx"] = max(10, min(self.W - 10, p["cx"] + rx * 620 * dt))
            p["cy"] = max(10, min(self.H - 128, p["cy"] + ry * 620 * dt))
        else:
            # the reticle relaxes back toward the view centre, HW-style
            p["cx"] += (self.W / 2 - p["cx"]) * min(1.0, dt * 3.0)
            p["cy"] += ((self.H - 118) / 2 - p["cy"]) * min(1.0, dt * 3.0)
        if p["a_down"] > 0:
            p["a_down"] += dt
            if p["a_down"] > 0.25:           # the selection ball starts to grow
                p["painting"] = True
                p["paint_r"] = min(240.0, p["paint_r"] + 320 * dt)
        if self.wheel:
            self.wheel_hot = self._wheel_slice_at(self._pad_pos())
        # keep the canvas-pointer story coherent for hover/ghost drawing
        self.mouse_pos = self._pad_pos()

    def _draw_pad_reticle(self, screen):
        if not self.pad["active"]:
            return
        cx, cy = self._pad_pos()
        t = getattr(self, "_anim_t", 0.0)
        g = 8 + math.sin(t * 4) * 1.5
        for sxd, syd in ((-1, -1), (1, -1), (1, 1), (-1, 1)):
            x0 = cx + sxd * g; y0 = cy + syd * g
            pygame.draw.line(screen, (245, 236, 210), (x0, y0), (x0 + sxd * 6, y0), 2)
            pygame.draw.line(screen, (245, 236, 210), (x0, y0), (x0, y0 + syd * 6), 2)
        pygame.draw.circle(screen, (245, 236, 210), (int(cx), int(cy)), 2)
        if self.pad["painting"] and self.pad["paint_r"] > 4:
            wx, wy = self.s2w(cx, cy)
            self.disc(screen, (240, 230, 120), wx, wy,
                      self.pad["paint_r"] / self.zoom, 2)

    def _on_key(self, ev):
        k = ev.key
        # end screen: N advances a tournament to its next round
        if self.done and k == pygame.K_n and self.tournament is not None:
            self._tournament_next()
            return
        # ── command palette: typing goes to it while open; "/" or Ctrl+P opens ──
        if self.pal is not None:
            self._pal_key(ev)
            return
        if k == pygame.K_SLASH or (k == pygame.K_p and
                                   pygame.key.get_mods() & pygame.KMOD_CTRL):
            self._pal_open()
            return
        # control groups: Ctrl+N assigns, Shift+N adds the selection to the
        # group, N selects, and a quick double-tap of N centres the camera on
        # the group (classic RTS muscle memory).
        if k in range(pygame.K_1, pygame.K_9 + 1):
            n = k - pygame.K_0
            mods = pygame.key.get_mods()
            sel_units = [u for u in self.selected if isinstance(u, Unit) and u.alive]
            if mods & pygame.KMOD_CTRL:
                self.groups[n] = sel_units
                self.flash(f"Control group {n} set ({len(self.groups[n])})")
            elif mods & pygame.KMOD_SHIFT:
                g = self._group(n)
                for u in sel_units:
                    if u not in g:
                        g.append(u)
                self.flash(f"Added to group {n} ({len(g)})")
            else:
                g = self._group(n)
                self._select(list(g))
                last_n, last_t = self._grp_tap
                if last_n == n and self.match_t - last_t < 0.35 and g:
                    cx = sum(u.x for u in g) / len(g)
                    cy = sum(u.y for u in g) / len(g)
                    self._centre_on(cx, cy)
                self._grp_tap = (n, self.match_t)
            return
        # command hotkeys act on the current selection / palette
        ch = getattr(ev, "unicode", "").lower()
        if k == pygame.K_TAB:
            self.show_tech = not self.show_tech
            return
        if k == pygame.K_f:
            self.fog_on = not self.fog_on
            if self.fog_on and self.fog: self._update_fog(force=True)
            self.flash(f"Fog of war {'on' if self.fog_on else 'off'}")
            return
        if self.cast_mode:
            if k == pygame.K_ESCAPE:
                self.cast_mode = None
            return
        if self.build_mode:
            if k == pygame.K_ESCAPE:
                self.build_mode = None
            return
        if k == pygame.K_ESCAPE:
            if self.selected:
                # first ESC clears the selection…
                self.selected = []
                self._resync_selected()
            else:
                # …a second ESC (nothing selected, no pending mode) asks the
                # host to leave the RTS scene and return to the menu.
                self.exit_requested = True
            return
        elif ch == "s":                     # stop
            for u in self._sel_units():
                u.goal = None; u.target = None; u.attack_move = False; u.state = "idle"
                u.stance = "aggressive"; u.patrol = None
        elif ch == "h":                     # hold position
            for u in self._sel_units():
                u.goal = None; u.state = "idle"; u.stance = "hold"; u.patrol = None
            if self._sel_units(): self.flash("Holding position")
        elif ch == "b":                     # burrow: stationary cloak
            sel = [u for u in self._sel_units() if not u.worker]
            for u in sel:
                if getattr(u, "burrowed", False):
                    u.burrowed = False; u.cloak = u._base_cloak
                else:
                    u._base_cloak = getattr(u, "cloak", False)
                    u.burrowed = True; u.cloak = True
                    u.goal = None; u.target = None; u.state = "idle"
            if sel:
                self.flash("Burrowed" if getattr(sel[0], "burrowed", False) else "Unburrowed")
        else:
            self._palette_hotkey(ch)

    def _palette_hotkey(self, ch):
        # if a production structure is selected, its letters train units
        for s in self._sel_structs():
            for uk in STRUCTURES[s.kind]["makes"]:
                if UNITS[uk]["key"].lower() == ch:
                    self._queue_train(s, uk); return
        # if a scribe is selected, letters open build placement
        if any(u.worker for u in self._sel_units()):
            for sk, spec in STRUCTURES.items():
                if spec["cost"] > 0 and spec["key"].lower() == ch:
                    self.build_mode = sk
                    self.flash(f"Placing {spec['name']} — click to build, Esc to cancel.")
                    return

    def _on_mouse_down(self, ev):
        mx, my = ev.pos
        self.mouse_pos = ev.pos
        # if a radial wheel is open, a click selects the slice under it
        if self.wheel:
            self._wheel_click(ev.pos); return
        # middle button starts a grab-pan of the map
        if ev.button == 2:
            self.mid_drag = (mx, my, self.cam_x, self.cam_y); return
        # spectator mode: camera only — no selecting or commanding either army,
        # but the minimap still jumps the view
        if getattr(self, "spectate", False):
            if self.mm_rect and self.mm_rect.collidepoint(mx, my):
                self._minimap_jump(mx, my)
            return
        # command palette: a click anywhere just closes it
        if self.pal is not None:
            self.pal = None; return
        # quick-command buttons (Select Army, etc.) and group badges
        for rect, action in self.buttons:
            if rect.collidepoint(mx, my):
                if isinstance(action, tuple) and action[0] == "group":
                    self._script_op({"op": "group", "n": action[1], "do": "select"})
                else:
                    self._quick_action(action[1] if isinstance(action, tuple) else action)
                return
        # minimap click → jump camera
        if self.mm_rect and self.mm_rect.collidepoint(mx, my):
            self._minimap_jump(mx, my); return
        # palette click?
        for rect, action in self.palette_rects:
            if rect.collidepoint(mx, my):
                self._palette_action(action); return
        if ev.button == 1:                  # left: select / place / cast
            if self.cast_mode:
                self._do_cast(*self.s2w(mx, my)); return
            if self.build_mode:
                self._try_build(*self.s2w(mx, my)); return
            self.box = (mx, my, mx, my)
        elif ev.button == 3:                # right: command
            self._command(*self.s2w(mx, my))

    def _on_mouse_up(self, ev):
        if ev.button == 2:
            self.mid_drag = None; return
        if ev.button == 1 and self.box:
            x0, y0, x1, y1 = self.box
            self.box = None
            if abs(x1 - x0) < 6 and abs(y1 - y0) < 6:
                self._click_select(x1, y1, pygame.key.get_mods() & pygame.KMOD_SHIFT)
            else:
                self._box_select(x0, y0, x1, y1)

    # ── touch / hybrid input ─────────────────────────────────────────────────
    # The touch model blends StarCraft (box-select, groups, minimap) with Halo
    # Wars (radial command wheel, one-tap "select army"): one finger taps to
    # select or drags a box; two fingers pan & pinch-zoom the big map; a held
    # finger opens a radial command wheel where you'd right-click on desktop.
    def _finger_pos(self, ev):
        if hasattr(ev, "pos"):
            return ev.pos
        return (ev.x * self.W, ev.y * self.H)

    def _on_finger_down(self, ev):
        fid = getattr(ev, "finger_id", getattr(ev, "fingerId", id(ev)))
        x, y = self._finger_pos(ev)
        self.fingers[fid] = dict(x=x, y=y, x0=x, y0=y, t=0.0, moved=False)
        if self.wheel:
            self._wheel_click((x, y)); return
        if len(self.fingers) == 2:
            self.box = None
            pts = list(self.fingers.values())
            cx = (pts[0]["x"] + pts[1]["x"]) / 2; cy = (pts[0]["y"] + pts[1]["y"]) / 2
            d = math.hypot(pts[0]["x"] - pts[1]["x"], pts[0]["y"] - pts[1]["y"])
            self.pinch0 = (max(1.0, d), self.zoom)
            self.pan_touch = (cx, cy, self.cam_x, self.cam_y)
        elif len(self.fingers) == 1:
            for rect, action in self.buttons:
                if rect.collidepoint(x, y):
                    if isinstance(action, tuple) and action[0] == "group":
                        self._script_op({"op": "group", "n": action[1], "do": "select"})
                    else:
                        self._quick_action(action[1] if isinstance(action, tuple) else action)
                    self.fingers.pop(fid, None); return
            for rect, action in self.palette_rects:
                if rect.collidepoint(x, y):
                    self._palette_action(action); self.fingers.pop(fid, None); return
            if self.mm_rect and self.mm_rect.collidepoint(x, y):
                self._minimap_jump(x, y); self.fingers.pop(fid, None); return
            if self.build_mode:
                self._try_build(*self.s2w(x, y)); self.fingers.pop(fid, None); return
            self.box = (x, y, x, y)

    def _on_finger_motion(self, ev):
        fid = getattr(ev, "finger_id", getattr(ev, "fingerId", id(ev)))
        f = self.fingers.get(fid)
        if not f:
            return
        x, y = self._finger_pos(ev)
        if abs(x - f["x0"]) + abs(y - f["y0"]) > 12:
            f["moved"] = True
        f["x"], f["y"] = x, y
        if len(self.fingers) == 2 and self.pinch0:
            pts = list(self.fingers.values())
            cx = (pts[0]["x"] + pts[1]["x"]) / 2; cy = (pts[0]["y"] + pts[1]["y"]) / 2
            d = math.hypot(pts[0]["x"] - pts[1]["x"], pts[0]["y"] - pts[1]["y"])
            d0, z0 = self.pinch0
            self.zoom = max(ZOOM_MIN, min(ZOOM_MAX, z0 * (max(1.0, d) / d0)))
            if self.pan_touch:
                px, py, cx0, cy0 = self.pan_touch
                self.cam_x = cx0 - (cx - px) / self.zoom
                self.cam_y = cy0 - (cy - py) / self.zoom
            self._clamp_cam()
        elif len(self.fingers) == 1 and self.box and f["moved"]:
            self.box = (self.box[0], self.box[1], x, y)

    def _on_finger_up(self, ev):
        fid = getattr(ev, "finger_id", getattr(ev, "fingerId", id(ev)))
        f = self.fingers.pop(fid, None)
        if self.wheel or f is None:
            return
        if len(self.fingers) == 0:
            self.pinch0 = None; self.pan_touch = None
            if self.box:
                x0, y0, x1, y1 = self.box
                self.box = None
                if not f["moved"]:
                    self._click_select(x1, y1, False)
                else:
                    self._box_select(x0, y0, x1, y1)

    def _touch_tick(self, dt):
        if len(self.fingers) == 1 and not self.wheel and self.build_mode is None:
            f = next(iter(self.fingers.values()))
            if not f["moved"]:
                f["t"] += dt
                if f["t"] >= LONGPRESS_T:
                    self.box = None
                    self._open_wheel(f["x"], f["y"])

    # ── radial command wheel (Halo Wars style) ───────────────────────────────
    def _open_wheel(self, sx, sy):
        slices = []
        us = self._sel_units(); ss = self._sel_structs()
        if any(u.worker for u in us):
            for sk, sp in STRUCTURES.items():
                if sp["cost"] > 0:
                    slices.append((sp["name"], ("build", sk)))
            slices.append(("Stop", ("cmd", "stop")))
        elif ss:
            s = ss[0]
            for uk in STRUCTURES[s.kind]["makes"]:
                slices.append((UNITS[uk]["name"], ("train", uk)))
            slices.append(("Rally", ("cmd", "rally_here")))
        elif us:
            slices = [("Move", ("cmd", "move_here")), ("Attack", ("cmd", "attack_here")),
                      ("Stop", ("cmd", "stop")), ("Hold", ("cmd", "hold"))]
        else:
            slices = [("Select Army", ("quick", "army")), ("Center Base", ("quick", "base"))]
        wx, wy = self.s2w(sx, sy)
        self.wheel = dict(x=sx, y=sy, slices=slices, wx=wx, wy=wy)
        self.wheel_hot = None

    def _wheel_slice_at(self, pt):
        if not self.wheel:
            return None
        dx = pt[0] - self.wheel["x"]; dy = pt[1] - self.wheel["y"]
        if math.hypot(dx, dy) < 26:
            return None
        n = len(self.wheel["slices"])
        if n == 0:
            return None
        ang = (math.atan2(dy, dx) + math.tau) % math.tau
        return int(ang / math.tau * n) % n

    def _wheel_click(self, pt):
        idx = self._wheel_slice_at(pt)
        w = self.wheel
        self.wheel = None; self.wheel_hot = None
        if idx is None or not w:
            return
        label, action = w["slices"][idx]
        kind, arg = action
        wx, wy = w["wx"], w["wy"]
        if kind == "build":
            self.build_mode = arg
            self.flash(f"Placing {STRUCTURES[arg]['name']} — tap to build.")
        elif kind == "train":
            for s in self._sel_structs():
                self._queue_train(s, arg)
        elif kind == "quick":
            self._quick_action(arg)
        elif kind == "cmd":
            if arg == "move_here":
                for u in self._sel_units(): u.goal = (wx, wy); u.target = None; u.attack_move = False; u.state = "move"
            elif arg == "attack_here":
                self._command_attack_move(wx, wy)
            elif arg == "stop":
                for u in self._sel_units(): u.goal = None; u.target = None; u.state = "idle"
            elif arg == "hold":
                for u in self._sel_units(): u.goal = None; u.state = "idle"
            elif arg == "rally_here":
                for s in self._sel_structs(): s.rally = (wx, wy)

    # ── quick commands (Halo Wars' one-touch army control) ───────────────────
    def _quick_action(self, arg):
        if arg == "army":
            army = [u for u in self.units if u.team == 0 and not u.worker and u.alive]
            self._select(army)
            self.flash(f"Selected army ({len(army)})")
        elif arg == "workers":
            self._select([u for u in self.units if u.team == 0 and u.worker and u.alive])
        elif arg == "idle":
            idle = [u for u in self.units if u.team == 0 and u.worker and u.alive
                    and u.state == "idle" and u.goal is None and u.target is None
                    and u.carrying == 0]
            if idle:
                self._select(idle)
                self.cam_x = idle[0].x - self.W / (2 * self.zoom)
                self.cam_y = idle[0].y - (self.H - 118) / (2 * self.zoom)
                self._clamp_cam()
                self.flash(f"Idle Scribes: {len(idle)}")
            else:
                self.flash("No idle Scribes.")
        elif arg == "base":
            hall = next((s for s in self.structs if s.team == 0 and s.kind == "scriptorium"), None)
            if hall:
                self.cam_x = hall.x - self.W / (2 * self.zoom)
                self.cam_y = hall.y - self.H / (2 * self.zoom)
                self._clamp_cam()

    # ── selection ──
    def _click_select(self, sx, sy, add):
        wx, wy = self.s2w(sx, sy)
        hit = None
        for u in self.units:
            if u.alive and u.team == 0 and math.hypot(u.x - wx, u.y - wy) < u.r + 6:
                hit = u; break
        if not hit:
            for s in self.structs:
                if s.alive and s.team == 0 and math.hypot(s.x - wx, s.y - wy) < s.r:
                    hit = s; break
        if hit:
            if add:
                if hit not in self.selected: self.selected.append(hit)
            else:
                self._select([hit])
        elif not add:
            self._select([])

    def _box_select(self, x0, y0, x1, y1):
        lo = (min(x0, x1), min(y0, y1)); hi = (max(x0, x1), max(y0, y1))
        picked = []
        for u in self.units:
            if u.alive and u.team == 0:
                sx, sy = self.w2s(u.x, u.y)
                if lo[0] <= sx <= hi[0] and lo[1] <= sy <= hi[1]:
                    picked.append(u)
        if picked:
            self._select(picked)

    def _select(self, ents):
        self.selected = list(ents)
        self._resync_selected()

    def _resync_selected(self):
        sset = set(id(e) for e in self.selected)
        for u in self.units: u.selected = id(u) in sset
        for s in self.structs: s.selected = id(s) in sset

    def _sel_units(self):
        return [e for e in self.selected if isinstance(e, Unit) and e.alive]
    def _sel_structs(self):
        return [e for e in self.selected if isinstance(e, Structure) and e.alive]

    # ── commands ──
    def _command(self, wx, wy):
        units = self._sel_units()
        if not units:
            # rally point for a selected structure
            for s in self._sel_structs():
                s.rally = (wx, wy)
            return
        # patrol: an armed patrol command turns the selection into patrollers
        # sweeping between their current spot and the clicked point
        if getattr(self, "_patrol_arm", False):
            self._patrol_arm = False
            for u in units:
                u.stance = "patrol"; u.patrol_a = (u.x, u.y); u.patrol = (wx, wy)
                u.goal = (wx, wy); u.state = "move"; u.target = None
            self.flash("Patrolling")
            return
        # shift held → QUEUE this move as a waypoint instead of replacing orders
        queue = bool(pygame.key.get_mods() & pygame.KMOD_SHIFT)
        # right-clicked an enemy? attack it. an ink node with a worker? harvest.
        tgt = self._enemy_at(wx, wy)
        node = self._node_at(wx, wy)
        for u in units:
            if tgt is not None:
                u.target = tgt; u.attack_move = False; u.goal = None; u.state = "attack"
                if not queue: u.waypoints = []
            elif node is not None and u.worker:
                u.harvest_node = node; u.target = None; u.state = "toNode"; u.goal = (node.x, node.y)
                if not queue: u.waypoints = []
            else:
                if queue and (u.goal is not None or u.waypoints):
                    u.waypoints.append((wx, wy))     # chain onto existing orders
                else:
                    u.goal = (wx, wy); u.target = None; u.attack_move = False
                    u.state = "move"; u.waypoints = []
                    if u.stance != "aggressive":     # a move order clears hold/patrol
                        u.stance = "aggressive"; u.patrol = None

    def _command_attack_move(self, wx, wy):
        for u in self._sel_units():
            u.goal = (wx, wy); u.attack_move = True; u.target = None; u.state = "move"

    def _enemy_at(self, wx, wy):
        for u in self.units:
            if u.alive and u.team != 0 and math.hypot(u.x - wx, u.y - wy) < u.r + 8:
                return u
        for s in self.structs:
            if s.alive and s.team != 0 and math.hypot(s.x - wx, s.y - wy) < s.r:
                return s
        # destructible rocks are neutral targets you can order attacks on
        for o in self.rocks:
            if o.alive and math.hypot(o.x - wx, o.y - wy) < o.r + 6:
                return o
        return None

    def _node_at(self, wx, wy):
        for n in self.nodes:
            if n.amount > 0 and math.hypot(n.x - wx, n.y - wy) < n.r + 10:
                return n
        return None

    # ── production & building ──
    def _queue_train(self, s, uk):
        spec = self.uspec(s.team, uk)
        need = spec.get("requires_research")
        if need and need not in self.upgrades[s.team]:
            if s.team == 0:
                upname = self.facdata(0)["upgrades"].get(need, {}).get("name", need)
                self.flash(f"Requires research: {upname}.")
            return
        if not self._supply_free(s.team, uk):
            if s.team == 0: self.flash("Supply blocked — raise an Ink Reservoir.")
            return
        if self.ink[s.team] >= spec["cost"]:
            self.ink[s.team] -= spec["cost"]
            s.queue.append(uk)
            if s.team == 0: self.flash(f"Training {spec['name']}…")
        elif s.team == 0:
            self.flash("Not enough ink.")

    def _try_build(self, wx, wy):
        sk = self.build_mode
        spec = self.sspec(0, sk)
        if not self._struct_available(0, sk):
            need = ", ".join(self.sspec(0, r)["name"] for r in spec.get("requires", []))
            self.flash(f"Requires {need} first."); self.build_mode = None; return
        if self.ink[0] < spec["cost"]:
            self.flash("Not enough ink."); self.build_mode = None; return
        if self._placement_blocked(wx, wy, spec["r"]):
            self.flash("Can't build there."); return
        workers = [u for u in self._sel_units() if u.worker]
        if not workers:
            self.flash("Select a Scribe to build."); self.build_mode = None; return
        self.ink[0] -= spec["cost"]
        st = Structure(sk, wx, wy, 0, complete=False, spec=spec)
        self.structs.append(st); self.stats[0]["structs"] += 1
        w = min(workers, key=lambda u: math.hypot(u.x - wx, u.y - wy))
        w.goal = (wx, wy); w.state = "move"
        self.build_mode = None
        self._sfx("deploy", 0.7)
        self.flash(f"Raising {spec['name']}…")

    def _placement_blocked(self, wx, wy, r):
        if wx < r or wy < r or wx > WORLD_W - r or wy > WORLD_H - r:
            return True
        for s in self.structs:
            if math.hypot(s.x - wx, s.y - wy) < s.r + r + 8:
                return True
        for n in self.nodes:
            if n.amount > 0 and math.hypot(n.x - wx, n.y - wy) < n.r + r:
                return True
        for o in self.objects:               # ruins & obstacles block building
            if o.blocks and math.hypot(o.x - wx, o.y - wy) < o.r + r:
                return True
        return False

    def _start_cast(self, ability, casters):
        ready = [c for c in casters if getattr(c, "cast_cd", 0) <= 0]
        if not ready:
            self.flash("Ability on cooldown."); return
        able = [c for c in ready if c.energy >= ability["cost"]]
        if not able:
            self.flash("Not enough energy."); return
        self.cast_mode = (ability, able)
        self.build_mode = None
        self.flash(f"{ability['name']} — click a target, Esc to cancel.")

    def _do_cast(self, wx, wy):
        ability, casters = self.cast_mode
        self.cast_mode = None
        caster = min(casters, key=lambda c: math.hypot(c.x - wx, c.y - wy))
        if caster.energy < ability["cost"] or not caster.alive:
            return
        dx, dy = wx - caster.x, wy - caster.y
        dd = math.hypot(dx, dy); rng = ability["cast_range"]
        if dd > rng:
            wx = caster.x + dx / dd * rng; wy = caster.y + dy / dd * rng
        caster.energy -= ability["cost"]
        caster.cast_cd = ability.get("cooldown", 0)      # start the ability cooldown
        self._cast_ability(ability, wx, wy, caster)
        self._sfx(ability.get("sfx") or ("thunder" if ability.get("kind") == "storm" else "whoosh"), 0.6)
        if caster.team == 0:
            self.flash(f"{ability['name']} cast!")

    def _cast_ability(self, ability, wx, wy, caster):
        """Deliver an ability by its mode: an instant AREA burst, a travelling
        BOLT that detonates on arrival, or a CHAIN that bounces between foes."""
        delivery = ability.get("delivery", "area")
        if delivery == "bolt":
            self.spell_bolts.append(dict(
                x=caster.x, y=caster.y - caster.r * 0.4, tx=wx, ty=wy, t=0.0,
                dur=max(0.1, math.hypot(wx - caster.x, wy - caster.y) / 900),
                ab=ability, team=caster.team, caster=caster))
        elif delivery == "chain":
            hops = self._chain_points(wx, wy, caster.team,
                                      int(ability.get("chain_count", 3)),
                                      ability.get("chain_range", 240))
            for i, (hx, hy) in enumerate(hops):
                self._spawn_area_spell(ability, hx, hy, caster,
                                       delay=i * 0.12, radius=90)
        else:
            self._spawn_area_spell(ability, wx, wy, caster)

    def _spawn_area_spell(self, ability, wx, wy, caster, delay=0.0, radius=None):
        ab = ability if radius is None else {**ability, "radius": radius}
        self.spells.append(dict(kind=ability.get("kind", "custom"), x=wx, y=wy, t=0.0,
                                dur=ability["dur"] + delay, delay=delay,
                                team=caster.team, ab=ab,
                                effects=_ability_effects(ability), _done=set(),
                                caster=caster))

    def _chain_points(self, wx, wy, team, count, rng):
        """Nearest enemy to the point, then bounce to the nearest un-hit enemy
        within range, up to `count` targets. Returns a list of (x,y) hop points."""
        cand = [u for u in self.units if u.alive and u.team != team]
        if not cand:
            return [(wx, wy)]
        chain = [min(cand, key=lambda u: math.hypot(u.x - wx, u.y - wy))]
        for _ in range(max(0, count - 1)):
            last = chain[-1]; nxt = None; best = rng
            for u in cand:
                if u in chain:
                    continue
                d = math.hypot(u.x - last.x, u.y - last.y)
                if d < best:
                    best = d; nxt = u
            if not nxt:
                break
            chain.append(nxt)
        return [(u.x, u.y) for u in chain]

    def _on_high(self, x, y):
        """Is this point on a high-ground plateau?"""
        for hx, hy, hr in self._high:
            if (x - hx) ** 2 + (y - hy) ** 2 < hr * hr:
                return True
        return False

    def _detected(self, u):
        """Is this cloaked unit revealed to its ENEMY? True if any enemy detector
        (Watch Spire) is within its detect range. Non-cloaked units are always
        visible."""
        if not getattr(u, "cloak", False):
            return True
        for s in self.structs:
            if s.team != u.team and s.complete and self.sspec(s.team, s.kind).get("detector"):
                if math.hypot(s.x - u.x, s.y - u.y) < self.sspec(s.team, s.kind).get("detect_range", 300):
                    return True
        return False

    # ── living map: fauna + ink surges ──────────────────────────────────────
    def _tick_life(self, dt):
        """Advance ambient fauna and the occasional ink surge. All cosmetic
        except the surge's small +ink on a node — a reason to watch the map."""
        tnow = getattr(self, "_anim_t", 0.0)
        for f in self.fauna:
            k = f["kind"]
            if k == "birds":
                f["x"] += f["vx"] * dt; f["y"] += f["vy"] * dt
                # wrap the flock around the page with margin
                if f["x"] < -160: f["x"] = WORLD_W + 150
                if f["x"] > WORLD_W + 160: f["x"] = -150
                if f["y"] < -160: f["y"] = WORLD_H + 150
                if f["y"] > WORLD_H + 160: f["y"] = -150
            elif k == "beetle":
                f["rest"] -= dt
                dx, dy = f["tx"] - f["x"], f["ty"] - f["y"]
                d = math.hypot(dx, dy)
                if d > 3:                      # darting to the next hidey spot
                    sp = 120 * dt
                    f["x"] += dx / d * sp; f["y"] += dy / d * sp
                elif f["rest"] <= 0:           # pick a new dash
                    f["tx"] = min(WORLD_W - 100, max(100, f["x"] + random.uniform(-260, 260)))
                    f["ty"] = min(WORLD_H - 100, max(100, f["y"] + random.uniform(-260, 260)))
                    f["rest"] = random.uniform(1.5, 5.0)
            # fish advance their orbit lazily in the drawer (pure function of t)
        # ── ink surges: a random living well periodically boils over ──
        if self.surge is None:
            self.surge_timer -= dt
            if self.surge_timer <= 0:
                live = [n for n in self.nodes if n.amount > 0]
                if live:
                    n = random.choice(live)
                    n.amount += 220           # the well runs rich for a while
                    self.surge = dict(node=n, t=0.0, dur=6.0)
                    if self._cell_state(n.x, n.y) != 0 or not self.fog_on:
                        self.flash("An ink well surges…")
                self.surge_timer = random.uniform(22, 40)
        else:
            self.surge["t"] += dt
            n = self.surge["node"]
            if self.surge["t"] < self.surge["dur"] and random.random() < 12 * dt:
                self._emit(n.x, n.y, 3, "ink", (140, 124, 210),
                           spread=int(n.r), life=0.7, up=70)
            if self.surge["t"] >= self.surge["dur"]:
                self.surge = None

    def _draw_fauna(self, screen):
        if self.zoom < 0.45:
            return
        tnow = getattr(self, "_anim_t", 0.0)
        for f in self.fauna:
            k = f["kind"]
            if k == "birds":
                for (ox, oy, ph) in f["offs"]:
                    bx, by = f["x"] + ox, f["y"] + oy
                    if self.fog_on and self._cell_state(bx, by) == 0:
                        continue
                    sx, sy = self.w2s(bx, by)
                    if not (-20 < sx < self.W + 20 and -20 < sy < self.H - 100):
                        continue
                    sy -= 34 * self.zoom          # they fly above the page
                    flap = math.sin(tnow * 9 + ph)
                    w = max(3, int(6 * self.zoom)); h = int(w * 0.6 * (0.4 + abs(flap)))
                    hd = 1 if f["vx"] >= 0 else -1
                    pygame.draw.lines(screen, (246, 242, 230), False,
                        [(sx - w * hd, sy - h * flap), (sx, sy), (sx + w * 0.6 * hd, sy - h * flap * 0.8)], 2)
                    # a faint travelling shadow sells the altitude
                    gsx, gsy = self.w2s(bx, by)
                    pygame.draw.ellipse(screen, (0, 0, 0),
                        (int(gsx - 3), int(gsy - 1), 6, 2), 1)
            elif k == "fish":
                px, py, pr = f["pool"]
                if self.fog_on and self._cell_state(px, py) == 0:
                    continue
                a = f["a"] + tnow * f["spd"]
                fx = px + math.cos(a) * f["r"]
                fy = py + math.sin(a) * f["r"] * 0.8
                sx, sy = self.w2s(fx, fy)
                if not (-20 < sx < self.W + 20 and -20 < sy < self.H - 100):
                    continue
                # a fin breaking the ink + an expanding ripple every few beats
                fin = math.sin(tnow * 2.0 + f["ph"])
                if fin > 0.2:
                    ln = max(2, int(5 * self.zoom * fin))
                    pygame.draw.line(screen, (150, 140, 205),
                                     (sx, sy - ln), (sx + ln, sy), 2)
                rp = (tnow * 0.6 + f["ph"]) % 1.0
                rr = int(pr * 0.25 * rp * self.zoom)
                if rr > 2:
                    self.disc(screen, (110, 100, 168), fx, fy, pr * 0.25 * rp, 1)
            elif k == "beetle":
                if self.fog_on and self._cell_state(f["x"], f["y"]) == 0:
                    continue
                sx, sy = self.wpos(f["x"], f["y"])
                if not (-10 < sx < self.W + 10 and -10 < sy < self.H - 100):
                    continue
                moving = math.hypot(f["tx"] - f["x"], f["ty"] - f["y"]) > 3
                jitter = math.sin(tnow * 22 + f["x"]) if moving else 0
                pygame.draw.ellipse(screen, (74, 62, 48),
                    (int(sx - 3), int(sy - 2 + jitter), 6, 4))
                pygame.draw.line(screen, (120, 104, 82),
                                 (int(sx - 2), int(sy)), (int(sx + 2), int(sy)), 1)
        # ── surge halo + relic pulse (pure functions of time) ──
        if self.surge is not None:
            n = self.surge["node"]
            ph = (self.surge["t"] * 1.4) % 1.0
            self.disc(screen, (150, 136, 220), n.x, n.y, n.r * (1.0 + ph * 1.6),
                      max(1, int(3 * (1 - ph))))
        for o in self.objects:
            if o.kind == "relic":
                ph = (tnow * 0.5 + o.x * 0.001) % 1.0
                if not self.fog_on or self._cell_state(o.x, o.y) != 0:
                    self.disc(screen, (214, 186, 120), o.x, o.y,
                              o.r * (0.8 + ph * 1.1), max(1, int(2 * (1 - ph))))

    def _draw_pools(self, screen):
        """Rippling ink pools — dark water regions with animated concentric
        ripples and a drifting sheen. Authored per map as terrain features."""
        if not self.pools:
            return
        tnow = getattr(self, "_anim_t", 0.0)
        for (px, py, pr) in self.pools:
            sx, sy = self.w2s(px, py)
            rr = int(pr * self.zoom)
            if rr < 3 or not (-rr < sx < self.W + rr and -rr < sy < self.H + rr):
                continue
            if self.fog_on and self._cell_state(px, py) == 0:
                continue
            pool = pygame.Surface((rr * 2 + 4, rr * 2 + 4), pygame.SRCALPHA)
            pygame.draw.circle(pool, (34, 30, 54, 210), (rr + 2, rr + 2), rr)
            pygame.draw.circle(pool, (58, 52, 92, 230), (rr + 2, rr + 2), rr, 2)
            for k in range(3):
                ph = (tnow * 0.5 + k * 0.33) % 1.0
                rk = int(rr * (0.2 + ph * 0.8))
                a = int(80 * (1 - ph))
                if rk > 1:
                    pygame.draw.circle(pool, (110, 100, 170, a), (rr + 2, rr + 2), rk, 1)
            hx = rr + 2 + math.cos(tnow * 0.7) * rr * 0.3
            hy = rr + 2 + math.sin(tnow * 0.6) * rr * 0.25
            pygame.draw.circle(pool, (150, 140, 210, 90), (int(hx), int(hy)), max(2, rr // 4))
            # foreshorten the composed pool to the page's viewing angle
            ph_ = max(2, int((rr * 2 + 4) * ISO))
            pool = pygame.transform.smoothscale(pool, (rr * 2 + 4, ph_))
            screen.blit(pool, (int(sx - rr - 2), int(sy - ph_ // 2)))

    def _draw_props(self, screen):
        """Themed decorative biome props — folded paper leaves, boats, cranes,
        reeds, ink blots, torn scraps — scattered per map. Purely visual, hidden
        under fog, gently animated."""
        if self.zoom < 0.4:
            return
        tnow = getattr(self, "_anim_t", 0.0)
        acc = tuple(self.theme.get("accent", [150, 120, 80]))
        dark = tuple(int(c * 0.7) for c in acc)
        for p in self.props:
            wx, wy = p["x"], p["y"]
            if self.fog_on and self._cell_state(wx, wy) == 0:
                continue
            sx, sy = self.wpos(wx, wy)
            rr = p["r"] * self.zoom
            if not (-30 < sx < self.W + 30 and -30 < sy < self.H - 88):
                continue
            style = p.get("style", "blot"); rot = p.get("rot", 0.0)
            bob = math.sin(tnow * 1.2 + p["x"] * 0.01) * rr * 0.08
            sy += bob
            if style == "leaf":
                # a folded leaf: two angled paper halves
                for sgn in (-1, 1):
                    a = rot + sgn * 0.5
                    tip = (sx + math.cos(a) * rr, sy + math.sin(a) * rr)
                    pygame.draw.polygon(screen, acc, [(sx, sy), tip,
                        (sx + math.cos(a + sgn * 0.4) * rr * 0.5, sy + math.sin(a + sgn * 0.4) * rr * 0.5)])
                pygame.draw.line(screen, dark, (sx, sy),
                                 (sx + math.cos(rot) * rr, sy + math.sin(rot) * rr), 1)
            elif style == "boat":
                # a little folded paper boat (hull + sail)
                hull = [(sx - rr, sy), (sx + rr, sy), (sx + rr * 0.6, sy + rr * 0.5),
                        (sx - rr * 0.6, sy + rr * 0.5)]
                pygame.draw.polygon(screen, acc, hull); pygame.draw.polygon(screen, dark, hull, 1)
                pygame.draw.polygon(screen, tuple(min(255, c + 30) for c in acc),
                                    [(sx, sy), (sx, sy - rr), (sx + rr * 0.7, sy)])
            elif style == "crane":
                # a folded crane silhouette: body diamond + wing + beak
                pygame.draw.polygon(screen, acc, [(sx, sy - rr * 0.5), (sx + rr * 0.4, sy),
                                                  (sx, sy + rr * 0.5), (sx - rr * 0.4, sy)])
                pygame.draw.polygon(screen, tuple(min(255, c + 24) for c in acc),
                                    [(sx, sy), (sx + rr, sy - rr * 0.6), (sx + rr * 0.3, sy + rr * 0.2)])
                pygame.draw.line(screen, dark, (sx - rr * 0.3, sy), (sx - rr, sy - rr * 0.4), 1)
            elif style == "reed":
                # marsh: a lily pad and a few tall reeds
                pad = pygame.Surface((int(rr * 2), int(rr)), pygame.SRCALPHA)
                pygame.draw.ellipse(pad, (*acc, 150), (0, 0, int(rr * 2), int(rr)))
                screen.blit(pad, (int(sx - rr), int(sy)))
                for k in range(3):
                    rx = sx + (k - 1) * rr * 0.5
                    sway = math.sin(tnow * 1.6 + k + p["x"]) * rr * 0.25
                    pygame.draw.line(screen, dark, (rx, sy), (rx + sway, sy - rr * 1.6), 2)
            elif style == "scrap":
                # torn newsprint scrap: a ragged quad with a couple of "text" ticks
                pts = [(sx - rr, sy - rr * 0.6), (sx + rr * 0.8, sy - rr),
                       (sx + rr, sy + rr * 0.7), (sx - rr * 0.7, sy + rr * 0.5)]
                pygame.draw.polygon(screen, tuple(min(255, c + 20) for c in acc), pts)
                pygame.draw.polygon(screen, dark, pts, 1)
                for ty in (-0.2, 0.2):
                    pygame.draw.line(screen, dark, (sx - rr * 0.5, sy + rr * ty),
                                     (sx + rr * 0.4, sy + rr * ty), 1)
            else:  # blot
                pygame.draw.circle(screen, dark, (int(sx), int(sy)), int(rr * 0.7))
                for k in range(3):
                    a = rot + k * 2.1
                    bx = sx + math.cos(a) * rr * 0.7; by = sy + math.sin(a) * rr * 0.7
                    pygame.draw.circle(screen, dark, (int(bx), int(by)), max(1, int(rr * 0.28)))

    def _draw_motes(self, screen):
        """Faint paper motes drifting across the sheet — pure ambience, driven by
        time so they cost nothing in the sim. Only on-screen ones are drawn."""
        if self.zoom < 0.4:
            return
        tnow = getattr(self, "_anim_t", 0.0)
        acc = tuple(self.theme.get("accent", [150, 120, 80]))
        for m in self.motes:
            wx = m["x"] + math.cos(m["ang"] + tnow * 0.1) * 40 + tnow * m["drift"] * 0.3
            wy = m["y"] + math.sin(m["ang"] + tnow * 0.13) * 40
            wx %= WORLD_W; wy %= WORLD_H
            sx, sy = self.w2s(wx, wy)
            if not (0 < sx < self.W and 0 < sy < self.H - 118):
                continue
            if self.fog_on and self._cell_state(wx, wy) == 0:
                continue
            tw = 0.4 + 0.6 * (0.5 + 0.5 * math.sin(tnow * 1.5 + m["ph"]))
            sz = max(1, int(m["sz"] * self.zoom))
            mo = pygame.Surface((sz * 2, sz * 2), pygame.SRCALPHA)
            mo.fill((*acc, int(90 * tw)))
            screen.blit(mo, (int(sx - sz), int(sy - sz)))

    def _emit(self, x, y, n, kind, col, spread=40, life=0.6, up=0.0):
        if len(self.particles) > 420:
            return
        for _ in range(n):
            a = random.uniform(0, math.tau)
            sp = random.uniform(0.2, 1.0) * spread
            self.particles.append(dict(
                x=x, y=y, vx=math.cos(a) * sp, vy=math.sin(a) * sp - up,
                t=0.0, life=life * random.uniform(0.7, 1.2), kind=kind,
                col=col, sz=random.uniform(1.5, 3.5)))

    def _tick_particles(self, dt):
        for p in self.particles:
            p["t"] += dt
            p["x"] += p["vx"] * dt; p["y"] += p["vy"] * dt
            p["vy"] += 60 * dt
            p["vx"] *= 0.92; p["vy"] *= 0.96
        self.particles = [p for p in self.particles if p["t"] < p["life"]]
        for pr in self.projectiles:
            pr["t"] += dt
            if pr["t"] >= pr["dur"]:
                self._emit(pr["tx"], pr["ty"], 2, "scrap", (230, 220, 190),
                           spread=30, life=0.3, up=10)
        self.projectiles = [pr for pr in self.projectiles if pr["t"] < pr["dur"]]

    def _draw_spell_bolts(self, screen):
        """A glowing spell bolt in flight toward its detonation point."""
        for b in self.spell_bolts:
            f = b["t"] / b["dur"]
            x = b["x"] + (b["tx"] - b["x"]) * f
            y = b["y"] + (b["ty"] - b["y"]) * f - math.sin(f * math.pi) * 24
            sx, sy = self.w2s(x, y)
            if not (-12 < sx < self.W + 12 and -12 < sy < self.H + 12):
                continue
            r = max(3, int(6 * self.zoom))
            glow = pygame.Surface((r * 4, r * 4), pygame.SRCALPHA)
            pygame.draw.circle(glow, (210, 190, 130, 120), (r * 2, r * 2), r * 2)
            screen.blit(glow, (int(sx - r * 2), int(sy - r * 2)), special_flags=pygame.BLEND_ADD)
            pygame.draw.circle(screen, (245, 232, 200), (int(sx), int(sy)), r)
            pygame.draw.circle(screen, (200, 170, 110), (int(sx), int(sy)), r, 1)

    def _draw_projectiles(self, screen):
        for pr in self.projectiles:
            f = pr["t"] / pr["dur"]
            x = pr["x"] + (pr["tx"] - pr["x"]) * f
            y = pr["y"] + (pr["ty"] - pr["y"]) * f - math.sin(f * math.pi) * 18
            sx, sy = self.w2s(x, y)
            if not (-8 < sx < self.W + 8 and -8 < sy < self.H + 8):
                continue
            ang = math.atan2((pr["ty"] - pr["y"]), (pr["tx"] - pr["x"]))
            ln = max(4, int(7 * self.zoom))
            ex = sx - math.cos(ang) * ln; ey = sy - math.sin(ang) * ln
            pygame.draw.line(screen, pr["col"], (ex, ey), (sx, sy), max(1, int(2 * self.zoom)))
            pygame.draw.circle(screen, (240, 232, 210), (int(sx), int(sy)), max(1, int(self.zoom * 1.5)))

    def _draw_particles(self, screen):
        for p in self.particles:
            sx, sy = self.w2s(p["x"], p["y"])
            if not (-10 < sx < self.W + 10 and -10 < sy < self.H + 10):
                continue
            fade = max(0.0, 1.0 - p["t"] / p["life"])
            sz = max(1, int(p["sz"] * self.zoom * (0.6 + fade)))
            col = p["col"]
            if p["kind"] == "scrap":
                surf = pygame.Surface((sz * 2, sz * 2), pygame.SRCALPHA)
                surf.fill((*col, int(200 * fade)))
                surf = pygame.transform.rotate(surf, (p["t"] * 300 + p["x"]) % 360)
                screen.blit(surf, (int(sx - sz), int(sy - sz)))
            else:
                s2 = pygame.Surface((sz * 2, sz * 2), pygame.SRCALPHA)
                pygame.draw.circle(s2, (*col, int(220 * fade)), (sz, sz), sz)
                screen.blit(s2, (int(sx - sz), int(sy - sz)))

    def events_loaded_count(self):
        return len(getattr(self, "events", []))

    def _tick_events(self, dt):
        """Evaluate map event scripts authored in the editor. Each event fires
        once when its trigger becomes true, running its list of actions."""
        for e in self.events:
            if e.get("_fired"):
                continue
            if self._trigger_true(e.get("trigger", {})) and \
                    all(self._condition_true(c) for c in e.get("conditions", [])):
                e["_fired"] = True
                for a in e.get("actions", []):
                    self._run_action(a)

    def _condition_true(self, c):
        """Extra gates the SC2 Event→Conditions→Actions model uses."""
        t = c.get("type")
        if t == "ink_at_least":
            return self.ink[c.get("team", 0)] >= c.get("amount", 0)
        if t == "time_after":
            return self.match_t >= c.get("at", 0)
        if t == "time_before":
            return self.match_t <= c.get("at", 1e9)
        if t == "unit_count":
            team = c.get("team", 0); n = c.get("n", 0); op = c.get("op", ">=")
            cnt = sum(1 for u in self.units if u.alive and u.team == team)
            return (cnt >= n) if op == ">=" else (cnt <= n) if op == "<=" else (cnt == n)
        return True

    def _trigger_true(self, tr):
        t = tr.get("type")
        if t == "time":
            return self.match_t >= tr.get("at", 0)
        if t == "unit_count":
            team = tr.get("team", 0); n = tr.get("n", 0); op = tr.get("op", ">=")
            c = sum(1 for u in self.units if u.alive and u.team == team)
            return (c >= n) if op == ">=" else (c <= n) if op == "<=" else (c == n)
        if t == "enter_region":
            rx, ry, rr = tr.get("region", [0, 0, 200]); team = tr.get("team", 0)
            return any(u.alive and u.team == team and
                       math.hypot(u.x - rx, u.y - ry) < rr for u in self.units)
        if t == "structure_destroyed":
            team = tr.get("team", 1); kind = tr.get("kind", "scriptorium")
            return not any(s.team == team and s.kind == kind for s in self.structs)
        return False

    def _run_action(self, a):
        t = a.get("type")
        if t == "message":
            self.flash(a.get("text", "")); self._sfx("select", 0.4)
        elif t == "ink":
            team = a.get("team", 0)
            self.ink[team] = min(self.cap[team], self.ink[team] + a.get("amount", 0))
        elif t == "reveal":
            rx, ry, rr = a.get("x", 0), a.get("y", 0), a.get("r", 300)
            if self.fog:
                cell = self.fog_cell
                for gy in range(self.fog_rows):
                    for gx in range(self.fog_cols):
                        if math.hypot(gx * cell - rx, gy * cell - ry) < rr:
                            if self.fog[gy][gx] == 0:
                                self.fog[gy][gx] = 1
        elif t == "spawn":
            team = a.get("team", 1); uk = a.get("unit", "soldier")
            ux, uy = a.get("x", 0), a.get("y", 0); n = a.get("count", 1)
            spec = self.uspec(team, uk)
            if spec:
                for k in range(n):
                    off = k * 26
                    u = Unit(uk, ux + (off % 80), uy + (off // 80) * 26, team, spec=spec)
                    self._apply_upgrades(u)
                    self.units.append(u)

    def _tick_spells(self, dt):
        # travelling bolts: fly to their point, then detonate into an area spell
        for b in self.spell_bolts:
            b["t"] += dt
            if b["t"] >= b["dur"]:
                if b["caster"] is not None:
                    self._spawn_area_spell(b["ab"], b["tx"], b["ty"], b["caster"])
                self._emit(b["tx"], b["ty"], 6, "ink", (200, 180, 120), spread=40, life=0.4, up=20)
        self.spell_bolts = [b for b in self.spell_bolts if b["t"] < b["dur"]]
        for sp in self.spells:
            sp["t"] += dt
            if sp["t"] < sp.get("delay", 0):
                continue                      # staggered spell not started yet
            ab = sp["ab"]; rad = ab["radius"]
            effects = sp.get("effects") or _ability_effects(ab)
            done = sp.setdefault("_done", set())
            for ei, eff in enumerate(effects):
                # per-effect stagger, measured from the spell's own start
                if sp["t"] - sp.get("delay", 0) < eff.get("delay", 0):
                    continue
                self._run_effect(sp, eff, ei, rad, dt, done)
        self.spells = [sp for sp in self.spells if sp["t"] < sp["dur"]]

    def _spell_targets(self, sp, eff):
        """Select the units an effect applies to, honouring its target mode and
        any HP conditions (if_hp_below / if_hp_above, as 0..1 fractions)."""
        team = sp["team"]; sx, sy = sp["x"], sp["y"]; rad = sp["ab"]["radius"]
        mode = eff.get("target")
        caster = sp.get("caster")
        below = eff.get("if_hp_below"); above = eff.get("if_hp_above")
        def cond_ok(u):
            frac = u.hp / u.maxhp if u.maxhp else 1.0
            if below is not None and frac >= below: return False
            if above is not None and frac <= above: return False
            return True
        if mode == "self":
            return [caster] if caster and caster.alive and cond_ok(caster) else []
        if mode is None:
            mode = "ally" if eff.get("effect") in ("heal", "buff", "shield") else "enemy"
        out = []
        for u in self.units:
            if not u.alive or math.hypot(u.x - sx, u.y - sy) >= rad:
                continue
            if not cond_ok(u):
                continue
            if mode == "all" or (mode == "enemy" and u.team != team) or \
               (mode == "ally" and u.team == team):
                out.append(u)
        return out

    def _run_effect(self, sp, eff, ei, rad, dt, done):
        """Apply one composable effect primitive for an active spell."""
        kind = eff.get("effect", "damage")
        team = sp["team"]; sx, sy = sp["x"], sp["y"]; caster = sp.get("caster")
        once = ei not in done
        # a few themed particles per effect — a per-effect vfx theme overrides
        # the default colour so custom spells can look like fire, ice, holy, etc.
        if random.random() < 0.4:
            pa = random.uniform(0, math.tau); pr = random.uniform(0, rad) * 0.9
            px = sx + math.cos(pa) * pr; py = sy + math.sin(pa) * pr
            theme = eff.get("vfx")
            if theme in VFX_THEMES:
                col, up = VFX_THEMES[theme]
            else:
                col = {"damage": (250, 170, 60), "burn": (250, 130, 50),
                       "heal": (140, 230, 160), "slow": (90, 170, 220),
                       "stun": (230, 220, 120), "buff": (230, 210, 120),
                       "shield": (150, 200, 235), "shred": (220, 120, 120),
                       "drain": (190, 80, 160), "knockback": (220, 200, 170),
                       "pull": (170, 200, 220), "spawn": (200, 190, 170),
                       "reveal": (200, 210, 235)}.get(kind, (200, 200, 200))
                up = random.uniform(0, 30)
            self._emit(px, py, 1, "ink", col, spread=10, life=0.4, up=up)
        tgts = self._spell_targets(sp, eff)
        if kind == "damage":
            amt = eff.get("amount", 40)
            if eff.get("dot"):
                for u in tgts: self._hurt(u, amt * dt)
            elif once:
                done.add(ei)
                for u in tgts: self._hurt(u, amt)
        elif kind == "drain":
            amt = eff.get("amount", 40)
            per = amt * dt if eff.get("dot") else (amt if once else 0)
            if per and (eff.get("dot") or once):
                if not eff.get("dot"): done.add(ei)
                total = 0.0
                for u in tgts:
                    total += self._hurt(u, per)
                if caster and caster.alive:
                    caster.hp = min(caster.maxhp, caster.hp + total * 0.6)
        elif kind == "heal":
            amt = eff.get("amount", 80)
            if eff.get("dot"):
                per = amt / max(0.1, sp["dur"])
                for u in tgts:
                    if u.hp < u.maxhp: u.hp = min(u.maxhp, u.hp + per * dt)
            elif once:
                done.add(ei)
                for u in tgts: u.hp = min(u.maxhp, u.hp + amt)
        elif kind == "slow":
            for u in tgts:
                u.slow_t = max(getattr(u, "slow_t", 0), eff.get("duration", 3.0))
        elif kind == "stun":
            for u in tgts:
                u.stun_t = max(getattr(u, "stun_t", 0), eff.get("duration", 1.5))
        elif kind == "burn":
            for u in tgts:
                if isinstance(u, Unit):
                    u.burn = min(6.0, getattr(u, "burn", 0) + eff.get("duration", 3.0) * dt)
                    u.burn_dps = max(getattr(u, "burn_dps", 0), eff.get("dps", 20))
        elif kind == "shred":
            for u in tgts:
                u.shred = max(getattr(u, "shred", 1.0), eff.get("mult", 1.3))
                u.shred_t = max(getattr(u, "shred_t", 0), eff.get("duration", 4.0))
        elif kind == "buff":
            if once:
                done.add(ei)
                stat = eff.get("stat", "dmg"); mult = eff.get("mult", 1.3)
                dur = eff.get("duration", 5.0)
                for u in tgts:
                    if stat == "spd": u.buff_spd = mult; u.buff_spd_t = dur
                    else: u.buff_dmg = mult; u.buff_dmg_t = dur
        elif kind == "shield":
            if once:
                done.add(ei)
                amt = eff.get("amount", 100); dur = eff.get("duration", 4.0)
                for u in tgts:
                    u.shield = max(getattr(u, "shield", 0), amt); u.shield_t = dur
        elif kind in ("knockback", "pull"):
            if once:
                done.add(ei)
                amt = eff.get("amount", 120) * (1 if kind == "knockback" else -1)
                for u in tgts:
                    dx, dy = u.x - sx, u.y - sy
                    d = math.hypot(dx, dy) or 1
                    u.x += dx / d * amt; u.y += dy / d * amt
                    u.x = max(0, min(WORLD_W, u.x)); u.y = max(0, min(WORLD_H, u.y))
        elif kind == "spawn":
            if once:
                done.add(ei)
                uk = eff.get("unit", "soldier"); n = eff.get("count", 2)
                spec = self._factbook()[self.fac[team]]["units"].get(uk)
                if spec:
                    for k in range(int(n)):
                        u = Unit(uk, sx + (k % 4) * 26 - 40, sy + (k // 4) * 26, team, spec=spec)
                        self._apply_upgrades(u); self.units.append(u)
        elif kind == "reveal":
            if team == 0 and self.fog:
                cell = self.fog_cell
                cx = int(sx // cell); cy = int(sy // cell); rr = int(rad // cell) + 1
                for gy in range(max(0, cy - rr), min(self.fog_rows, cy + rr + 1)):
                    for gx in range(max(0, cx - rr), min(self.fog_cols, cx + rr + 1)):
                        if (gx - cx) ** 2 + (gy - cy) ** 2 <= rr * rr:
                            self.fog[gy][gx] = 2

    def _palette_action(self, action):
        kind, arg = action
        if kind == "train":
            for s in self._sel_structs():
                if arg in self.sspec(s.team, s.kind)["makes"]:
                    self._queue_train(s, arg)
        elif kind == "build":
            self.build_mode = arg
            self.flash(f"Placing {self.sspec(0, arg)['name']} — click to build.")
        elif kind == "research":
            for s in self._sel_structs():
                if arg in self.sspec(s.team, s.kind).get("researches", []):
                    self._start_research(s, arg)
        elif kind == "cast":
            casters = [u for u in self._sel_units() if u.ability and u.ability["id"] == arg]
            if casters:
                self._start_cast(casters[0].ability, casters)
        elif kind == "cmd":
            if arg == "stop":
                for u in self._sel_units():
                    u.goal = None; u.target = None; u.state = "idle"; u.stance = "aggressive"
            elif arg == "attack":
                self.flash("Right-click a target to attack.")
            elif arg == "hold":
                for u in self._sel_units():
                    u.goal = None; u.state = "idle"; u.stance = "hold"; u.patrol = None
                self.flash("Holding position")
            elif arg == "patrol":
                # arm patrol: the next right-click sets the far endpoint
                self._patrol_arm = True
                self.flash("Right-click to set the patrol route.")

    def _minimap_jump(self, mx, my):
        r = self.mm_rect
        fx = (mx - r.x) / r.w; fy = (my - r.y) / r.h
        self.cam_x = fx * WORLD_W - self.W / (2 * self.zoom)
        self.cam_y = fy * WORLD_H - (self.H - 118) / (2 * self.zoom)
        self._clamp_cam()

    # ── update ──
    def update(self, dt):
        self._frame_dt = dt
        if self.phase == "setup":
            return
        if self.done:
            # keep the clock ticking so the end-screen confetti stays animated
            self._anim_t = getattr(self, "_anim_t", 0.0) + min(dt, 0.05)
            return
        dt = min(dt, 0.05)
        self.match_t += dt
        self._anim_t = getattr(self, "_anim_t", 0.0) + dt
        for d in self.dying:
            d["t"] += dt
        self.dying = [d for d in self.dying if d["t"] < 0.6]
        for p in self.pings:
            p["t"] += dt
        self.pings = [p for p in self.pings if p["t"] < 4.0]
        self.msg_t = max(0.0, self.msg_t - dt)
        self._pad_tick(dt)
        self._tick_life(dt)
        # arm edge panning only after the grace period AND once we've seen a
        # real (canvas-translated) pointer event — never on a stale OS cursor.
        if not self.edge_pan_armed and self.match_t >= self.pan_grace and self.mouse_pos is not None:
            self.edge_pan_armed = True
        self._touch_tick(dt)
        self._edge_scroll(dt)
        self._income(dt)
        for s in list(self.structs):
            self._update_struct(s, dt)
        for u in list(self.units):
            self._update_unit(u, dt)
        self._tick_mechanics(dt)
        self._tick_relics(dt)
        self._tick_rocks_towers(dt)
        self._tick_spells(dt)
        self._tick_particles(dt)
        self._tick_events(dt)
        self.fog_t += dt
        if self.fog_t >= 0.25:              # recompute vision 4×/sec
            self.fog_t = 0.0
            self._update_fog()
        self._cull()
        if getattr(self, "spectate", False):
            # AI vs AI: alternate which side acts first each tick so neither gets
            # a systematic ordering edge
            self._spec_flip = not getattr(self, "_spec_flip", False)
            if self._spec_flip:
                self._ai(dt, team=0); self._ai(dt, team=1)
            else:
                self._ai(dt, team=1); self._ai(dt, team=0)
        else:
            self._ai(dt)
        self._check_end()

    def _income(self, dt):
        # passive trickle from Ink Reservoirs raises the cap; harvesting is active
        for t in (0, 1):
            reservoirs = sum(1 for s in self.structs if s.kind == "well" and s.team == t and s.complete)
            self.cap[t] = 400 + reservoirs * 250

    def _update_struct(self, s, dt):
        if not s.alive:
            return
        if not s.complete:
            # a scribe standing next to it raises it. AI-controlled teams also
            # self-raise slowly so the AI doesn't need perfect worker micro —
            # that's team 1 in normal play, and BOTH teams in spectate mode.
            ai_team = (s.team == 1) or getattr(self, "spectate", False)
            near = any(u.worker and u.team == s.team and math.hypot(u.x - s.x, u.y - s.y) < s.r + 20
                       for u in self.units)
            if near or ai_team:
                s.build_left = max(0.0, s.build_left - dt)
            return
        # train queued units (faction-specific build time)
        if s.queue:
            s.train_t += dt
            lead = s.queue[0]
            if s.train_t >= self.uspec(s.team, lead)["build_t"]:
                s.train_t = 0.0
                s.queue.pop(0)
                self._spawn_from(s, lead)
        # research in progress
        if s.research:
            uk, left = s.research
            left -= dt
            if left <= 0:
                self.upgrades[s.team].add(uk)
                up = self._factbook()[self.fac[s.team]]["upgrades"][uk]
                # retroactively apply to existing units of this team
                self._retro_upgrade(s.team, uk)
                if s.team == 0: self.flash(f"Researched: {up['name']}")
                s.research = None
            else:
                s.research = (uk, left)
        # defensive fire: any structure with tower stats shoots (the classic
        # Watch Spire keeps its numbers as the defaults) — factions tune their
        # towers into stingers, long-eyes, or thorned bulwarks via data.
        sp = self.sspec(s.team, s.kind)
        tdmg = sp.get("tower_dmg", 22 if s.kind == "spire" else 0)
        if tdmg > 0:
            s.cool = max(0.0, s.cool - dt)
            if s.cool <= 0:
                foe = self._nearest_enemy(s.x, s.y, s.team,
                                          sp.get("tower_rng", 240))
                if foe:
                    self._hurt(foe, tdmg)
                    s.cool = sp.get("tower_cool", 0.8)
                    self._emit(s.x, s.y - s.r * 0.6, 2, "ink",
                               self._tcol(s.team), spread=8, life=0.25, up=30)

    def _spawn_from(self, s, uk):
        ang = random.uniform(0, math.tau)
        ux = s.x + math.cos(ang) * (s.r + 22)
        uy = s.y + math.sin(ang) * (s.r + 22)
        u = Unit(uk, ux, uy, s.team, spec=self.uspec(s.team, uk))
        self._apply_upgrades(u)
        self.stats[s.team]["built"] += 1
        if s.team == 0:
            self._sfx("deploy", 0.5)
        if s.rally:
            u.goal = s.rally; u.state = "move"
            if u.worker:
                node = self._node_at(*s.rally) or self._nearest_node(s.rally[0], s.rally[1])
                if node: u.harvest_node = node; u.state = "toNode"; u.goal = (node.x, node.y)
        elif u.worker:
            node = self._nearest_node(s.x, s.y)
            if node: u.harvest_node = node; u.state = "toNode"; u.goal = (node.x, node.y)
        self.units.append(u)

    def _update_unit(self, u, dt):
        if not u.alive:
            return
        u.spawn_t = max(0.0, u.spawn_t - dt)
        u.cool = max(0.0, u.cool - dt)
        u.atk_anim = max(0.0, getattr(u, "atk_anim", 0.0) - dt)
        # momentum bleeds off slowly when not actively attacking a target
        if getattr(u, "momentum", 0.0) > 0 and u.target is None:
            u.momentum = max(0.0, u.momentum - dt * 0.3)
            if u.momentum == 0.0:
                u._mom_target = None
        # caster energy regen; snare timer decay
        if u.max_energy:
            u.energy = min(u.max_energy, u.energy + 0.9 * dt)
        if getattr(u, "cast_cd", 0) > 0:
            u.cast_cd = max(0.0, u.cast_cd - dt)
        if getattr(u, "slow_t", 0) > 0:
            u.slow_t = max(0.0, u.slow_t - dt)
        # cloaked units leave a fading shimmer trail while moving
        if getattr(u, "cloak", False) and u.state in ("move", "attack"):
            u.trail.append((u.x, u.y))
            if len(u.trail) > 8:
                u.trail.pop(0)
        elif getattr(u, "trail", None):
            if u.trail: u.trail.pop(0)
        # worker economy loop
        if u.worker and u.state in ("toNode", "mining", "toBase", "idle") and u.target is None:
            self._update_worker(u, dt); return
        # burrowed units are immobile (stationary cloak) — they just sit hidden
        if getattr(u, "burrowed", False):
            u.goal = None; u.target = None; u.state = "idle"
            return
        # ── stance behaviour ──
        # HOLD: attack only foes that come within range; never chase off the spot.
        if u.stance == "hold" and u.target is None and u.goal is None:
            foe = self._nearest_enemy(u.x, u.y, u.team, u.rng + 30)
            if foe:
                u.target = foe
            else:
                return                       # sit tight
        # PATROL: sweep between two points, engaging foes seen en route.
        if u.stance == "patrol" and u.patrol and u.target is None and u.goal is None:
            foe = self._nearest_enemy(u.x, u.y, u.team, 170)
            if foe:
                u.target = foe
            else:
                u.goal = u.patrol; u.state = "move"
        # combat / movement
        if u.target is not None:
            if not u.target.alive or (getattr(u.target, "cloak", False)
                                      and u.target.team != u.team and not self._detected(u.target)):
                u.target = None; u.state = "idle"
            else:
                # HOLD units break off if the foe flees beyond a short leash
                if u.stance == "hold":
                    hd = math.hypot(u.target.x - u.x, u.target.y - u.y)
                    if hd > u.rng + 60:
                        u.target = None; u.state = "idle"; return
                self._pursue_and_attack(u, u.target, dt); return
        if u.goal is not None:
            arrived = self._step_to(u, u.goal[0], u.goal[1], dt)
            if u.attack_move:
                foe = self._nearest_enemy(u.x, u.y, u.team, 170)
                if foe: u.target = foe; u.state = "attack"
            if arrived:
                if u.stance == "patrol" and u.patrol:
                    # bounce to the other endpoint
                    a = getattr(u, "patrol_a", (u.x, u.y)); b = u.patrol
                    nxt = a if math.hypot(u.x - b[0], u.y - b[1]) < math.hypot(u.x - a[0], u.y - a[1]) else b
                    u.goal = nxt; u.state = "move"
                elif u.waypoints:
                    u.goal = u.waypoints.pop(0)     # advance to the next queued point
                else:
                    u.goal = None; u.state = "idle"
        else:
            # idle soldiers auto-acquire nearby foes (leash)
            if not u.worker:
                foe = self._nearest_enemy(u.x, u.y, u.team, 150)
                if foe: u.target = foe; u.state = "attack"

    def _update_worker(self, u, dt):
        node = u.harvest_node
        if u.carrying >= INK_PER_TRIP:
            base = self._nearest_dropoff(u.x, u.y, u.team)
            if base is None: u.state = "idle"; return
            u.state = "toBase"
            if self._step_to(u, base.x, base.y, dt, stop=base.r + 12):
                gain = u.carrying * self.facdata(u.team)["econ"] * self._harvest_mult(u.team)
                # difficulty economy modifier applies to the AI opponent only in
                # normal play; in spectate BOTH sides are AI, so apply to neither
                # (keeps AI-vs-AI perfectly fair)
                if u.team == 1 and not getattr(self, "spectate", False):
                    gain *= self._diff_params()["econ"]
                self.ink[u.team] = min(self.cap[u.team], self.ink[u.team] + gain)
                self.stats[u.team]["harvested"] += gain
                u.carrying = 0
                u.state = "toNode"
            return
        if node is None or node.amount <= 0:
            node = self._nearest_node(u.x, u.y)
            u.harvest_node = node
            if node is None: u.state = "idle"; return
        if u.state == "mining":
            u.harvest_t += dt
            if u.harvest_t >= HARVEST_TIME:
                got = min(INK_PER_TRIP, node.amount)
                node.amount -= got; u.carrying = got; u.harvest_t = 0.0
                u.state = "toBase"
            return
        # walk to node
        if self._step_to(u, node.x, node.y, dt, stop=node.r + 8):
            u.state = "mining"; u.harvest_t = 0.0

    def _hurt(self, tgt, dmg, attacker=None):
        """Apply damage, honouring the target faction's defensive mechanic, and
        credit a kill (with veterancy rank-ups) to the attacker if this blow is
        lethal. Court 'cohesion': a unit takes less damage per nearby ally."""
        if isinstance(tgt, Unit) and self.facdata(tgt.team).get("mechanic") == "cohesion":
            allies = sum(1 for a in self.units
                         if a.alive and a.team == tgt.team and a is not tgt
                         and math.hypot(a.x - tgt.x, a.y - tgt.y) < 100)
            dmg *= (1.0 - min(0.42, allies * 0.11))   # up to −42% when grouped
        # high-ground advantage: attacking from low ground onto a target that
        # stands on a plateau deals reduced damage (the SC ramp defence)
        if attacker is not None and isinstance(attacker, Unit) and isinstance(tgt, Unit):
            if self._on_high(tgt.x, tgt.y) and not self._on_high(attacker.x, attacker.y):
                dmg *= 0.7
        was_alive = tgt.hp > 0
        # armour shred: a shredded target takes amplified damage
        if isinstance(tgt, Unit) and getattr(tgt, "shred", 1.0) > 1.0:
            dmg *= tgt.shred
        # paper-ward shield absorbs damage first (from a 'shield' effect)
        sh = getattr(tgt, "shield", 0)
        if sh > 0 and dmg > 0:
            absorbed = min(sh, dmg)
            tgt.shield = sh - absorbed
            dmg -= absorbed
        tgt.hp -= dmg
        # "under attack" alert: your stuff taking damage off-screen pings the
        # minimap, flashes a warning, and plays a cue (throttled) — StarCraft's
        # signature "Your forces are under attack!"
        if getattr(tgt, "team", None) == 0 and self._anim_t - self.alert_t > 5.0:
            if not self._on_screen(tgt.x, tgt.y):
                self.alert_t = self._anim_t
                self.pings.append(dict(x=tgt.x, y=tgt.y, t=0.0))
                self.flash("⚠ Your forces are under attack!")
                self._sfx("error", 0.5)
        # kill credit → veterancy for the slayer (combat units only)
        if was_alive and tgt.hp <= 0 and attacker is not None \
                and isinstance(attacker, Unit) and not attacker.worker:
            self._credit_kill(attacker)
        return dmg

    # veterancy rank thresholds (kills) and the per-rank stat multiplier
    _VET_THRESH = (1, 3, 6)
    def _credit_kill(self, u):
        u.vet_xp += 1
        new_rank = sum(1 for t in self._VET_THRESH if u.vet_xp >= t)
        if new_rank > u.vet_rank:
            # apply the delta for each rank gained (+12% dmg & maxhp per rank)
            for _ in range(new_rank - u.vet_rank):
                frac = u.hp / u.maxhp if u.maxhp else 1.0
                u.maxhp *= 1.12
                u.hp = u.maxhp * min(1.0, frac + 0.15)   # a small heal on promotion
                u.dmg *= 1.12
            u.vet_rank = new_rank
            self.stats[u.team]["vets"] += 1
            if u.team == 0:
                self._sfx("select", 0.4)

    def _pursue_and_attack(self, u, tgt, dt):
        if getattr(u, "stun_t", 0) > 0:
            return                            # frozen — cannot attack
        d = math.hypot(tgt.x - u.x, tgt.y - u.y)
        reach = u.rng + getattr(tgt, "r", 12)
        if d > reach:
            self._step_to(u, tgt.x, tgt.y, dt, stop=reach - 2)
        else:
            if u.cool <= 0:
                # Gilded Press momentum: staying on one target winds up bonus
                # damage (up to +50%); switching targets resets the press
                mom_mult = 1.0
                if self.facdata(u.team).get("mechanic") == "momentum":
                    if getattr(u, "_mom_target", None) is tgt:
                        u.momentum = min(1.0, getattr(u, "momentum", 0.0) + 0.28)
                    else:
                        # switching targets keeps some wind-up rather than a full
                        # reset, so momentum survives the chaos of a real battle
                        u._mom_target = tgt
                        u.momentum = max(0.0, getattr(u, "momentum", 0.0) - 0.4)
                    mom_mult = 1.0 + 0.7 * u.momentum
                self._hurt(tgt, u.dmg * getattr(u, "buff_dmg", 1.0) * mom_mult, attacker=u)
                u.cool = 0.6
                u.atk_anim = 0.3            # trigger an attack lunge on the model
                # ranged units loose a visible paper dart / ink bolt (visual only —
                # damage already applied, so balance is unchanged)
                if u.rng >= 90:
                    self.projectiles.append(dict(
                        x=u.x, y=u.y - u.r * 0.4, tx=tgt.x, ty=tgt.y - getattr(tgt, "r", 10) * 0.4,
                        t=0.0, dur=max(0.12, math.hypot(tgt.x - u.x, tgt.y - u.y) / 900),
                        col=self._tdark(u.team), team=u.team))
                else:
                    # melee: a small burst of paper scraps at the point of impact
                    self._emit(tgt.x, tgt.y, 3, "scrap", (230, 220, 190), spread=40, life=0.35, up=15)
                # throttled hit sfx (at most a few per second across the field)
                now = getattr(self, "_anim_t", 0.0)
                if now - getattr(self, "_last_hit_snd", -1) > 0.18:
                    self._last_hit_snd = now
                    self._sfx("tower_hit", 0.3)
                # Ember mechanic: attacks set the target alight (stacking DoT)
                if self.facdata(u.team).get("mechanic") == "burn" and isinstance(tgt, Unit):
                    tgt.burn = min(4.0, getattr(tgt, "burn", 0) + 2.0)
                    tgt.burn_dps = max(getattr(tgt, "burn_dps", 0), u.dmg * 0.25)
            u.face = 1 if tgt.x >= u.x else -1

    def _step_to(self, u, tx, ty, dt, stop=4.0):
        dx, dy = tx - u.x, ty - u.y
        d = math.hypot(dx, dy)
        if d <= stop:
            return True
        # base heading toward the goal
        hx, hy = dx / d, dy / d
        # local obstacle avoidance: if a blocking map object sits close ahead,
        # add a tangential steering push so the unit slides around it rather than
        # clipping through. Cheap per-unit (only a handful of blockers per map).
        if self._blockers:
            ax, ay = 0.0, 0.0
            look = u.r + 60
            for o in self._blockers:
                if getattr(o, "destructible", False) and not o.alive: continue
                ox, oy = o.x - u.x, o.y - u.y
                od = math.hypot(ox, oy)
                clear = o.r + u.r
                if od < clear + look and od > 1:
                    # only avoid things roughly in front of us
                    if ox * hx + oy * hy > -0.2 * od:
                        # steer perpendicular to the obstacle, away from its centre
                        push = (clear + look - od) / (clear + look)
                        # tangent direction: rotate the away-vector 90° toward goal
                        awx, awy = -ox / od, -oy / od
                        # pick the tangent that points more toward the goal
                        t1 = (-awy, awx); t2 = (awy, -awx)
                        tsel = t1 if (t1[0] * hx + t1[1] * hy) > (t2[0] * hx + t2[1] * hy) else t2
                        ax += (tsel[0] * 0.8 + awx * 0.5) * push
                        ay += (tsel[1] * 0.8 + awy * 0.5) * push
            hx += ax; hy += ay
            hl = math.hypot(hx, hy)
            if hl > 0.001:
                hx, hy = hx / hl, hy / hl
        step = u.spd * getattr(u, "buff_spd", 1.0) * dt
        if getattr(u, "stun_t", 0) > 0:
            return False                      # frozen — cannot move
        if getattr(u, "slow_t", 0) > 0:
            step *= 0.5                       # snared (Tide Undertow)
        u.x += hx * min(step, d)
        u.y += hy * min(step, d)
        # hard clamp: never let a unit end up inside a blocker's solid core — if it
        # did, push it back out to the surface (belt-and-braces with the steering)
        for o in self._blockers:
            if getattr(o, "destructible", False) and not o.alive: continue
            ox, oy = u.x - o.x, u.y - o.y
            od = math.hypot(ox, oy)
            mind = o.r + u.r * 0.5
            if od < mind and od > 0.1:
                u.x = o.x + ox / od * mind
                u.y = o.y + oy / od * mind
        u.face = 1 if dx >= 0 else -1
        # occasional dust scrap kicked up while moving (throttled, cheap)
        if not u.worker and random.random() < 0.04 * self.zoom:
            self._emit(u.x, u.y + u.r * 0.3, 1, "scrap", (200, 188, 160), spread=12, life=0.4, up=8)
        return d - step <= stop

    def _nearest_enemy(self, x, y, team, rad):
        best = None; bd = rad
        for e in self.units:
            if e.alive and e.team != team:
                # can't target a cloaked enemy that isn't detected
                if getattr(e, "cloak", False) and not self._detected(e):
                    continue
                dd = math.hypot(e.x - x, e.y - y)
                if dd < bd: bd = dd; best = e
        for s in self.structs:
            if s.alive and s.team != team:
                dd = math.hypot(s.x - x, s.y - y)
                if dd < bd: bd = dd; best = s
        return best

    def _nearest_node(self, x, y):
        live = [n for n in self.nodes if n.amount > 0]
        return min(live, key=lambda n: math.hypot(n.x - x, n.y - y)) if live else None

    def _nearest_dropoff(self, x, y, team):
        halls = [s for s in self.structs if s.team == team and s.complete
                 and s.kind in ("scriptorium", "well")]
        return min(halls, key=lambda s: math.hypot(s.x - x, s.y - y)) if halls else None

    def _tick_rocks_towers(self, dt):
        """Destructible rocks fall when HP is depleted (opening a path);
        watchtowers reveal a wide area for whichever team holds them."""
        for o in self.rocks:
            if o.alive and o.hp <= 0:
                o.alive = False; o.blocks = False
                self._emit(o.x, o.y, 24, "scrap", (150, 140, 120), spread=140, life=1.0, up=40)
                self._emit(o.x, o.y, 10, "ink", (60, 54, 44), spread=90, life=0.7)
                self._sfx("tower_hit", 0.6)
                self.flash(f"{o.label} cleared")
        for o in self.watchtowers:
            near = {0: 0, 1: 0}
            for u in self.units:
                if u.alive and math.hypot(u.x - o.x, u.y - o.y) < o.r + 70:
                    near[u.team] += 1
            o.owner = 0 if near[0] > near[1] else 1 if near[1] > near[0] else None
            if o.owner == 0 and self.fog:
                cell = self.fog_cell
                cx = int(o.x // cell); cy = int(o.y // cell); rad = int(o.sight // cell) + 1
                for gy in range(max(0, cy - rad), min(self.fog_rows, cy + rad + 1)):
                    for gx in range(max(0, cx - rad), min(self.fog_cols, cx + rad + 1)):
                        if (gx - cx) ** 2 + (gy - cy) ** 2 <= rad * rad:
                            self.fog[gy][gx] = 2

    def _tick_relics(self, dt):
        """Relics grant a slow ink bounty to whichever team has the most units
        nearby (contested = nobody). A light objective to fight over."""
        for o in self.objects:
            if o.kind != "relic":
                continue
            near = {0: 0, 1: 0}
            for u in self.units:
                if u.alive and math.hypot(u.x - o.x, u.y - o.y) < o.r + 90:
                    near[u.team] += 1
            if near[0] > near[1]:
                o.owner = 0
            elif near[1] > near[0]:
                o.owner = 1
            elif near[0] == 0:
                o.owner = None
            if o.owner is not None:
                self.ink[o.owner] = min(self.cap[o.owner], self.ink[o.owner] + 6 * dt)

    def _tick_mechanics(self, dt):
        """Faction-asymmetric passives: Ember burns tick down + damage, Tide
        units regenerate. Runs each tick over all units."""
        for u in self.units:
            if not u.alive:
                continue
            # burning (applied by Ember): chip damage while the timer runs
            if getattr(u, "burn", 0) > 0:
                u.burn -= dt
                u.hp -= u.burn_dps * dt
                if u.burn <= 0:
                    u.burn = 0.0; u.burn_dps = 0.0
            # Tide mechanic: steady regeneration, faster near your own structures
            if self.facdata(u.team).get("mechanic") == "regen" and u.hp < u.maxhp:
                rate = 0.015
                if any(s.team == u.team and s.complete and
                       math.hypot(s.x - u.x, s.y - u.y) < s.r + 140 for s in self.structs):
                    rate = 0.04
                u.hp = min(u.maxhp, u.hp + u.maxhp * rate * dt)
            # composable-ability buffs/shields expire on their timers
            if getattr(u, "buff_dmg_t", 0) > 0:
                u.buff_dmg_t -= dt
                if u.buff_dmg_t <= 0: u.buff_dmg = 1.0; u.buff_dmg_t = 0.0
            if getattr(u, "buff_spd_t", 0) > 0:
                u.buff_spd_t -= dt
                if u.buff_spd_t <= 0: u.buff_spd = 1.0; u.buff_spd_t = 0.0
            if getattr(u, "shield_t", 0) > 0:
                u.shield_t -= dt
                if u.shield_t <= 0: u.shield = 0.0; u.shield_t = 0.0
            if getattr(u, "stun_t", 0) > 0:
                u.stun_t -= dt
            if getattr(u, "shred_t", 0) > 0:
                u.shred_t -= dt
                if u.shred_t <= 0: u.shred = 1.0; u.shred_t = 0.0

    def _cull(self):
        # Ember mechanic: a dying Ember unit COMBUSTS, dealing area damage &
        # igniting nearby foes — punishes clumping and trading into them.
        for u in self.units:
            if not u.alive and not getattr(u, "_combusted", False) \
                    and self.facdata(u.team).get("mechanic") == "burn" and not u.worker:
                u._combusted = True
                for f in self.units:
                    if f.alive and f.team != u.team and math.hypot(f.x - u.x, f.y - u.y) < 70:
                        self._hurt(f, u.maxhp * 0.12)
                        if isinstance(f, Unit):
                            f.burn = min(4.0, getattr(f, "burn", 0) + 1.5)
                            f.burn_dps = max(getattr(f, "burn_dps", 0), u.dmg * 0.2)
        # capture freshly-dead combat units as display-only crumpling corpses
        for u in self.units:
            if not u.alive:
                self.stats[u.team]["lost"] += 1
                self.stats[1 - u.team]["killed"] += 1
                # ink splash + a couple of paper scraps where it fell
                self._emit(u.x, u.y, 8, "ink", (60, 54, 96), spread=70, life=0.5, up=20)
                self._emit(u.x, u.y, 4, "scrap", tuple(self._tcol(u.team)), spread=60, life=0.7, up=30)
                self.dying.append(dict(card=u.card, x=u.x, y=u.y, team=u.team,
                                       r=u.r, face=getattr(u, "face", 1), t=0.0))
        # structures that fell scatter a burst of paper scraps
        for s in self.structs:
            if not s.alive:
                self._emit(s.x, s.y, 20, "scrap", tuple(self._tcol(s.team)), spread=130, life=1.0, up=40)
                self._emit(s.x, s.y, 10, "ink", (50, 44, 80), spread=90, life=0.7)
        self.units = [u for u in self.units if u.alive]
        self.structs = [s for s in self.structs if s.alive]
        self.selected = [e for e in self.selected if getattr(e, "alive", False)]

    def _clamp_cam(self):
        # viewport size in world units grows as we zoom out
        vw = self.W / self.zoom; vh = (self.H - 118) / (self.zoom * ISO)
        self.cam_x = max(-60, min(WORLD_W - vw + 60, self.cam_x))
        self.cam_y = max(-60, min(WORLD_H - vh + 60, self.cam_y))

    def _edge_scroll(self, dt):
        # Screen-speed feel is constant across zoom levels: world speed scales
        # with 1/zoom (zoomed out pans more world, same apparent pace).
        base = 640 * dt / max(0.55, self.zoom)
        # ── mouse edge panning ──
        # CRITICAL: use the last event-translated CANVAS-space position, NOT
        # pygame.mouse.get_pos() (window space — the old phantom-pan bug).
        # Only pan once armed and while the pointer is inside the canvas.
        px_ = py_ = 0.0
        if self.edge_pan_armed and self.mouse_pos is not None:
            mx, my = self.mouse_pos
            if 0 <= mx <= self.W and 0 <= my <= self.H:
                edge = 56                     # trigger band (canvas px)
                # PROPORTIONAL: depth into the band drives speed 0→1 with a
                # squared curve, so grazing the band drifts and the rim flies.
                if mx < edge:                 px_ = -((edge - mx) / edge) ** 2
                elif mx > self.W - edge:      px_ = ((mx - (self.W - edge)) / edge) ** 2
                bot = self.H - 120
                if my < edge:                 py_ = -((edge - my) / edge) ** 2
                elif my > bot - edge:         py_ = min(1.0, ((my - (bot - edge)) / edge)) ** 2
        # diagonal corners shouldn't pan √2 faster — normalise the vector
        mag = math.hypot(px_, py_)
        if mag > 1.0:
            px_ /= mag; py_ /= mag
        # ease the pan in/out (~120ms) so motion starts and stops soft-footed
        target = 1.0 if mag > 0.001 else 0.0
        k = min(1.0, dt / 0.12)
        self._pan_ramp += (target - self._pan_ramp) * k
        if self._pan_ramp > 0.003:
            self.cam_x += px_ * base * self._pan_ramp
            self.cam_y += py_ * base * self._pan_ramp
        # arrow keys too (always available, independent of arming)
        keys = pygame.key.get_pressed()
        if keys[pygame.K_LEFT]: self.cam_x -= base
        if keys[pygame.K_RIGHT]: self.cam_x += base
        if keys[pygame.K_UP]: self.cam_y -= base
        if keys[pygame.K_DOWN]: self.cam_y += base
        self._clamp_cam()

    # ── AI ──
    def _diff_params(self):
        """AI tuning by difficulty: worker target, army attack threshold, cadence
        scale (lower = thinks more often), AI economy multiplier, and whether it
        uses advanced tactics (defensive spires, high-ground staging, cloak raids)."""
        return [
            dict(workers=5, attack=11, cadence=1.5, econ=0.9, adv=False),   # Easy
            dict(workers=7, attack=8,  cadence=1.0, econ=1.0, adv=True),    # Normal
            dict(workers=9, attack=6,  cadence=0.7, econ=1.15, adv=True),   # Hard
        ][self.difficulty]

    def _ai(self, dt, team=1):
        dp = self._diff_params()
        # per-team AI cadence timer (supports optional dual-AI/observer runs)
        timers = getattr(self, "_ai_timers", None)
        if timers is None:
            timers = self._ai_timers = {}
        timers[team] = timers.get(team, 0.0) - dt
        if timers[team] > 0:
            return
        timers[team] = random.uniform(2.5, 4.5) * dp["cadence"]
        self.ai_t = timers[team]
        hall = next((s for s in self.structs if s.team == team and s.kind == "scriptorium"), None)
        if not hall:
            return
        workers = [u for u in self.units if u.team == team and u.worker]
        army = [u for u in self.units if u.team == team and not u.worker]
        all_tables = [s for s in self.structs if s.team == team and s.kind == "table"]
        tables = [s for s in all_tables if s.complete]
        binderies = [s for s in self.structs if s.team == team and s.kind == "bindery"]
        spires = [s for s in self.structs if s.team == team and s.kind == "spire"]
        us = lambda k: self.uspec(team, k)
        # supply-block relief: if we're near the cap, raise an Ink Reservoir
        if self._supply_used(team) + 3 >= self._supply_cap(team) and \
                self.ink[team] >= self.sspec(team, "well")["cost"] and \
                not any(s.team == team and s.kind == "well" and not s.complete for s in self.structs):
            self._ai_place("well", hall)
        # keep enough workers (difficulty-scaled)
        elif len(workers) < dp["workers"] and self.ink[team] >= us("scribe")["cost"] and \
                self._supply_free(team, "scribe"):
            hall.queue.append("scribe"); self.ink[team] -= us("scribe")["cost"]
        # build exactly one table
        elif not all_tables and self.ink[team] >= self.sspec(team, "table")["cost"]:
            self._ai_place("table", hall)
        # advanced tactics: raise a defensive Watch Spire at home (detection +
        # a guard tower) once teched, so cloaked raids don't go unanswered
        elif dp["adv"] and tables and not spires and len(army) >= 3 and \
                self.ink[team] >= self.sspec(team, "spire")["cost"]:
            self._ai_place("spire", hall)
        # tech up to a bindery (tier 2) once we have an army core
        elif tables and not binderies and len(army) >= 4 and \
                self.ink[team] >= self.sspec(team, "bindery")["cost"]:
            self._ai_place("bindery", hall)
        # research an upgrade if a researcher is free and idle
        elif random.random() < 0.4 and self._ai_try_research(team):
            pass
        # train army from ready tables / binderies
        elif tables and self.ink[team] >= 60:
            prod = tables + [b for b in binderies if b.complete]
            t = random.choice(prod)
            makes = self.sspec(team, t.kind)["makes"]
            # ensure the AI fields a caster: if a bindery is up and we have none,
            # train the caster (a "makes" entry that has an ability)
            caster_kind = next((mk for mk in makes
                                if self.uspec(team, mk).get("ability")), None)
            have_caster = any(u.team == team and u.ability for u in self.units)
            if caster_kind and not have_caster and self.ink[team] >= self.uspec(team, caster_kind)["cost"] \
                    and self._supply_free(team, caster_kind) and len(t.queue) < 3:
                t.queue.append(caster_kind); self.ink[team] -= self.uspec(team, caster_kind)["cost"]
            else:
                pick = random.choice([m for m in makes if not self.uspec(team, m).get("ability")]
                                     + ["soldier", "soldier"])
                if pick in makes and self.ink[team] >= us(pick)["cost"] and len(t.queue) < 3 \
                        and self._supply_free(team, pick):
                    t.queue.append(pick); self.ink[team] -= us(pick)["cost"]
        # attack when the army is big enough (per-team phase so dual-AI is fair)
        phases = getattr(self, "_ai_phases", None)
        if phases is None:
            phases = self._ai_phases = {}
        foe_team = 1 - team
        # attack threshold eases down as the match drags on, so late-game
        # stalemates resolve instead of turtling forever
        thresh = dp["attack"]
        if self.match_t > 100: thresh = max(5, thresh - 2)
        if self.match_t > 170: thresh = max(3, thresh - 3)
        if self.match_t > 240: thresh = 2
        if len(army) >= thresh and phases.get(team) != "attack":
            phases[team] = "attack"
        if phases.get(team) == "attack":
            target = next((s for s in self.structs if s.team == foe_team and s.kind == "scriptorium"), None)
            if target is None:
                target = next((s for s in self.structs if s.team == foe_team), None)
            if target:
                # advanced: stage through a high-ground plateau near the target
                # for the combat/vision edge, instead of marching straight in
                stage = None
                if dp["adv"] and self._high:
                    stage = min(self._high, key=lambda h: math.hypot(h[0] - target.x, h[1] - target.y))
                for u in army:
                    # if we're already near the enemy base and idle, lock onto a
                    # structure there so the assault actually lands (no dithering)
                    if u.target is None and math.hypot(u.x - target.x, u.y - target.y) < 360:
                        near_struct = min((s for s in self.structs if s.team == foe_team),
                                          key=lambda s: math.hypot(s.x - u.x, s.y - u.y),
                                          default=None)
                        if near_struct:
                            u.target = near_struct; u.state = "attack"; u.goal = None
                            continue
                    if u.target is None and u.goal is None:
                        if stage and math.hypot(u.x - stage[0], u.y - stage[1]) > stage[2] + 40 \
                                and random.random() < 0.5:
                            u.goal = (stage[0], stage[1])
                        else:
                            u.goal = (target.x, target.y)
                        u.attack_move = True; u.state = "move"
            if len(army) < 3:
                phases[team] = "grow"
        # advanced: cloaked-raider harassment — send an idle raider at the enemy's
        # worker line to snipe economy while the main army is elsewhere
        if dp["adv"]:
            raider = next((u for u in self.units if u.team == team and u.alive
                           and getattr(u, "cloak", False) and not getattr(u, "burrowed", False)
                           and u.target is None and u.goal is None), None)
            if raider:
                ew = [u for u in self.units if u.team == foe_team and u.alive and u.worker]
                if ew:
                    tgt = min(ew, key=lambda w: math.hypot(w.x - raider.x, w.y - raider.y))
                    raider.goal = (tgt.x, tgt.y); raider.attack_move = True; raider.state = "move"
        # cast spells: AI casters drop offensive spells on player clusters, or
        # heal their own wounded (throttled by energy + cooldown, and working for
        # custom composite abilities that have no legacy 'kind')
        for c in self.units:
            if c.team != team or not c.alive or not c.ability:
                continue
            ab = c.ability
            if c.energy < ab.get("cost", 0) or getattr(c, "cast_cd", 0) > 0:
                continue
            effs = _ability_effects(ab)
            helps_allies = any(e.get("effect") in ("heal", "buff", "shield") or
                               e.get("target") in ("ally", "self") for e in effs)
            hits_foes = any(e.get("effect") in ("damage", "slow", "stun", "burn",
                            "shred", "drain", "knockback", "pull") and
                            e.get("target", "enemy") in ("enemy", "all") for e in effs)
            rng = ab.get("cast_range", 400)
            if hits_foes:
                foes = [u for u in self.units if u.alive and u.team == foe_team
                        and math.hypot(u.x - c.x, u.y - c.y) < rng]
                if len(foes) >= 2:
                    tx = sum(u.x for u in foes) / len(foes); ty = sum(u.y for u in foes) / len(foes)
                    c.energy -= ab.get("cost", 0); c.cast_cd = ab.get("cooldown", 0)
                    self._cast_ability(ab, tx, ty, c)
                    continue
            if helps_allies:
                hurt = [u for u in self.units if u.alive and u.team == team
                        and u.hp < u.maxhp * 0.7 and math.hypot(u.x - c.x, u.y - c.y) < rng]
                if len(hurt) >= 2:
                    tx = sum(u.x for u in hurt) / len(hurt); ty = sum(u.y for u in hurt) / len(hurt)
                    c.energy -= ab.get("cost", 0); c.cast_cd = ab.get("cooldown", 0)
                    self._cast_ability(ab, tx, ty, c)

    def _ai_try_research(self, team):
        """Have an idle production/tech building start a random un-researched upgrade."""
        ups = self._factbook()[self.fac[team]]["upgrades"]
        for s in self.structs:
            if s.team != team or not s.complete or s.research is not None:
                continue
            avail = self.sspec(team, s.kind).get("researches", [])
            todo = [u for u in avail if u not in self.upgrades[team] and self.ink[team] >= ups[u]["cost"]]
            if todo:
                self._start_research(s, random.choice(todo))
                return True
        return False

    def _ai_place(self, sk, hall):
        team = hall.team
        spec = self.sspec(team, sk)
        for _ in range(20):
            ang = random.uniform(0, math.tau); rad = random.uniform(90, 220)
            wx = hall.x + math.cos(ang) * rad; wy = hall.y + math.sin(ang) * rad
            if not self._placement_blocked(wx, wy, spec["r"]):
                self.ink[team] -= spec["cost"]
                self.structs.append(Structure(sk, wx, wy, team, complete=False, spec=spec))
                self.stats[team]["structs"] += 1
                return

    def _check_end(self):
        if self.done:
            return
        p = any(s.team == 0 and s.kind == "scriptorium" for s in self.structs)
        e = any(s.team == 1 and s.kind == "scriptorium" for s in self.structs)
        if not e:
            self.done = True; self.result = "win"; self._sfx("victory", 0.8)
            self.flash("Victory — their Scriptorium is unmade!")
        elif not p:
            self.done = True; self.result = "lose"; self._sfx("defeat", 0.8)
            self.flash("Defeat — your Scriptorium has fallen.")
        if self.done and self.tournament is not None and not self._t_reported:
            self._t_reported = True
            self.tournament = tourney.report_player(
                self.tournament, self.result == "win", self._factbook())

    # ── rendering ──
    def draw(self, screen):
        if self.phase == "setup":
            self._draw_setup(screen)
            return
        screen.fill(tuple(self.theme["ground"]))
        self._draw_grid(screen)
        self._draw_highground(screen)
        self._draw_pools(screen)
        self._draw_motes(screen)
        self._draw_props(screen)
        self._draw_fauna(screen)
        self._draw_nodes(screen)
        self._draw_objects(screen)
        # structures under units — enemy structures only if in vision (last-seen
        # enemy buildings still show on explored ground, StarCraft-style)
        for s in sorted(self.structs, key=lambda s: s.y):
            if s.team != 0 and self.fog_on and self._cell_state(s.x, s.y) == 0:
                continue
            self._draw_struct(screen, s)
        self._draw_dying(screen)
        for u in sorted(self.units, key=lambda u: u.y):
            # enemy units are hidden unless currently visible
            if u.team != 0 and self.fog_on and not self._visible(u):
                continue
            # cloaked enemies are invisible unless a detector reveals them
            if u.team != 0 and getattr(u, "cloak", False) and not self._detected(u):
                continue
            self._draw_unit(screen, u)
        # fog overlay (drawn over the world, under the HUD)
        if self.fog_on:
            self._draw_fog(screen)
        # soft paper vignette around the play area
        vg = getattr(self, "_vignette", None)
        if vg is not None:
            screen.blit(vg, (0, 0))
        self._draw_spells(screen)
        self._draw_particles(screen)
        self._draw_projectiles(screen)
        self._draw_spell_bolts(screen)
        # weather / lighting grade over the world
        self._draw_weather(screen)
        if self.cast_mode:
            self._draw_cast_ghost(screen)
        self._draw_vignette(screen)
        # waypoint paths for selected units (shift-queued orders)
        for u in self._sel_units():
            if u.waypoints:
                pts = [self.w2s(u.x, u.y)]
                if u.goal: pts.append(self.w2s(*u.goal))
                pts += [self.w2s(*w) for w in u.waypoints]
                if len(pts) > 1:
                    pygame.draw.lines(screen, (240, 230, 120), False,
                                      [(int(x), int(y)) for x, y in pts], 1)
                    for wx, wy in pts[1:]:
                        pygame.draw.circle(screen, (240, 230, 120), (int(wx), int(wy)), 3, 1)
        if self.box:
            self._draw_box(screen)
        if self.build_mode:
            self._draw_ghost(screen)
        self._draw_hud(screen)
        self._draw_quick_buttons(screen)
        self._draw_minimap(screen)
        if self.wheel:
            self._draw_wheel(screen)
        if self.show_tech:
            self._draw_tech_tree(screen)
        if self.msg_t > 0 or self._ink_anim.get("toast", 0) > 0:
            self._draw_msg(screen)
        self._draw_pad_reticle(screen)
        self._draw_palette_overlay(screen)
        if self.done:
            self._draw_end(screen)

    def _draw_faction_emblem(self, screen, fk, cx, cy, r, col):
        """A distinct folded-paper crest per faction, drawn as a small emblem."""
        dark = tuple(int(c * 0.6) for c in col)
        light = tuple(min(255, int(c * 1.3)) for c in col)
        # a paper medallion backing
        pygame.draw.circle(screen, (36, 32, 26), (cx, cy), r + 2)
        if fk == "court":
            # a folded fan / spread of pleats
            for k in range(5):
                a = -math.pi / 2 + (k - 2) * 0.42
                ex = cx + math.cos(a) * r; ey = cy + math.sin(a) * r
                pygame.draw.line(screen, light if k % 2 else col, (cx, cy + r * 0.4), (ex, ey), 2)
            pygame.draw.arc(screen, col, (cx - r, cy - r * 0.6, r * 2, r * 1.6),
                            math.pi * 1.15, math.pi * 1.85, 2)
        elif fk == "ember":
            # a rising flame (stacked folded points)
            pts = [(cx, cy - r), (cx + r * 0.5, cy), (cx + r * 0.25, cy + r * 0.2),
                   (cx, cy + r * 0.6), (cx - r * 0.25, cy + r * 0.2), (cx - r * 0.5, cy)]
            pygame.draw.polygon(screen, col, pts)
            pygame.draw.polygon(screen, light, [(cx, cy - r * 0.4), (cx + r * 0.25, cy + r * 0.1),
                                                (cx, cy + r * 0.4), (cx - r * 0.25, cy + r * 0.1)])
        elif fk == "tide":
            # nested waves
            for k in range(3):
                yy = cy - r * 0.5 + k * r * 0.5
                pts = [(cx - r + i * r / 3, yy + math.sin(i) * 3) for i in range(7)]
                pygame.draw.lines(screen, light if k == 1 else col, False,
                                  [(int(px), int(yy + math.sin(px * 0.4) * 4)) for px in
                                   range(cx - r, cx + r, 4)], 2)
        elif fk == "press":
            # a printing-press platen: a gilded cog with a central stamp
            for k in range(8):
                a = k / 8 * math.tau
                ex = cx + math.cos(a) * r; ey = cy + math.sin(a) * r
                pygame.draw.line(screen, col, (cx, cy), (int(ex), int(ey)), 2)
            pygame.draw.circle(screen, col, (cx, cy), int(r * 0.6))
            pygame.draw.circle(screen, dark, (cx, cy), int(r * 0.6), 1)
            pygame.draw.rect(screen, light,
                             (int(cx - r * 0.22), int(cy - r * 0.22), int(r * 0.44), int(r * 0.44)))
        else:
            # expansion factions: a generic folded star in the faction colour —
            # deterministic in the key so each race keeps a stable crest.
            pts = 5 + (sum(ord(c) for c in fk) % 3)          # 5..7 points
            outer, inner = r, r * 0.45
            star = []
            for k in range(pts * 2):
                rad = outer if k % 2 == 0 else inner
                a = -math.pi / 2 + k * math.pi / pts
                star.append((cx + math.cos(a) * rad, cy + math.sin(a) * rad))
            pygame.draw.polygon(screen, col, star)
            pygame.draw.polygon(screen, light, star, 1)
            pygame.draw.circle(screen, dark, (cx, cy), int(r * 0.28))
        pygame.draw.circle(screen, dark, (cx, cy), r + 2, 1)

    def _draw_setup(self, screen):
        """Pre-match picker: choose your faction and the map. Enemy faction is
        random for variety."""
        screen.fill((30, 26, 22))
        f = self.ctx["font"]; sf = self.ctx["sfont"]; bf = self.ctx["bfont"]
        self.setup_rects = []
        title = bf.render("FOLDWORKS", True, (224, 196, 120))
        screen.blit(title, (self.W // 2 - title.get_width() // 2, 40))
        sub = f.render("Choose your bindery and battleground.", True, (222, 214, 198))
        screen.blit(sub, (self.W // 2 - sub.get_width() // 2, 104))
        # faction cards
        fac_keys = list(FACTIONS.keys())
        screen.blit(f.render("Faction", True, (200, 180, 120)), (80, 150))
        fw = (self.W - 200) // len(fac_keys)
        for i, fk in enumerate(fac_keys):
            fac = FACTIONS[fk]
            box = pygame.Rect(80 + i * fw, 180, fw - 20, 150)
            sel = (i == self.setup_fac)
            pygame.draw.rect(screen, (44, 38, 30) if not sel else (60, 52, 40), box, border_radius=8)
            pygame.draw.rect(screen, fac["color"] if sel else (90, 82, 70), box, 3 if sel else 1, border_radius=8)
            screen.blit(f.render(fac["name"], True, fac["color"]), (box.x + 12, box.y + 10))
            # a distinct paper emblem for the faction, lower-right of the card
            self._draw_faction_emblem(screen, fk, box.right - 34, box.bottom - 34, 22, fac["color"])
            # wrap the blurb
            words = fac["trait"].split(); line = ""; yy = box.y + 42
            for w in words:
                if sf.size(line + " " + w)[0] > box.w - 24:
                    screen.blit(sf.render(line, True, (210, 202, 186)), (box.x + 12, yy)); yy += 18; line = w
                else:
                    line = (line + " " + w).strip()
            if line: screen.blit(sf.render(line, True, (210, 202, 186)), (box.x + 12, yy))
            self.setup_rects.append((box, ("fac", i)))
        # map list
        screen.blit(f.render("Map", True, (200, 180, 120)), (80, 356))
        map_keys = list(MAPS.keys())
        mw = (self.W - 160) // 3
        for i, mk in enumerate(map_keys):
            m = MAPS[mk]
            col = i % 3; rowi = i // 3
            box = pygame.Rect(80 + col * (mw + 10), 386 + rowi * 84, mw, 76)
            sel = (i == self.setup_map)
            pygame.draw.rect(screen, (44, 38, 30) if not sel else (60, 52, 40), box, border_radius=6)
            pygame.draw.rect(screen, (224, 196, 120) if sel else (90, 82, 70), box, 3 if sel else 1, border_radius=6)
            # a little themed map thumbnail on the right of the tile
            thumb = pygame.Rect(box.right - 92, box.y + 8, 84, 60)
            th = m.get("theme", {"ground": [196, 178, 148], "accent": [150, 120, 80]})
            pygame.draw.rect(screen, tuple(th["ground"]), thumb, border_radius=3)
            pygame.draw.rect(screen, (70, 62, 52), thumb, 1, border_radius=3)
            mw2, mh2 = m["size"]; tsx = thumb.w / mw2; tsy = thumb.h / mh2
            for hx, hy, hr in m.get("highground", []):
                pygame.draw.circle(screen, tuple(min(255, c + 16) for c in th["ground"]),
                                   (int(thumb.x + hx * tsx), int(thumb.y + hy * tsy)), max(2, int(hr * tsx)))
            for o in m.get("objects", []):
                if o.get("kind") in ("ruin", "obstacle"):
                    pygame.draw.circle(screen, (110, 100, 86),
                                       (int(thumb.x + o["x"] * tsx), int(thumb.y + o["y"] * tsy)), 2)
            for si, (spx, spy) in enumerate(m["spawns"]):
                pygame.draw.circle(screen, (90, 150, 220) if si == 0 else (216, 100, 92),
                                   (int(thumb.x + spx * tsx), int(thumb.y + spy * tsy)), 3)
            screen.blit(f.render(m["name"], True, (232, 224, 208)), (box.x + 10, box.y + 8))
            screen.blit(sf.render(f"{m['size'][0]}×{m['size'][1]}", True, (150, 200, 235)), (box.x + 10, box.y + 32))
            screen.blit(sf.render(m["desc"][:34], True, (180, 172, 156)), (box.x + 10, box.y + 52))
            self.setup_rects.append((box, ("map", i)))
        # difficulty selector
        screen.blit(sf.render("Difficulty", True, (200, 180, 120)), (self.W // 2 - 200, self.H - 108))
        for i, name in enumerate(self._diff_names):
            db = pygame.Rect(self.W // 2 - 120 + i * 90, self.H - 112, 82, 28)
            sel = (i == self.difficulty)
            pygame.draw.rect(screen, (60, 52, 40) if sel else (44, 38, 30), db, border_radius=6)
            pygame.draw.rect(screen, (224, 196, 120) if sel else (90, 82, 70), db, 2 if sel else 1, border_radius=6)
            screen.blit(sf.render(name, True, (232, 224, 208)),
                        (db.centerx - sf.size(name)[0] // 2, db.y + 6))
            self.setup_rects.append((db, ("diff", i)))
        # start button
        start = pygame.Rect(self.W // 2 - 230, self.H - 70, 220, 44)
        hover = start.collidepoint(pygame.mouse.get_pos())
        pygame.draw.rect(screen, (120, 90, 50) if hover else (90, 68, 40), start, border_radius=8)
        pygame.draw.rect(screen, (224, 196, 120), start, 2, border_radius=8)
        st = f.render("Begin Battle", True, (240, 228, 200))
        screen.blit(st, (start.centerx - st.get_width() // 2, start.centery - st.get_height() // 2))
        self.setup_rects.append((start, ("start", 0)))
        # tournament button — starts a fresh ladder or resumes the saved one
        if tourney is not None:
            trun = tourney.active()
            tb = pygame.Rect(self.W // 2 - 230, self.H - 122, 460, 40)
            hovert = tb.collidepoint(pygame.mouse.get_pos())
            pygame.draw.rect(screen, (96, 70, 96) if hovert else (70, 52, 72),
                             tb, border_radius=8)
            pygame.draw.rect(screen, (216, 170, 220), tb, 2, border_radius=8)
            if trun:
                label = f"Continue Tournament — {tourney.round_name(trun)}"
            else:
                label = "Tournament — every bindery enters the bracket"
            tl = self.ctx["font"].render(label, True, (238, 224, 240))
            screen.blit(tl, (tb.centerx - tl.get_width() // 2,
                             tb.centery - tl.get_height() // 2))
            self.setup_rects.append((tb, ("tournament", 0)))
        # watch AI-vs-AI button
        watch = pygame.Rect(self.W // 2 + 10, self.H - 70, 220, 44)
        hoverw = watch.collidepoint(pygame.mouse.get_pos())
        pygame.draw.rect(screen, (60, 76, 96) if hoverw else (46, 58, 74), watch, border_radius=8)
        pygame.draw.rect(screen, (150, 200, 235), watch, 2, border_radius=8)
        wt = f.render("Watch AI Battle", True, (216, 232, 244))
        screen.blit(wt, (watch.centerx - wt.get_width() // 2, watch.centery - wt.get_height() // 2))
        self.setup_rects.append((watch, ("watch", 0)))

    def _setup_input(self, ev):
        pt = None
        if ev.type == pygame.MOUSEBUTTONDOWN:
            pt = ev.pos
        elif ev.type == pygame.FINGERDOWN:
            pt = self._finger_pos(ev)
        elif ev.type == pygame.KEYDOWN and ev.key in (pygame.K_RETURN, pygame.K_KP_ENTER, pygame.K_SPACE):
            self._begin_match(); return
        elif ev.type == pygame.KEYDOWN and ev.key == pygame.K_ESCAPE:
            self.exit_requested = True; return
        if pt is None:
            return
        for rect, action in self.setup_rects:
            if rect.collidepoint(pt):
                kind, idx = action
                if kind == "fac": self.setup_fac = idx
                elif kind == "map": self.setup_map = idx
                elif kind == "diff": self.difficulty = idx
                elif kind == "start": self._begin_match()
                elif kind == "watch": self._begin_match(spectate=True)
                elif kind == "tournament": self._begin_tournament_match()
                return

    def _begin_tournament_match(self):
        """Start (or resume) the ladder: seed a bracket on first press, then
        play the player's pairing with the enemy faction FORCED to the draw."""
        if tourney is None:
            return
        fac_keys = list(FACTIONS.keys())
        state = tourney.active()
        if state is None:
            player = fac_keys[self.setup_fac % len(fac_keys)]
            state = tourney.new(player, fac_keys)
            self.flash(f"The bracket is drawn — {tourney.round_name(state)}.")
        m = tourney.current_match(state)
        if m is None:                        # a bye, or the run has finished
            state = tourney.report_player(state, True, self._factbook())
            m = tourney.current_match(state)
            if m is None:
                self.flash("The tournament is decided.")
                return
        self.tournament = state
        self._t_reported = False
        # the player plays their own faction; the opponent is the draw
        self.setup_fac = fac_keys.index(state["player_fac"])
        self.forced_enemy = m["b"] if m["player_side"] == 0 else m["a"]
        self._begin_match()
        self.flash(f"{tourney.round_name(state)} — "
                   f"{self.facdata(0)['name']} vs {self.facdata(1)['name']}")

    def _tournament_next(self):
        """From the end screen: hop straight into the next round's match."""
        if self.tournament is None or tourney is None:
            return
        if self.tournament.get("done"):
            champ = tourney.champion(self.tournament)
            name = self._factbook().get(champ, {}).get("name", champ)
            self.flash(f"Champion: {name}")
            return
        if not tourney.player_alive(self.tournament):
            self.flash("Your run is over — the herald plays out the rest.")
            return
        m = tourney.current_match(self.tournament)
        if m is None:
            self.tournament = tourney.report_player(
                self.tournament, True, self._factbook())
            m = tourney.current_match(self.tournament)
            if m is None:
                return
        self.done = False; self.result = None
        self._t_reported = False
        self.forced_enemy = m["b"] if m["player_side"] == 0 else m["a"]
        self._begin_match()
        self.flash(f"{tourney.round_name(self.tournament)} — "
                   f"{self.facdata(0)['name']} vs {self.facdata(1)['name']}")

    def _draw_tech_tree(self, screen):
        """A full-screen overlay of the player faction's tech tree: structures by
        tier, the units each trains, and the upgrades each researches. Toggle Tab."""
        fac = self._factbook()[self.fac[0]]
        veil = pygame.Surface((self.W, self.H), pygame.SRCALPHA); veil.fill((12, 10, 8, 230))
        screen.blit(veil, (0, 0))
        f = self.ctx["font"]; sf = self.ctx["sfont"]; bf = self.ctx["bfont"]
        title = bf.render(fac["name"], True, fac["color"])
        screen.blit(title, (60, 40))
        screen.blit(f.render(fac["trait"], True, (222, 214, 198)), (62, 96))
        screen.blit(sf.render("Tech Tree — Tab to close", True, (150, 140, 124)), (62, 122))
        # columns by tier
        tiers = {1: [], 2: [], 3: []}
        for sk, sp in fac["structures"].items():
            tiers[sp["tier"]].append((sk, sp))
        col_w = (self.W - 120) // 3
        for ti in (1, 2, 3):
            cx = 60 + (ti - 1) * col_w
            y = 170
            hdr = f.render(f"Tier {ti}", True, (230, 200, 120))
            screen.blit(hdr, (cx, y)); y += 30
            for sk, sp in tiers[ti]:
                box = pygame.Rect(cx, y, col_w - 24, 96)
                built = any(s.team == 0 and s.kind == sk and s.complete for s in self.structs)
                avail = self._struct_available(0, sk)
                border = (150, 220, 150) if built else (fac["color"] if avail else (90, 82, 70))
                pygame.draw.rect(screen, (34, 30, 24), box, border_radius=6)
                pygame.draw.rect(screen, border, box, 2, border_radius=6)
                screen.blit(f.render(sp["name"], True, (235, 228, 212)), (box.x + 8, box.y + 6))
                info = f"{sp['cost']} ink" if sp["cost"] else "start"
                if sp.get("requires"):
                    info += "  ← " + ", ".join(fac["structures"][r]["name"] for r in sp["requires"])
                screen.blit(sf.render(info, True, (170, 160, 144)), (box.x + 8, box.y + 30))
                makes = sp.get("makes", [])
                if makes:
                    txt = "Trains: " + ", ".join(fac["units"][m]["name"] for m in makes)
                    screen.blit(sf.render(txt, True, (150, 200, 235)), (box.x + 8, box.y + 50))
                res = sp.get("researches", [])
                if res:
                    txt = "Research: " + ", ".join(fac["upgrades"][rk]["name"] for rk in res)
                    screen.blit(sf.render(txt[:52], True, (220, 180, 120)), (box.x + 8, box.y + 70))
                y += 108

    def _draw_quick_buttons(self, screen):
        """Minimal army chips above the HUD: quiet ghost outlines at rest that
        ink-pop solid on hover; the Idle chip only pops into existence when a
        worker is actually idle. Control-group badges dock on the right."""
        self.buttons = []
        sf = self.ctx["sfont"]
        mouse = self.mouse_pos or (-1, -1)      # CANVAS space (window get_pos misfires)
        n_idle = sum(1 for u in self.units if u.team == 0 and u.worker and u.alive
                     and u.state == "idle" and u.goal is None and u.target is None
                     and u.carrying == 0)
        defs = [("Army", "army", True), ("Scribes", "workers", True),
                (f"Idle {n_idle}", "idle", n_idle > 0), ("Base", "base", True)]
        x = 12; y = self.H - 118 - 32
        for label, arg, present in defs:
            pres = self._ink_t("qb_" + arg, present)
            if pres <= 0.001:
                continue
            rect = pygame.Rect(x, y, 76, 26)
            hover = rect.collidepoint(mouse)
            hp_ = self._ink_t("qbh_" + arg, hover)
            idle_glow = arg == "idle"
            # rest state: a quiet outline; hover: the chip fills with ink
            w = int(rect.w * (0.9 + 0.1 * pres)); h = int(rect.h * pres)
            r = pygame.Rect(rect.centerx - w // 2, rect.bottom - h, w, max(2, h))
            fill_a = int(40 + 185 * hp_)
            base = (150, 120, 55) if idle_glow else (52, 46, 38)
            chip = pygame.Surface((r.w, r.h), pygame.SRCALPHA)
            pygame.draw.rect(chip, (*base, fill_a), chip.get_rect(), border_radius=7)
            pygame.draw.rect(chip, (220, 180, 90) if idle_glow else (128, 112, 86),
                             chip.get_rect(), 1, border_radius=7)
            screen.blit(chip, r.topleft)
            if pres > 0.5:
                col = (246, 236, 214) if hover else (206, 196, 178)
                screen.blit(sf.render(label, True, col),
                            (rect.centerx - sf.size(label)[0] / 2, rect.y + 5))
            self.buttons.append((rect, ("quick", arg)))
            x += 82
        # ── control-group badges: pop in as groups come into being ──
        bx = self.W - 292
        for n in range(1, 10):
            g = self._group(n)
            t = self._ink_t(f"grp_{n}", bool(g))
            if t <= 0.001:
                continue
            rect = pygame.Rect(bx + (n - 1) * 30, y, 26, 26)
            hover = rect.collidepoint(mouse)
            hh = int(rect.h * t)
            r = pygame.Rect(rect.x, rect.bottom - hh, rect.w, max(2, hh))
            badge = pygame.Surface((r.w, r.h), pygame.SRCALPHA)
            pygame.draw.rect(badge, (48, 42, 56, 235 if hover else 175),
                             badge.get_rect(), border_radius=6)
            pygame.draw.rect(badge, (170, 150, 210), badge.get_rect(), 1, border_radius=6)
            screen.blit(badge, r.topleft)
            if t > 0.5:
                screen.blit(sf.render(str(n), True, (236, 228, 246)), (rect.x + 5, rect.y + 4))
                cnt = sf.render(str(len(g)), True, (176, 160, 200))
                screen.blit(cnt, (rect.right - cnt.get_width() - 3, rect.bottom - 13))
            self.buttons.append((rect, ("group", n)))

    def _draw_wheel(self, screen):
        w = self.wheel
        cx, cy = w["x"], w["y"]
        n = len(w["slices"])
        R = 92; r0 = 26
        # dim veil
        veil = pygame.Surface((self.W, self.H), pygame.SRCALPHA); veil.fill((10, 8, 6, 90))
        screen.blit(veil, (0, 0))
        sf = self.ctx["sfont"]
        for i, (label, action) in enumerate(w["slices"]):
            a0 = i / n * math.tau; a1 = (i + 1) / n * math.tau
            mid = (a0 + a1) / 2
            hot = (i == self.wheel_hot)
            col = (120, 104, 74) if hot else (52, 44, 36)
            # slice as a ring wedge (approximate with a polygon fan)
            pts = [(cx + math.cos(a0) * r0, cy + math.sin(a0) * r0)]
            steps = 8
            for s in range(steps + 1):
                a = a0 + (a1 - a0) * s / steps
                pts.append((cx + math.cos(a) * R, cy + math.sin(a) * R))
            pts.append((cx + math.cos(a1) * r0, cy + math.sin(a1) * r0))
            pygame.draw.polygon(screen, col, pts)
            pygame.draw.polygon(screen, (150, 132, 100), pts, 2)
            lx = cx + math.cos(mid) * (R * 0.66); ly = cy + math.sin(mid) * (R * 0.66)
            lbl = sf.render(label, True, (245, 238, 222) if hot else (210, 200, 184))
            screen.blit(lbl, (lx - lbl.get_width() / 2, ly - lbl.get_height() / 2))
        pygame.draw.circle(screen, (30, 26, 22), (int(cx), int(cy)), r0)
        pygame.draw.circle(screen, (150, 132, 100), (int(cx), int(cy)), r0, 2)
        x = sf.render("×", True, (220, 200, 180))
        screen.blit(x, (cx - x.get_width() / 2, cy - x.get_height() / 2))

    def _build_weather_overlay(self):
        """Pre-render a lighting grade for the map's weather: a tint wash plus a
        directional glow (sun) or an even gloom (overcast/storm)."""
        w = self.weather
        tint = tuple(w.get("tint", [255, 245, 220])); strg = w.get("strength", 0.1)
        ov = pygame.Surface((self.W, self.H - 118), pygame.SRCALPHA)
        if w["kind"] == "clear":
            ov.fill((*tint, int(strg * 90)))
            glow = pygame.Surface((self.W, self.H - 118), pygame.SRCALPHA)
            for i in range(5):
                rr = int(self.W * (0.5 - i * 0.06))
                pygame.draw.circle(glow, (255, 244, 210, 16), (int(self.W * 0.28), int(self.H * 0.16)), rr)
            ov.blit(glow, (0, 0))
        elif w["kind"] == "overcast":
            ov.fill((*tint, int(strg * 150)))
        elif w["kind"] == "mist":
            ov.fill((*tint, int(strg * 130)))
        elif w["kind"] == "storm":
            ov.fill((*tint, int(strg * 170)))
            for i in range(8):
                a = 10 + i * 8
                pygame.draw.rect(ov, (20, 22, 40, a),
                                 (i * 4, i * 4, self.W - i * 8, self.H - 118 - i * 8), 3)
        self._weather_ov = ov

    def _draw_weather(self, screen):
        """Blit the lighting grade + animated precipitation over the world."""
        ov = getattr(self, "_weather_ov", None)
        if ov is not None:
            screen.blit(ov, (0, 0))
        w = self.weather; tnow = getattr(self, "_anim_t", 0.0)
        if w["kind"] == "storm":
            for dp in self.wdrops:
                y = (dp["y"] + tnow * 600 * dp["sp"]) % (self.H - 118)
                x = (dp["x"] + y * 0.4) % self.W
                pygame.draw.line(screen, (150, 160, 190), (x, y), (x - 6, y + 14), 1)
            if math.sin(tnow * 0.7) > 0.985:
                fl = pygame.Surface((self.W, self.H - 118), pygame.SRCALPHA)
                fl.fill((220, 225, 255, 60)); screen.blit(fl, (0, 0))
        elif w["kind"] == "mist":
            for dp in self.wdrops[:40]:
                x = (dp["x"] + tnow * 20 * dp["sp"]) % self.W
                y = dp["y"] % (self.H - 118) + math.sin(tnow * 0.5 + dp["ph"]) * 8
                puff = pygame.Surface((60, 30), pygame.SRCALPHA)
                pygame.draw.ellipse(puff, (210, 225, 205, 16), (0, 0, 60, 30))
                screen.blit(puff, (int(x - 30), int(y - 15)))

    def _build_paper_tile(self):
        """Pre-render a seamless paper-fibre texture tile once, tinted to the map
        theme. Tiled under the world for a warm handmade-paper surface."""
        import random
        g = self.theme["ground"]
        sz = 128
        tile = pygame.Surface((sz, sz))
        tile.fill(tuple(g))
        rng = random.Random(self.mapdef.get("seed", 1))
        # speckle of slightly lighter/darker fibres
        for _ in range(900):
            x = rng.randrange(sz); y = rng.randrange(sz)
            d = rng.randint(-14, 12)
            tile.set_at((x, y), tuple(max(0, min(255, c + d)) for c in g))
        # a few short fibre streaks
        for _ in range(60):
            x = rng.randrange(sz); y = rng.randrange(sz); ln = rng.randint(3, 9)
            col = tuple(max(0, min(255, c + rng.randint(-10, 8))) for c in g)
            pygame.draw.line(tile, col, (x, y), (x + ln, y + rng.randint(-1, 1)))
        self._paper_tile = tile
        # a soft screen vignette (built once)
        self._vignette = pygame.Surface((self.W, self.H), pygame.SRCALPHA)
        for i in range(6):
            a = 8 + i * 6
            pygame.draw.rect(self._vignette, (20, 14, 8, a),
                             (i * 3, i * 3, self.W - i * 6, self.H - 118 - i * 6), 2)

    def _draw_grid(self, screen):
        """Themed paper terrain: tiled fibre texture, decorative ink stains, faint
        ruled lines (notebook feel), a wider grid, and long fold-creases."""
        th = self.theme
        hcut = self.H - 118
        # tile the pre-rendered paper texture, offset by the camera
        tile = getattr(self, "_paper_tile", None)
        if tile is not None:
            tsz = int(128 * self.zoom)
            if tsz >= 8:
                scaled = pygame.transform.scale(tile, (tsz, tsz)) if abs(self.zoom - 1) > 0.01 else tile
                ox = int(-(self.cam_x * self.zoom) % tsz) - tsz
                oy = int(-(self.cam_y * self.zoom) % tsz) - tsz
                for x in range(ox, self.W + tsz, tsz):
                    for y in range(oy, hcut + tsz, tsz):
                        screen.blit(scaled, (x, y))
        # authored fold-valley bands: soft shaded terrain sweeps with a lit edge,
        # giving each map hand-crafted contour rather than flat ground
        gcol = th["ground"]
        shade = tuple(max(0, int(c * 0.93)) for c in gcol)
        hilite = tuple(min(255, int(c + 14)) for c in gcol)
        for (x1, y1, x2, y2, bw) in self.bands:
            a = self.w2s(x1, y1); b = self.w2s(x2, y2)
            w2 = max(2, int(bw * self.zoom))
            # the shaded valley floor
            pygame.draw.line(screen, shade, a, b, w2)
            # a lit fold-crease highlight along one edge
            dx, dy = b[0] - a[0], b[1] - a[1]
            dl = math.hypot(dx, dy) or 1
            nx, ny = -dy / dl * w2 * 0.42, dx / dl * w2 * 0.42
            pygame.draw.line(screen, hilite, (a[0] + nx, a[1] + ny), (b[0] + nx, b[1] + ny), 1)
        # decorative ink stains (world-space blots)
        for (bx, by, br, alpha) in self.stains:
            sx, sy = self.w2s(bx, by)
            rr = int(br * self.zoom)
            if rr > 1 and -rr < sx < self.W + rr and -rr < sy < hcut + rr:
                blot = pygame.Surface((rr * 2, rr * 2), pygame.SRCALPHA)
                col = tuple(th.get("accent", [150, 120, 80]))
                pygame.draw.circle(blot, (*col, int(alpha * 255)), (rr, rr), rr)
                pygame.draw.circle(blot, (*col, int(alpha * 160)), (int(rr * 1.2), int(rr * 0.7)),
                                   max(1, int(rr * 0.5)))
                screen.blit(blot, (int(sx - rr), int(sy - rr)))
        grid = tuple(th["grid"]); ruled = tuple(th["ruled"]); crease = tuple(th["crease"])
        # close-set ruled lines (horizontal), the notebook feel
        rstep = 46
        ry = -(self.cam_y % rstep) * self.zoom
        rline = tuple(int(g * 0.7 + r * 0.3) for g, r in zip(th["ground"], ruled))
        while ry < hcut:
            pygame.draw.line(screen, rline, (0, ry), (self.W, ry))
            ry += rstep * self.zoom
        # red margin line down the left of the "page"
        margin_x = self.w2s(120, 0)[0]
        if 0 < margin_x < self.W:
            pygame.draw.line(screen, tuple(th["ruled"]), (margin_x, 0), (margin_x, hcut), 2)
        # wider structural grid
        step = 200
        x = -(self.cam_x % step) * self.zoom
        while x < self.W:
            pygame.draw.line(screen, grid, (x, 0), (x, hcut))
            x += step * self.zoom
        y = -(self.cam_y % step) * self.zoom
        while y < hcut:
            pygame.draw.line(screen, grid, (0, y), (self.W, y))
            y += step * self.zoom
        # long fold-creases across the whole map (fixed in world space)
        for (fx, fy, gx, gy) in [(0, WORLD_H * 0.33, WORLD_W, WORLD_H * 0.42),
                                 (WORLD_W * 0.2, 0, WORLD_W * 0.32, WORLD_H)]:
            a = self.w2s(fx, fy); b = self.w2s(gx, gy)
            pygame.draw.line(screen, crease, a, b, 2)
            pygame.draw.line(screen, tuple(min(255, c + 18) for c in th["ground"]),
                             (a[0], a[1] + 2), (b[0], b[1] + 2), 1)

    def _draw_nodes(self, screen):
        tnow = getattr(self, "_anim_t", 0.0)
        for n in self.nodes:
            if n.amount <= 0: continue
            sx, sy = self.wpos(n.x, n.y)
            if -40 < sx < self.W + 40 and -40 < sy < self.H + 40:
                rr = int(n.r * self.zoom)
                # a well rim, an ink pool, and a slow shimmer ripple — flat on
                # the page, so foreshortened to ellipses by the iso view
                self.disc(screen, (36, 32, 58), n.x, n.y, n.r + 2 / self.zoom)
                self.disc(screen, (54, 48, 88), n.x, n.y, n.r, 2)
                f = max(0.15, n.amount / 2000.0)
                pool = int(rr * f)
                self.disc(screen, (78, 68, 130), n.x, n.y, n.r * f)
                self.disc(screen, (120, 108, 190), n.x, n.y, max(2 / self.zoom, n.r * f * 0.7))
                # shimmer highlight that drifts
                if self.zoom > 0.5 and pool > 4:
                    hx = sx + math.cos(tnow * 1.3 + n.x) * pool * 0.4
                    hy = sy + math.sin(tnow * 1.1 + n.x) * pool * 0.3 * ISO
                    pygame.draw.circle(screen, (170, 160, 220), (int(hx), int(hy)), max(1, pool // 4))
                    # rising ink bubbles that surface and pop (procedural, timed)
                    for b in range(3):
                        ph = (tnow * 0.8 + b * 0.33 + n.x * 0.01) % 1.0
                        ba = int(200 * (1 - ph))
                        bx = sx + math.sin((ph + b) * 5 + n.x) * pool * 0.35
                        by = sy + pool * ISO * 0.4 - ph * pool * 0.9
                        bub = pygame.Surface((6, 6), pygame.SRCALPHA)
                        pygame.draw.circle(bub, (150, 138, 210, ba), (3, 3), max(1, int(pool * 0.12)))
                        screen.blit(bub, (int(bx - 3), int(by - 3)))

    def _update_fog(self, force=False):
        """Recompute the player's vision: mark cells visible within sight radius
        of any team-0 unit or structure. Previously-seen cells stay 'explored'
        (dimmed) rather than reverting to black."""
        if not self.fog_on or self.fog is None:
            return
        for row in self.fog:
            for c in range(len(row)):
                if row[c] == 2:
                    row[c] = 1
        cell = self.fog_cell
        def reveal(wx, wy, sight, on_high):
            cx = int(wx // cell); cy = int(wy // cell)
            rad = int(sight // cell) + 1
            for gy in range(max(0, cy - rad), min(self.fog_rows, cy + rad + 1)):
                for gx in range(max(0, cx - rad), min(self.fog_cols, cx + rad + 1)):
                    if (gx - cx) ** 2 + (gy - cy) ** 2 <= rad * rad:
                        # low-ground viewers can't see ONTO high ground (the SC
                        # "you can't see up the ramp" rule) beyond a short peek
                        if not on_high and self._on_high(gx * cell, gy * cell):
                            if (gx - cx) ** 2 + (gy - cy) ** 2 > (rad * 0.4) ** 2:
                                continue
                        self.fog[gy][gx] = 2
        for u in self.units:
            if u.alive and u.team == 0:
                oh = self._on_high(u.x, u.y)
                base_sight = (260 if not u.worker else 200) * (1.4 if oh else 1.0)
                reveal(u.x, u.y, base_sight, oh)
        for s in self.structs:
            if s.team == 0 and s.complete:
                oh = self._on_high(s.x, s.y)
                reveal(s.x, s.y, (340 if s.kind == "spire" else 300) * (1.3 if oh else 1.0), oh)

    def _cell_state(self, wx, wy):
        if not self.fog_on or self.fog is None:
            return 2
        gx = int(wx // self.fog_cell); gy = int(wy // self.fog_cell)
        if 0 <= gy < self.fog_rows and 0 <= gx < self.fog_cols:
            return self.fog[gy][gx]
        return 0

    def _visible(self, ent):
        return self._cell_state(ent.x, ent.y) == 2

    def _draw_fog(self, screen):
        """Darken the world by vision state: unexplored cells fully black,
        explored-but-not-visible dimmed. Only the on-screen cells are drawn."""
        cell = self.fog_cell
        # world rect currently on screen
        wx0, wy0 = self.s2w(0, 0)
        wx1, wy1 = self.s2w(self.W, self.H - 118)
        gx0 = max(0, int(wx0 // cell) - 1); gx1 = min(self.fog_cols, int(wx1 // cell) + 2)
        gy0 = max(0, int(wy0 // cell) - 1); gy1 = min(self.fog_rows, int(wy1 // cell) + 2)
        csz = int(cell * self.zoom) + 2
        veil = pygame.Surface((self.W, self.H), pygame.SRCALPHA)
        for gy in range(gy0, gy1):
            for gx in range(gx0, gx1):
                st = self.fog[gy][gx]
                if st == 2:
                    continue
                sx, sy = self.w2s(gx * cell, gy * cell)
                alpha = 235 if st == 0 else 110    # black unseen, dim explored
                pygame.draw.rect(veil, (8, 8, 12, alpha), (int(sx), int(sy), csz, csz))
        screen.blit(veil, (0, 0))

    def _draw_spells(self, screen):
        for sp in self.spells:
            sx, sy = self.w2s(sp["x"], sp["y"])
            rad = int(sp["ab"]["radius"] * self.zoom)
            kind = sp["kind"]
            pulse = 0.5 + 0.5 * math.sin(sp["t"] * 8)
            surf = pygame.Surface((rad * 2 + 4, rad * 2 + 4), pygame.SRCALPHA)
            if kind == "storm":
                # crackling fiery storm
                pygame.draw.circle(surf, (230, 110, 40, int(70 + 60 * pulse)), (rad + 2, rad + 2), rad)
                for _ in range(6):
                    a = random.uniform(0, math.tau); rr = random.uniform(0, rad)
                    ex = rad + 2 + math.cos(a) * rr; ey = rad + 2 + math.sin(a) * rr
                    pygame.draw.circle(surf, (250, 200, 80, 200), (int(ex), int(ey)), max(1, rad // 12))
            elif kind == "heal":
                pygame.draw.circle(surf, (120, 220, 150, int(60 + 40 * pulse)), (rad + 2, rad + 2), rad)
                pygame.draw.circle(surf, (150, 240, 170, 160), (rad + 2, rad + 2), rad, 2)
            elif kind == "snare":
                pygame.draw.circle(surf, (70, 140, 200, int(70 + 50 * pulse)), (rad + 2, rad + 2), rad)
                pygame.draw.circle(surf, (120, 190, 230, 170), (rad + 2, rad + 2), rad, 2)
            screen.blit(surf, (int(sx - rad - 2), int(sy - rad - 2)))

    def _draw_cast_ghost(self, screen):
        ability, casters = self.cast_mode
        mx, my = pygame.mouse.get_pos()
        rad = int(ability["radius"] * self.zoom)
        col = {"storm": (230, 120, 50), "heal": (120, 220, 150),
               "snare": (80, 150, 210)}.get(ability["kind"], (220, 220, 120))
        pygame.draw.circle(screen, col, (mx, my), rad, 2)
        # a line from the nearest caster + its cast-range ring
        if casters:
            c = min(casters, key=lambda c: (c.x - self.s2w(mx, my)[0]) ** 2)
            cx, cy = self.w2s(c.x, c.y)
            pygame.draw.circle(screen, (*col, 60)[:3], (int(cx), int(cy)),
                               int(ability["cast_range"] * self.zoom), 1)

    def _draw_vignette(self, screen):
        """A soft darkened border framing the play area — cheap, cached."""
        vg = getattr(self, "_vignette_surf", None)
        if vg is None or vg.get_width() != self.W:
            vg = pygame.Surface((self.W, self.H - 118), pygame.SRCALPHA)
            edge = 90
            for i in range(edge):
                a = int(70 * (1 - i / edge) ** 2)
                pygame.draw.rect(vg, (20, 16, 12, a),
                                 (i, i, self.W - 2 * i, (self.H - 118) - 2 * i), 1)
            self._vignette_surf = vg
        screen.blit(vg, (0, 0))

    def _draw_highground(self, screen):
        """Raised plateaus seen at the page's angle: a foreshortened elliptical
        top LIFTED off the ground, a visible cliff wall filling the gap, and any
        authored ramps drawn as shaded runs up the face."""
        g = self.theme["ground"]
        top = tuple(min(255, int(c * 1.12)) for c in g)
        cliff = tuple(int(c * 0.72) for c in g)
        cliff_lit = tuple(int(c * 0.86) for c in g)
        rim = tuple(int(c * 0.92) for c in g)
        lift = HG_LIFT * self.zoom
        for hx, hy, hr in self._high:
            sx, sy = self.w2s(hx, hy)
            rr = int(hr * self.zoom)
            ry = max(1, int(rr * ISO))
            if not (-rr < sx < self.W + rr and -ry - lift < sy < self.H + ry):
                continue
            # cliff wall: the band between the ground footprint and the lifted
            # top — a rect capped by the bottom halves of the two ellipses.
            wall = pygame.Rect(int(sx - rr), int(sy - lift), rr * 2, int(lift) + ry)
            pygame.draw.rect(screen, cliff, (wall.x, int(sy - lift), wall.w, int(lift)))
            pygame.draw.ellipse(screen, cliff,
                                (int(sx - rr), int(sy - ry), rr * 2, ry * 2))
            # a lit sliver down the sunward (left) face of the wall
            pygame.draw.rect(screen, cliff_lit,
                             (wall.x, int(sy - lift), max(2, rr // 6), int(lift)))
            # the lifted plateau top
            pygame.draw.ellipse(screen, top,
                                (int(sx - rr), int(sy - lift - ry), rr * 2, ry * 2))
            pygame.draw.ellipse(screen, rim,
                                (int(sx - rr), int(sy - lift - ry), rr * 2, ry * 2), 2)
            # crease marks around the top rim
            if self.zoom > 0.5:
                for k in range(8):
                    a = k / 8 * math.tau
                    ex = sx + math.cos(a) * (rr - 6)
                    ey = sy - lift + math.sin(a) * (ry - 4)
                    pygame.draw.circle(screen, rim, (int(ex), int(ey)), 2)
        # ── ramps: a folded paper run from the page up onto the plateau ──
        for (rx, ry_, rw, rl, ddx, ddy) in self._ramps:
            # foot edge (on the ground) and top edge (lifted, at the plateau)
            px, py = -ddy, ddx                       # across-axis
            f0 = self.w2s(rx - px * rw / 2, ry_ - py * rw / 2)
            f1 = self.w2s(rx + px * rw / 2, ry_ + py * rw / 2)
            tx, ty = rx + ddx * rl, ry_ + ddy * rl
            t0 = self.w2s(tx - px * rw / 2, ty - py * rw / 2)
            t1 = self.w2s(tx + px * rw / 2, ty + py * rw / 2)
            t0 = (t0[0], t0[1] - lift)
            t1 = (t1[0], t1[1] - lift)
            if not any(-80 < x < self.W + 80 and -80 < y < self.H + 80
                       for x, y in (f0, f1, t0, t1)):
                continue
            # slope tone: ramps facing the light (up/left) read brighter
            face = 1.06 if (ddy < 0 or ddx < 0) else 0.9
            slope = tuple(min(255, int(c * face)) for c in g)
            pygame.draw.polygon(screen, slope, [f0, f1, t1, t0])
            pygame.draw.polygon(screen, rim, [f0, f1, t1, t0], 2)
            # fold creases across the run
            if self.zoom > 0.5:
                for k in (0.25, 0.5, 0.75):
                    a0 = (f0[0] + (t0[0] - f0[0]) * k, f0[1] + (t0[1] - f0[1]) * k)
                    a1 = (f1[0] + (t1[0] - f1[0]) * k, f1[1] + (t1[1] - f1[1]) * k)
                    pygame.draw.line(screen, tuple(int(c * 0.88) for c in g), a0, a1, 1)

    def _draw_objects(self, screen):
        for o in self.objects:
            sx, sy = self.wpos(o.x, o.y)
            rr = int(o.r * self.zoom)
            if not (-rr - 20 < sx < self.W + rr + 20 and -rr - 20 < sy < self.H + rr + 20):
                continue
            if o.kind == "rock":
                if not o.alive:
                    continue
                pts = []
                for k in range(7):
                    a = k / 7 * math.tau
                    rad = rr * (0.85 + 0.2 * math.sin(k * 3.1 + o.x))
                    pts.append((sx + math.cos(a) * rad, sy + math.sin(a) * rad))
                pygame.draw.polygon(screen, o.color, pts)
                pygame.draw.polygon(screen, tuple(int(c * 0.6) for c in o.color), pts, 2)
                frac = o.hp / o.maxhp
                for c2 in range(1 + int((1 - frac) * 3)):
                    a = (c2 * 2.1 + o.x) % math.tau
                    pygame.draw.line(screen, (40, 36, 30), (sx, sy),
                                     (sx + math.cos(a) * rr * 0.8, sy + math.sin(a) * rr * 0.8), 1)
                if frac < 1.0:
                    self._hp_bar(screen, sx, sy - rr - 6, rr * 1.6, frac, 0)
                if rr > 14:
                    lbl = self.ctx["sfont"].render(o.label, True, (60, 52, 40))
                    screen.blit(lbl, (sx - lbl.get_width() / 2, sy + rr + 2))
                continue
            if o.kind == "watchtower":
                own = o.owner
                ring = self._tcol(own) if own is not None else (170, 160, 110)
                if own is not None and self.zoom > 0.4:
                    asz = int(o.sight * self.zoom)
                    aura = pygame.Surface((asz * 2, asz * 2), pygame.SRCALPHA)
                    pygame.draw.circle(aura, (*ring, 22), (asz, asz), asz)
                    screen.blit(aura, (int(sx - asz), int(sy - asz)))
                pygame.draw.circle(screen, (60, 54, 40), (int(sx), int(sy)), rr + 2)
                pygame.draw.circle(screen, o.color, (int(sx), int(sy)), rr)
                pygame.draw.circle(screen, ring, (int(sx), int(sy)), rr + 2, 3)
                pygame.draw.line(screen, (90, 80, 60), (sx, sy - rr), (sx, sy - rr * 1.6), 2)
                pygame.draw.circle(screen, ring, (int(sx), int(sy - rr * 1.6)), max(2, rr // 4))
                if rr > 14:
                    lbl = self.ctx["sfont"].render(o.label, True, (60, 52, 40))
                    screen.blit(lbl, (sx - lbl.get_width() / 2, sy + rr + 2))
                continue
            if o.kind in ("ruin", "obstacle"):
                # a crumpled paper mound — irregular blocking shape
                pts = []
                for k in range(8):
                    a = k / 8 * math.tau
                    rad = rr * (0.8 + 0.25 * math.sin(k * 2.3 + o.x))
                    pts.append((sx + math.cos(a) * rad, sy + math.sin(a) * rad))
                pygame.draw.polygon(screen, o.color, pts)
                pygame.draw.polygon(screen, tuple(int(c * 0.7) for c in o.color), pts, 2)
                # a couple of fold creases
                pygame.draw.line(screen, tuple(int(c * 0.8) for c in o.color),
                                 (sx - rr * 0.5, sy), (sx + rr * 0.5, sy - rr * 0.2), 1)
            elif o.kind == "relic":
                # a glowing seal; ring tinted by current owner
                own = o.owner
                ring = self._tcol(own) if own is not None else (150, 140, 120)
                pygame.draw.circle(screen, (40, 34, 26), (int(sx), int(sy)), rr + 3)
                pygame.draw.circle(screen, o.color, (int(sx), int(sy)), rr)
                pygame.draw.circle(screen, ring, (int(sx), int(sy)), rr + 3, 3)
                st = self.ctx["sfont"].render("✦", True, (60, 50, 30))
                screen.blit(st, (sx - st.get_width() / 2, sy - st.get_height() / 2))
            elif o.kind == "decoration":
                pygame.draw.circle(screen, o.color, (int(sx), int(sy)), rr, 2)
            # richnode is drawn as a real InkNode already; label big ones
            if rr > 16 and o.kind in ("ruin", "relic", "richnode"):
                lbl = self.ctx["sfont"].render(o.label, True, (60, 52, 40))
                screen.blit(lbl, (sx - lbl.get_width() / 2, sy + rr + 2))

    def _draw_struct(self, screen, s):
        sx, sy = self.wpos(s.x, s.y)
        if not (-80 < sx < self.W + 80 and -80 < sy < self.H + 80):
            return
        col = self._tcol(s.team); dark = self._tdark(s.team)
        light = tuple(min(255, int(c * 1.25)) for c in col)
        r = int(s.r * self.zoom)
        tnow = getattr(self, "_anim_t", 0.0)
        # soft cast shadow on the ground
        sh = pygame.Surface((r * 2 + 6, r + 6), pygame.SRCALPHA)
        pygame.draw.ellipse(sh, (0, 0, 0, 70), (0, 0, r * 2 + 6, r + 6))
        screen.blit(sh, (int(sx - r - 3), int(sy + r * 0.2)))
        if not s.complete:
            # rising construction: the finished silhouette unfolds upward from a
            # flat footprint, with folding-panel lines and a scaffold shimmer
            f = 1.0 - s.build_left / s.build_total
            ease = f * f * (3 - 2 * f)               # smoothstep
            # flat footprint slab always shown
            pygame.draw.ellipse(screen, dark, (int(sx - r), int(sy), r * 2, int(r * 0.5)))
            # the structure body clipped to a rising height via a squash surface
            bh = max(2, int(r * 1.6 * ease))
            panel = pygame.Surface((r * 2 + 8, bh + 4), pygame.SRCALPHA)
            # draw the real body onto the panel, then blit the grown portion
            # (simple approximation: a folded wedge that grows taller)
            top = 4
            pygame.draw.polygon(panel, col,
                                [(4, bh), (r + 4, top), (r * 2 + 4, bh)])
            pygame.draw.polygon(panel, light,
                                [(r + 4, top), (r * 2 + 4, bh), (r + 4, bh)])
            pygame.draw.polygon(panel, dark,
                                [(4, bh), (r + 4, top), (r + 4, bh)])
            # horizontal fold lines that "assemble" as it rises
            for fy in range(top, bh, max(4, int(r * 0.3))):
                pygame.draw.line(panel, (*dark, 160), (6, fy), (r * 2 + 2, fy), 1)
            screen.blit(panel, (int(sx - r - 4), int(sy + r * 0.2 - bh)))
            # scaffold ticks around the rim
            for k in range(6):
                a = k / 6 * math.tau
                pygame.draw.circle(screen, (230, 220, 190),
                                   (int(sx + math.cos(a) * r), int(sy + math.sin(a) * r * 0.5)), 1)
            self._hp_bar(screen, sx, sy - r - 8, r * 1.6, f, s.team)
            return
        # ── a completed structure — each KIND gets its own silhouette so it's
        #    recognisable at a glance, not just by its letter ──
        self._draw_struct_body(screen, s, sx, sy, r, col, dark, light, tnow)
        self._draw_faction_accent(screen, s, sx, sy, r, col, dark, light, tnow)
        self._draw_struct_skin(screen, s, sx, sy, r)
        self._draw_struct_motif(screen, s, sx, sy, r, tnow)
        # damage state: cracks appear as the building is worn down
        frac = s.hp / s.maxhp
        if frac < 0.6 and self.zoom > 0.45:
            import random as _rnd
            rr = _rnd.Random(s.uid)
            n = 3 if frac < 0.3 else 2
            for _ in range(n):
                a = rr.uniform(0, math.tau); ln = r * rr.uniform(0.3, 0.7)
                x0 = sx + math.cos(a) * r * 0.2; y0 = sy - r * 0.2 + math.sin(a) * r * 0.2
                pygame.draw.line(screen, (40, 34, 28),
                                 (x0, y0), (x0 + math.cos(a) * ln, y0 - abs(math.sin(a)) * ln), 1)
            if frac < 0.3:                   # scorch smudge when nearly rubble
                sm = pygame.Surface((r, r), pygame.SRCALPHA)
                pygame.draw.circle(sm, (20, 16, 12, 90), (r // 2, r // 2), r // 2)
                screen.blit(sm, (int(sx - r / 2), int(sy - r / 2)))
        # letter tag
        tag = STRUCTURES[s.kind]["key"] if s.kind in STRUCTURES else self.sspec(s.team, s.kind)["key"]
        lbl = self.ctx["sfont"].render(tag, True, (245, 240, 226))
        screen.blit(lbl, (sx - lbl.get_width() / 2, sy - lbl.get_height() / 2 + r * 0.1))
        # detector pulse ring for the Watch Spire
        if self.sspec(s.team, s.kind).get("detector") and self.zoom > 0.5:
            dr = int(self.sspec(s.team, s.kind).get("detect_range", 300) * self.zoom)
            ring = pygame.Surface((dr * 2 + 4, dr * 2 + 4), pygame.SRCALPHA)
            pygame.draw.circle(ring, (150, 200, 235, 30), (dr + 2, dr + 2), dr, 2)
            screen.blit(ring, (int(sx - dr - 2), int(sy - dr - 2)))
        if s.selected:
            pygame.draw.circle(screen, (240, 230, 120), (int(sx), int(sy)), r + 6, 2)
        self._hp_bar(screen, sx, sy - r - r * 0.55 - 8, r * 1.6, s.hp / s.maxhp, s.team)
        if s.selected and s.rally:
            rx, ry = self.w2s(*s.rally)
            pygame.draw.line(screen, (240, 230, 120), (sx, sy), (rx, ry), 1)
            pygame.draw.circle(screen, (240, 230, 120), (int(rx), int(ry)), 4)

    def _pennant(self, screen, sx, top_y, r, light, dark, uid, tnow):
        pole_top = (sx, top_y)
        pygame.draw.line(screen, (90, 80, 64), (sx, top_y + r * 0.4), pole_top, 2)
        wave = math.sin(tnow * 3 + uid) * r * 0.12
        flag = [(sx, pole_top[1]), (sx + r * 0.5, pole_top[1] + r * 0.12 + wave),
                (sx, pole_top[1] + r * 0.28)]
        pygame.draw.polygon(screen, light, flag)
        pygame.draw.polygon(screen, dark, flag, 1)

    def _draw_struct_skin(self, screen, s, sx, sy, r):
        """The faction's paper 'skin': a subtle pattern texture + a trim line
        over the finished building, so every race's architecture reads at a
        glance. Declared in faction data: skin = {pattern, trim}."""
        if not s.complete or self.zoom < 0.45:
            return
        skin = self.facdata(s.team).get("skin")
        if not skin:
            # core factions get a derived skin from their mechanic, so every
            # race is textured even without explicit data
            skin = {"cohesion": {"pattern": "hatch", "trim": (214, 178, 108)},
                    "burn":     {"pattern": "scale", "trim": (235, 140, 70)},
                    "regen":    {"pattern": "weave", "trim": (110, 200, 196)},
                    "momentum": {"pattern": "dots",  "trim": (208, 196, 150)},
                    }.get(self.facdata(s.team).get("mechanic"))
            if not skin:
                return
        pat = skin.get("pattern", "")
        trim = tuple(skin.get("trim", (200, 200, 200)))
        top = int(sy - r * 1.05)
        body = pygame.Rect(int(sx - r * 0.72), top, int(r * 1.44), int(r * 0.95))
        tex = pygame.Surface(body.size, pygame.SRCALPHA)
        w, h = body.size
        if pat == "dots":                    # gilded wax cells
            step = max(6, r // 4)
            for yy in range(step // 2, h, step):
                for xx in range((yy // step % 2) * step // 2, w, step):
                    pygame.draw.circle(tex, (*trim, 46), (xx, yy), max(1, step // 5))
        elif pat == "weave":                 # wind-loom cross weave
            step = max(7, r // 3)
            for d in range(-h, w, step):
                pygame.draw.line(tex, (*trim, 34), (d, 0), (d + h, h), 1)
                pygame.draw.line(tex, (*trim, 34), (d + h, 0), (d, h), 1)
        elif pat == "rings":                 # heartwood growth rings
            for k in range(3):
                rr = int(r * (0.35 + k * 0.28))
                pygame.draw.circle(tex, (*trim, 40), (w // 2, h), rr, 1)
        elif pat == "hatch":                 # scribing hatch
            step = max(6, r // 4)
            for d in range(0, w + h, step):
                pygame.draw.line(tex, (*trim, 34), (d, 0), (d - h, h), 1)
        elif pat == "scale":                 # overlapping folded scales
            step = max(8, r // 3)
            for yy in range(0, h, step):
                for xx in range((yy // step % 2) * step // 2, w, step):
                    pygame.draw.arc(tex, (*trim, 44),
                                    (xx, yy, step, step), 0, math.pi, 1)
        screen.blit(tex, body.topleft)
        # trim: a fine faction-coloured line along the roof edge
        pygame.draw.line(screen, trim, (body.left + 2, body.top + 2),
                         (body.right - 2, body.top + 2), 1)

    def _draw_struct_motif(self, screen, s, sx, sy, r, tnow):
        """A per-faction visual signature over each building — procedural (driven
        by time, no sim state) so factions read distinctly at a glance."""
        if not s.complete or self.zoom < 0.4:
            return
        fk = self.fac[s.team]
        if fk == "swarm":
            # a ring of gilded mites orbiting the building
            for k in range(4):
                a = tnow * (1.2 + k * 0.13) + k * math.tau / 4 + s.uid
                mx = sx + math.cos(a) * r * 1.15
                my = sy - r * 0.5 + math.sin(a) * r * 0.5
                pygame.draw.circle(screen, (222, 186, 96), (int(mx), int(my)), 2)
                pygame.draw.circle(screen, (150, 118, 52), (int(mx), int(my)), 2, 1)
            return
        if fk == "windwrit":
            # wind streamers trailing off the roof, rippling
            for k in range(2):
                base_x = sx + (k * 2 - 1) * r * 0.45
                base_y = sy - r * 1.05
                pts = []
                for i in range(6):
                    t2 = i / 5
                    pts.append((base_x + t2 * r * 0.9 * (k * 2 - 1) * -1
                                + math.sin(tnow * 5 + i + s.uid) * 2,
                                base_y - t2 * r * 0.25
                                + math.sin(tnow * 6 + i * 1.4 + k * 2) * 3 * t2))
                pygame.draw.lines(screen, (150, 190, 226), False, pts, 2)
            return
        if fk == "rootbound":
            # leaf sprigs slowly unfurling around the base
            for k in range(3):
                ph = (tnow * 0.25 + k * 0.33 + s.uid * 0.07) % 1.0
                a = k * 2.1 + s.uid
                lx = sx + math.cos(a) * r * 0.95
                ly = sy + math.sin(a) * r * 0.4 * ISO + r * 0.15
                gl = min(1.0, ph * 2)
                tip = (lx + math.cos(a - 0.6) * 7 * gl, ly - 9 * gl)
                pygame.draw.line(screen, (110, 140, 84), (lx, ly), tip, 2)
                pygame.draw.circle(screen, (156, 186, 116),
                                   (int(tip[0]), int(tip[1])), max(1, int(3 * gl)))
            return
        mech = self.facdata(s.team).get("mechanic")
        if mech == "burn":
            glow = pygame.Surface((r * 3, r * 3), pygame.SRCALPHA)
            ga = int(40 + 22 * math.sin(tnow * 4 + s.uid))
            pygame.draw.circle(glow, (240, 130, 50, ga), (int(r * 1.5), int(r * 1.5)), int(r * 1.2))
            screen.blit(glow, (int(sx - r * 1.5), int(sy - r * 1.5)), special_flags=pygame.BLEND_ADD)
            for k in range(3):
                ph = (tnow * 0.7 + k * 0.37 + s.uid * 0.11) % 1.0
                ex = sx + math.sin((ph + k) * 6) * r * 0.4
                ey = sy - r * 0.4 - ph * r * 1.4
                a = int(220 * (1 - ph))
                em = pygame.Surface((6, 6), pygame.SRCALPHA)
                pygame.draw.circle(em, (250, 190, 80, a), (3, 3), max(1, int(r * 0.09)))
                screen.blit(em, (int(ex - 3), int(ey - 3)))
        elif mech == "regen":
            ph = (tnow * 0.6 + s.uid * 0.2) % 1.0
            rr = int(r * (0.6 + ph * 0.9))
            a = int(120 * (1 - ph))
            rip = pygame.Surface((rr * 2 + 4, rr * 2 + 4), pygame.SRCALPHA)
            pygame.draw.circle(rip, (110, 200, 220, a), (rr + 2, rr + 2), rr, 2)
            screen.blit(rip, (int(sx - rr - 2), int(sy - rr - 2)))
            sh = pygame.Surface((r, r), pygame.SRCALPHA)
            pygame.draw.circle(sh, (150, 220, 240, 50), (int(r * 0.4), int(r * 0.3)), int(r * 0.3))
            screen.blit(sh, (int(sx - r * 0.5), int(sy - r * 0.7)))
        elif mech == "cohesion":
            hl = int(180 + 40 * math.sin(tnow * 2 + s.uid))
            pygame.draw.line(screen, (min(255, hl + 40),) * 3,
                             (sx, sy - r * 0.9), (sx, sy - r * 0.2), 1)
        elif mech == "momentum":
            # a slowly turning gilded gear glint sweeping around the building
            a = (tnow * 1.4 + s.uid) % math.tau
            gx = sx + math.cos(a) * r * 0.7; gy = sy - r * 0.2 + math.sin(a) * r * 0.4
            gl = pygame.Surface((int(r * 0.6), int(r * 0.6)), pygame.SRCALPHA)
            pygame.draw.circle(gl, (240, 210, 120, 150), (int(r * 0.3), int(r * 0.3)), max(1, int(r * 0.16)))
            screen.blit(gl, (int(gx - r * 0.3), int(gy - r * 0.3)))

    def _draw_struct_body(self, screen, s, sx, sy, r, col, dark, light, tnow):
        kind = s.kind
        def poly(pts, c, edge=None):
            pygame.draw.polygon(screen, c, pts)
            if edge: pygame.draw.polygon(screen, edge, pts, 1)
        if kind == "scriptorium":
            # grand town hall: tall stepped keep with a big pennant
            poly([(sx - r, sy + r * 0.6), (sx - r, sy - r * 0.2), (sx, sy - r * 0.4),
                  (sx, sy + r * 0.7)], dark)
            poly([(sx + r, sy + r * 0.6), (sx + r, sy - r * 0.2), (sx, sy - r * 0.4),
                  (sx, sy + r * 0.7)], col)
            # tall folded roof
            poly([(sx - r, sy - r * 0.2), (sx, sy - r * 0.4), (sx, sy - r * 1.15),
                  (sx - r * 0.55, sy - r * 0.7)], light, dark)
            poly([(sx + r, sy - r * 0.2), (sx, sy - r * 0.4), (sx, sy - r * 1.15),
                  (sx + r * 0.55, sy - r * 0.7)], col, dark)
            self._pennant(screen, sx, sy - r * 1.5, r, light, dark, s.uid, tnow)
        elif kind == "table":
            # barracks: low, wide folded tent
            poly([(sx - r, sy + r * 0.5), (sx - r * 0.6, sy - r * 0.35),
                  (sx + r * 0.6, sy - r * 0.35), (sx + r, sy + r * 0.5)], col, dark)
            poly([(sx - r * 0.6, sy - r * 0.35), (sx, sy - r * 0.6),
                  (sx + r * 0.6, sy - r * 0.35)], light, dark)
            pygame.draw.line(screen, dark, (sx, sy - r * 0.6), (sx, sy + r * 0.5), 1)
        elif kind == "well":
            # reservoir: a round basin brimming with ink
            pygame.draw.circle(screen, dark, (int(sx), int(sy)), r)
            pygame.draw.circle(screen, col, (int(sx), int(sy)), int(r * 0.82))
            pool = int(r * 0.6)
            pygame.draw.circle(screen, (78, 68, 130), (int(sx), int(sy - r * 0.05)), pool)
            pygame.draw.circle(screen, (120, 108, 190), (int(sx - r * 0.15), int(sy - r * 0.15)),
                               max(2, int(pool * 0.4)))
        elif kind == "spire":
            # a tall, narrow watch tower with a pointed cap
            poly([(sx - r * 0.5, sy + r * 0.6), (sx - r * 0.35, sy - r * 0.7),
                  (sx + r * 0.35, sy - r * 0.7), (sx + r * 0.5, sy + r * 0.6)], col, dark)
            poly([(sx - r * 0.35, sy - r * 0.7), (sx, sy - r * 1.3),
                  (sx + r * 0.35, sy - r * 0.7)], light, dark)
            pygame.draw.circle(screen, light, (int(sx), int(sy - r * 1.3)), max(2, int(r * 0.12)))
        elif kind == "bindery":
            # tier-2 tech: a big folded book/ledger — stacked pages
            for i, dy in enumerate((0.5, 0.25, 0.0)):
                shade = [dark, col, light][i]
                poly([(sx - r, sy + r * dy), (sx + r, sy + r * dy),
                      (sx + r * 0.85, sy + r * dy - r * 0.28), (sx - r * 0.85, sy + r * dy - r * 0.28)],
                     shade, dark)
            self._pennant(screen, sx, sy - r * 0.9, r * 0.8, light, dark, s.uid, tnow)
        elif kind == "menagerie":
            # tier-3: an ornate multi-peak hall
            poly([(sx - r, sy + r * 0.6), (sx + r, sy + r * 0.6),
                  (sx + r, sy - r * 0.2), (sx - r, sy - r * 0.2)], col, dark)
            for px in (-0.6, 0.0, 0.6):
                poly([(sx + r * (px - 0.35), sy - r * 0.2), (sx + r * px, sy - r * 0.85),
                      (sx + r * (px + 0.35), sy - r * 0.2)], light if px == 0 else col, dark)
            self._pennant(screen, sx, sy - r * 1.2, r, light, dark, s.uid, tnow)
        elif kind == "reedhut":
            # small round marsh hut with a thatch cap
            pygame.draw.circle(screen, col, (int(sx), int(sy + r * 0.1)), int(r * 0.7))
            poly([(sx - r * 0.75, sy - r * 0.1), (sx, sy - r * 0.8),
                  (sx + r * 0.75, sy - r * 0.1)], dark, dark)
        else:
            # generic folded keep fallback
            poly([(sx - r, sy), (sx, sy + r * 0.7), (sx, sy - r * 0.15), (sx - r, sy - r * 0.4)], dark)
            poly([(sx + r, sy), (sx, sy + r * 0.7), (sx, sy - r * 0.15), (sx + r, sy - r * 0.4)], col)
            poly([(sx - r, sy - r * 0.4), (sx, sy - r * 0.15), (sx, sy - r),
                  (sx - r * 0.5, sy - r * 0.72)], light, dark)
            poly([(sx + r, sy - r * 0.4), (sx, sy - r * 0.15), (sx, sy - r),
                  (sx + r * 0.5, sy - r * 0.72)], col, dark)

    def _draw_faction_accent(self, screen, s, sx, sy, r, col, dark, light, tnow):
        """Structural accents that differ by FACTION, so the same building kind
        reads differently for each side: Ember gets angular forge chimneys with
        heat, Tide gets rounded coral/fin bumps, Court gets crisp fold ridges."""
        if self.zoom < 0.45:
            return
        mech = self.facdata(s.team).get("mechanic")
        if mech == "burn":
            for dx in (-0.45, 0.1):
                cx = sx + r * dx
                pts = [(cx, sy - r * 0.5), (cx + r * 0.14, sy - r * 0.95),
                       (cx + r * 0.28, sy - r * 0.5)]
                pygame.draw.polygon(screen, dark, pts)
                pygame.draw.polygon(screen, (30, 20, 16), pts, 1)
                pygame.draw.circle(screen, (250, 150, 60),
                                   (int(cx + r * 0.14), int(sy - r * 0.95)), max(1, int(r * 0.07)))
        elif mech == "regen":
            for dx in (-0.7, 0.0, 0.7):
                pygame.draw.circle(screen, light,
                                   (int(sx + r * dx), int(sy + r * 0.35)), max(2, int(r * 0.2)))
                pygame.draw.circle(screen, dark,
                                   (int(sx + r * dx), int(sy + r * 0.35)), max(2, int(r * 0.2)), 1)
            fin = [(sx + r * 0.3, sy - r * 0.1), (sx + r * 0.55, sy - r * 0.75),
                   (sx + r * 0.62, sy - r * 0.1)]
            pygame.draw.polygon(screen, light, fin)
            pygame.draw.polygon(screen, dark, fin, 1)
        elif mech == "cohesion":
            for dy in (0.1, 0.35):
                pygame.draw.line(screen, tuple(min(255, c + 30) for c in light),
                                 (sx - r * 0.7, sy + r * dy), (sx, sy + r * (dy - 0.12)), 1)
                pygame.draw.line(screen, tuple(min(255, c + 30) for c in light),
                                 (sx + r * 0.7, sy + r * dy), (sx, sy + r * (dy - 0.12)), 1)
        elif mech == "momentum":
            # printing-press cog teeth jutting from the side + a gilded rim
            for k in range(6):
                a = -0.3 + k * 0.42
                bx = sx + math.cos(a) * r * 0.9; by = sy - r * 0.1 + math.sin(a) * r * 0.5
                pygame.draw.rect(screen, tuple(min(255, c + 20) for c in light),
                                 (int(bx - 2), int(by - 2), 4, 4))
            pygame.draw.arc(screen, (240, 210, 120),
                            (int(sx - r * 0.6), int(sy - r * 0.5), int(r * 1.2), int(r * 1.0)),
                            0.2, 2.9, 2)

    def _draw_dying(self, screen):
        """Render crumpling corpses: the model shrinks, flattens, and fades over
        ~0.6s — a paper unit collapsing into a crushed heap."""
        og = self.ctx.get("origami"); cards = self.ctx.get("cards")
        for d in self.dying:
            sx, sy = self.wpos(d["x"], d["y"])
            if not (-30 < sx < self.W + 30 and -30 < sy < self.H + 30):
                continue
            prog = d["t"] / 0.6
            r = max(3, int(d["r"] * self.zoom * (1.0 - prog * 0.5)))
            tint = self._tcol(d["team"])
            # fade the paper toward the ground colour as it crumples
            fade = tuple(int(tint[i] * (1 - prog) + 150 * prog) for i in range(3))
            if og is not None and cards is not None and self.zoom > 0.42:
                try:
                    card = cards.get(d["card"], {})
                    recipe = og.recipe_for(d["card"], card.get("family", "figure"), card)
                    s = r * 1.55 * (1.0 - prog * 0.4)
                    f = 1 if d["face"] >= 0 else -1
                    squash = 1.0 - prog * 0.7          # flatten downward
                    for part in recipe:
                        ax = part.get("at", [0, 0])[0]; ay = part.get("at", [0, 0])[1]
                        psz = s * part.get("size", 1.0)
                        pcol = tuple(min(255, int(c * part.get("tone", 1.0))) for c in fade)
                        px = sx + ax * s * f
                        py = sy + ay * s * squash - r * 0.2
                        og.draw_part(screen, part.get("part"), px, py, psz, f, pcol,
                                     part.get("h", 0.4) * psz * 0.4, 0.0, 0.0, None)
                    continue
                except Exception:
                    pass
            # fallback: a shrinking faded diamond
            pygame.draw.polygon(screen, fade, [(sx, sy - r), (sx + r, sy), (sx, sy + r), (sx - r, sy)])

    def _draw_unit(self, screen, u):
        sx, sy = self.wpos(u.x, u.y)
        if not (-30 < sx < self.W + 30 and -30 < sy < self.H + 30):
            return
        col = self._tcol(u.team)
        r = max(4, int(u.r * self.zoom))
        # spawn unfold: the paper unit rises from a flat sheet with a little
        # overshoot bounce (squash factor drives the origami vertical scale)
        if u.spawn_t > 0:
            prog = max(0.0, min(1.0, 1.0 - u.spawn_t / 0.35))
            over = math.sin(prog * math.pi) * 0.12
            unfold = prog + over            # 0→~1.12→1
        else:
            unfold = 1.0
        # soft, scaled ground shadow (alpha ellipse)
        shs = pygame.Surface((r * 2 + 6, r + 4), pygame.SRCALPHA)
        pygame.draw.ellipse(shs, (40, 34, 26, 90), (0, 0, r * 2 + 6, r + 4))
        screen.blit(shs, (int(sx - r - 3), int(sy + r * 0.35)))
        drew = False
        # ── origami model: if the host game handed us its renderer, draw the
        #    unit's actual folded-paper card model, tinted to the team colour. ──
        og = self.ctx.get("origami"); cards = self.ctx.get("cards")
        if og is not None and cards is not None and self.zoom > 0.42:
            try:
                self._draw_origami(screen, u, sx, sy, r, col, og, cards, unfold)
                drew = True
            except Exception:
                drew = False
        if not drew:
            rr = max(3, int(r * unfold))
            if u.worker:
                pts = [(sx, sy - rr), (sx + rr, sy), (sx, sy + rr), (sx - rr, sy)]
            else:
                pts = [(sx, sy - rr), (sx + rr, sy + rr), (sx - rr, sy + rr)]
            pygame.draw.polygon(screen, col, pts)
            pygame.draw.polygon(screen, self._tdark(u.team), pts, 2)
        # burning overlay (Ember mechanic): flickering embers over a burning unit
        if getattr(u, "burn", 0) > 0:
            for _ in range(2):
                ex = sx + random.uniform(-r, r); ey = sy + random.uniform(-r, r * 0.5)
                pygame.draw.circle(screen, (240, random.randint(120, 190), 40),
                                   (int(ex), int(ey)), max(1, r // 5))
        if u.carrying > 0:
            # a glowing ink drop bobbing above a laden worker
            gy = int(sy - r - 4 + math.sin(getattr(self, "_anim_t", 0) * 5 + u.uid) * 2)
            gl = pygame.Surface((10, 10), pygame.SRCALPHA)
            pygame.draw.circle(gl, (150, 130, 220, 120), (5, 5), 5)
            screen.blit(gl, (int(sx - 5), gy - 5))
            pygame.draw.circle(screen, (120, 108, 190), (int(sx), gy), 3)
            pygame.draw.circle(screen, (180, 168, 235), (int(sx - 1), gy - 1), 1)
        if u.selected:
            self.disc(screen, (240, 230, 120), u.x, u.y, (r + 5) / self.zoom, 2)
        # cloak shimmer: own cloaked units get a faint shifting ring; a detected
        # enemy cloaked unit gets a red "revealed" ring
        if getattr(u, "cloak", False):
            if u.team == 0:
                # a fading shimmer trail of recent positions
                for ti, (tx, ty) in enumerate(getattr(u, "trail", [])):
                    tsx, tsy = self.w2s(tx, ty)
                    ta = int(60 * (ti + 1) / max(1, len(u.trail)))
                    tr = pygame.Surface((r * 2, r * 2), pygame.SRCALPHA)
                    pygame.draw.circle(tr, (150, 200, 235, ta), (r, r), int(r * 0.7))
                    screen.blit(tr, (int(tsx - r), int(tsy - r)))
                a = int(80 + 60 * math.sin(getattr(self, "_anim_t", 0) * 4 + u.uid))
                sh = pygame.Surface((r * 2 + 8, r * 2 + 8), pygame.SRCALPHA)
                pygame.draw.circle(sh, (150, 200, 235, a), (r + 4, r + 4), r + 3, 2)
                screen.blit(sh, (int(sx - r - 4), int(sy - r - 4)))
            elif self._detected(u):
                pygame.draw.circle(screen, (235, 110, 90), (int(sx), int(sy)), r + 4, 1)
        if u.hp < u.maxhp:
            self._hp_bar(screen, sx, sy - r - 6, r * 1.8, u.hp / u.maxhp, u.team)
        # caster energy bar (blue), just under the hp bar
        if u.max_energy:
            w = max(10, int(r * 1.8)); ex = int(sx - w / 2); ey = int(sy - r - 2)
            pygame.draw.rect(screen, (30, 26, 40), (ex - 1, ey - 1, w + 2, 4))
            pygame.draw.rect(screen, (90, 140, 230), (ex, ey, int(w * u.energy / u.max_energy), 2))
        # veterancy chevrons — small gold pips above ranked units
        vr = getattr(u, "vet_rank", 0)
        if vr > 0:
            for i in range(vr):
                px = int(sx - (vr - 1) * 4 + i * 8)
                py = int(sy - r - 13)
                pygame.draw.polygon(screen, (245, 210, 90),
                                    [(px, py + 3), (px + 3, py - 2), (px + 6, py + 3)])
                pygame.draw.polygon(screen, (120, 90, 20),
                                    [(px, py + 3), (px + 3, py - 2), (px + 6, py + 3)], 1)

    def _draw_origami(self, screen, u, sx, sy, r, tint, og, cards, unfold=1.0):
        """Draw the unit's card model as folded paper at (sx,sy), sized to the
        unit and tinted toward its team colour. `unfold` (0..~1.12) vertically
        squashes the model for the spawn-unfold animation."""
        key = u.card
        card = cards.get(key, {})
        fam = card.get("family", "figure")
        recipe = og.recipe_for(key, fam, card)
        # blend the card's own paper with the team colour so allegiance reads
        paper = card.get("paper", (200, 190, 170))
        base = tuple(int(paper[i] * 0.45 + tint[i] * 0.55) for i in range(3))
        # per-unit variation: each unit gets a slight, stable scale + tint jitter
        # seeded from its uid, so a group of the same type reads as individuals
        vh = (u.uid * 2654435761) & 0xFFFFFFFF
        vscale = 0.92 + ((vh >> 3) & 15) / 15 * 0.16          # 0.92 .. 1.08
        tj = ((vh >> 9) & 15) - 7                              # -7 .. +8
        base = tuple(max(0, min(255, c + tj)) for c in base)
        s = r * 1.55 * vscale               # part scale ~ unit radius
        f = 1 if getattr(u, "face", 1) >= 0 else -1
        tnow = getattr(self, "_anim_t", 0.0)
        moving = 1.0 if u.state in ("move", "toNode", "toBase", "attack") else 0.0
        # attack lunge: atk_anim runs 0.3→0; map to a 0..1 lunge pulse
        atk = getattr(u, "atk_anim", 0.0)
        lunge = math.sin((0.3 - min(0.3, atk)) / 0.3 * math.pi) if atk > 0 else 0.0
        fam_style = {"bird": "flap", "insect": "flicker", "fish": "undulate",
                     "serpent": "undulate", "wisp": "pulse", "tree": "sway",
                     "beast": "", "figure": ""}.get(fam, "")
        anim = dict(t=tnow, move=moving, phase=(u.uid % 17) * 0.37, attack=lunge,
                    breathe=math.sin(tnow * 2.2 + (u.uid % 31) * 0.4),
                    style=fam_style, size=s)
        lean = lunge * r * 0.35 * f
        # gentle idle sway when standing still — a breathing paper wobble
        sway = 0.0
        if moving < 0.5 and lunge == 0:
            sway = math.sin(tnow * 1.6 + u.uid) * r * 0.05
        # worker harvest bob: a rhythmic dip while drawing ink from a well
        bob = 0.0
        if u.worker and u.state == "mining":
            bob = abs(math.sin(tnow * 6 + u.uid)) * r * 0.22
        # directional fold-lighting: parts on the light-facing (upper-left) side
        # are brightened, the far side shaded — makes the fold read as 3-D paper
        for part in recipe:
            pt = part.get("part")
            ax = part.get("at", [0, 0])[0]; ay = part.get("at", [0, 0])[1]
            psz = s * part.get("size", 1.0)
            tone = part.get("tone", 1.0)
            hgt = part.get("h", 0.4) * psz * 0.6 * unfold
            # light comes from upper-left: ax<0 (left) and ay<0 (up) → brighter
            lightf = 1.0 + (-ax * 0.10 - ay * 0.12) * f
            lightf = max(0.82, min(1.18, lightf))
            pcol = tuple(min(255, int(c * tone * lightf)) for c in base)
            px = sx + ax * s * f + lean + sway * (0.4 + ay)
            # unfold squashes vertical offsets; bob dips the whole model
            py = sy + ay * s * unfold - r * 0.3 * unfold + bob
            og.draw_part(screen, pt, px, py, psz, f, pcol, hgt, tnow, 0.0, anim)

    def _hp_bar(self, screen, cx, cy, w, frac, team):
        w = max(10, int(w))
        x = int(cx - w / 2); y = int(cy)
        pygame.draw.rect(screen, (30, 26, 22), (x - 1, y - 1, w + 2, 5))
        col = (120, 200, 130) if frac > 0.5 else (220, 180, 90) if frac > 0.25 else (210, 90, 80)
        pygame.draw.rect(screen, col, (x, y, int(w * max(0, frac)), 3))

    def _draw_box(self, screen):
        x0, y0, x1, y1 = self.box
        r = pygame.Rect(min(x0, x1), min(y0, y1), abs(x1 - x0), abs(y1 - y0))
        s = pygame.Surface((max(1, r.w), max(1, r.h)), pygame.SRCALPHA)
        s.fill((240, 230, 120, 40)); screen.blit(s, r.topleft)
        pygame.draw.rect(screen, (240, 230, 120), r, 1)

    def _draw_ghost(self, screen):
        spec = STRUCTURES[self.build_mode]
        # canvas-space pointer (window-space get_pos() misplaces the preview on
        # scaled windows — the same coordinate bug edge-scroll had)
        mx, my = self.mouse_pos if self.mouse_pos is not None else pygame.mouse.get_pos()
        wx, wy = self.s2w(mx, my)
        ok = not self._placement_blocked(wx, wy, spec["r"])
        r = int(spec["r"] * self.zoom)
        col = (120, 220, 140) if ok else (220, 100, 90)
        gx, gy = self.wpos(wx, wy)          # preview lifts onto high ground
        self.disc(screen, col, wx, wy, spec["r"], 2)
        pygame.draw.polygon(screen, col, [(gx, gy - r), (gx + r, gy),
                                          (gx, gy + r * 0.7), (gx - r, gy)], 1)

    # ── HUD: command palette + resources ──
    def _draw_hud(self, screen):
        H = self.H
        bar = pygame.Rect(0, H - 118, self.W, 118)
        pygame.draw.rect(screen, (30, 26, 22), bar)
        pygame.draw.rect(screen, (70, 60, 48), bar, 2)
        f = self.ctx["font"]; sf = self.ctx["sfont"]
        # resources
        screen.blit(f.render(f"Ink {int(self.ink[0])}/{self.cap[0]}", True, (150, 200, 235)), (16, H - 108))
        used = self._supply_used(0); cap = self._supply_cap(0)
        # supply turns amber when nearly full, red when blocked
        scol = (150, 210, 150) if used < cap - 2 else (230, 200, 90) if used < cap else (230, 110, 90)
        screen.blit(f.render(f"Supply {used}/{cap}", True, scol), (150, H - 108))
        n_workers = sum(1 for u in self.units if u.team == 0 and u.worker)
        n_army = sum(1 for u in self.units if u.team == 0 and not u.worker)
        screen.blit(sf.render(f"Scribes {n_workers}   Army {n_army}", True, (222, 214, 198)), (16, H - 84))
        # selection summary
        self._draw_selection_panel(screen, 16, H - 60)
        # command palette (context-sensitive)
        self._draw_palette(screen)

    def _draw_selection_panel(self, screen, x, y):
        sf = self.ctx["sfont"]
        us = self._sel_units(); ss = self._sel_structs()
        if not us and not ss:
            screen.blit(sf.render("Drag select · right-click move/attack · Ctrl/Shift+# groups (tap ## to jump) · / palette",
                                  True, (150, 140, 124)), (x, y))
            return
        if us:
            from collections import Counter
            c = Counter(u.kind for u in us)
            txt = "  ".join(f"{UNITS[k]['name']}×{n}" for k, n in c.items())
            # a little origami portrait of the first selected unit type
            og = self.ctx.get("origami"); cards = self.ctx.get("cards")
            px_off = 0
            if og is not None and cards is not None:
                try:
                    u0 = us[0]; card = cards.get(u0.card, {})
                    recipe = og.recipe_for(u0.card, card.get("family", "figure"), card)
                    paper = card.get("paper", (200, 190, 170)); tint = self._tcol(u0.team)
                    base = tuple(int(paper[i] * 0.45 + tint[i] * 0.55) for i in range(3))
                    pcx, pcy = x + 16, y + 4; ps = 15
                    pygame.draw.circle(screen, (40, 34, 28), (pcx, pcy), 18)
                    for part in recipe:
                        ax = part.get("at", [0, 0])[0]; ay = part.get("at", [0, 0])[1]
                        psz = ps * part.get("size", 1.0)
                        pcol = tuple(min(255, int(c2 * part.get("tone", 1.0))) for c2 in base)
                        og.draw_part(screen, part.get("part"), pcx + ax * ps, pcy + ay * ps - 4,
                                     psz, 1, pcol, part.get("h", 0.4) * psz * 0.5, 0.0, 0.0, None)
                    px_off = 40
                except Exception:
                    px_off = 0
            screen.blit(sf.render(f"Selected: {txt}", True, (232, 224, 208)), (x + px_off, y))
        elif ss:
            s = ss[0]
            q = f"  queue: {len(s.queue)}" if s.queue else ""
            state = "building…" if not s.complete else "ready"
            screen.blit(sf.render(f"{STRUCTURES[s.kind]['name']} — {state}{q}", True, (232, 224, 208)), (x, y))

    def _draw_palette(self, screen):
        self.palette_rects = []
        sf = self.ctx["sfont"]
        actions = []                    # (label, hotkey, action, cost, enabled)
        us = self._sel_units(); ss = self._sel_structs()
        if ss:
            s = ss[0]
            for uk in STRUCTURES[s.kind]["makes"]:
                sp = UNITS[uk]
                actions.append((sp["name"], sp["key"], ("train", uk), sp["cost"], True))
            # research options offered by this structure
            for rk in STRUCTURES[s.kind].get("researches", []):
                up = self._factbook()[self.fac[0]]["upgrades"][rk]
                done = rk in self.upgrades[0]
                actions.append(((up["name"] + (" ✓" if done else "")), "", ("research", rk),
                                0 if done else up["cost"], not done))
        if any(u.worker for u in us):
            for sk, sp in STRUCTURES.items():
                if sp["cost"] > 0:
                    ok = self._struct_available(0, sk)
                    actions.append((sp["name"], sp["key"], ("build", sk), sp["cost"], ok))
            actions.append(("Stop", "S", ("cmd", "stop"), 0, True))
        elif us:
            # spellcaster ability button(s) — shown when a caster is selected
            casters = [u for u in us if u.ability]
            if casters:
                ab = casters[0].ability
                max_e = max(c.energy for c in casters)
                ready = max_e >= ab["cost"]
                actions.append((f"{ab['name']}", ab.get("key", "K"), ("cast", ab["id"]),
                                ab["cost"], ready))
            actions.append(("Stop", "S", ("cmd", "stop"), 0, True))
            actions.append(("Attack", "A", ("cmd", "attack"), 0, True))
            actions.append(("Hold", "H", ("cmd", "hold"), 0, True))
            actions.append(("Patrol", "", ("cmd", "patrol"), 0, True))
        # lay out as buttons on the right of the HUD bar
        bx = self.W - 12; by = self.H - 108
        cellw, cellh = 132, 26
        col_x = bx - cellw
        row = 0
        for (label, hk, action, cost, enabled) in actions[:12]:
            rect = pygame.Rect(col_x, by + row * (cellh + 3), cellw, cellh)
            hover = rect.collidepoint(pygame.mouse.get_pos())
            base = (58, 50, 40) if enabled else (40, 36, 32)
            pygame.draw.rect(screen, base if not hover else (78, 66, 52), rect, border_radius=5)
            pygame.draw.rect(screen, (110, 96, 74) if enabled else (70, 62, 52), rect, 1, border_radius=5)
            txtcol = (230, 222, 206) if enabled else (120, 112, 100)
            pre = f"[{hk}] " if hk else ""
            screen.blit(sf.render(f"{pre}{label}", True, txtcol), (rect.x + 6, rect.y + 4))
            if cost:
                screen.blit(sf.render(f"{cost}", True, (150, 200, 235)), (rect.right - 26, rect.y + 4))
            if enabled:
                self.palette_rects.append((rect, action))
            row += 1
            if by + row * (cellh + 3) > self.H - 6:
                row = 0; col_x -= cellw + 6

    def _build_minimap_bg(self, w, h):
        """Pre-render the minimap terrain once: themed ground, fold-valley bands,
        high-ground plateaus, and obstacles — so the minimap reads as the map."""
        bg = pygame.Surface((w, h))
        th = self.theme
        bg.fill(tuple(th["ground"]))
        sx = w / WORLD_W; sy = h / WORLD_H
        # fold-valley bands
        shade = tuple(max(0, int(c * 0.9)) for c in th["ground"])
        for (x1, y1, x2, y2, bw) in self.bands:
            pygame.draw.line(bg, shade, (x1 * sx, y1 * sy), (x2 * sx, y2 * sy),
                             max(1, int(bw * sx)))
        # high-ground plateaus (lighter)
        hi = tuple(min(255, c + 16) for c in th["ground"])
        for hx, hy, hr in self._high:
            pygame.draw.circle(bg, hi, (int(hx * sx), int(hy * sy)), max(2, int(hr * sx)))
        # obstacles / ruins
        for o in self.objects:
            if o.blocks:
                pygame.draw.circle(bg, (110, 100, 86), (int(o.x * sx), int(o.y * sy)),
                                   max(1, int(o.r * sx)))
        self._mm_bg = bg

    def _draw_minimap(self, screen):
        mm_w, mm_h = 200, 134
        r = pygame.Rect(self.W - mm_w - 12, self.H - 118 - mm_h - 8, mm_w, mm_h)
        self.mm_rect = r
        if getattr(self, "_mm_bg", None) is None or self._mm_bg.get_size() != (mm_w, mm_h):
            self._build_minimap_bg(mm_w, mm_h)
        screen.blit(self._mm_bg, (r.x, r.y))
        pygame.draw.rect(screen, (90, 78, 60), r, 2)
        sx = r.w / WORLD_W; sy = r.h / WORLD_H
        for n in self.nodes:
            if n.amount > 0 and self._cell_state(n.x, n.y) > 0:
                pygame.draw.circle(screen, (120, 108, 190),
                                   (int(r.x + n.x * sx), int(r.y + n.y * sy)), 2)
        for s in self.structs:
            if s.team != 0 and self.fog_on and self._cell_state(s.x, s.y) == 0:
                continue
            pygame.draw.rect(screen, self._tcol(s.team),
                             (int(r.x + s.x * sx) - 2, int(r.y + s.y * sy) - 2, 4, 4))
        for u in self.units:
            if u.team != 0 and self.fog_on and not self._visible(u):
                continue
            if u.team != 0 and getattr(u, "cloak", False) and not self._detected(u):
                continue
            screen.set_at((int(r.x + u.x * sx), int(r.y + u.y * sy)), self._tcol(u.team))
        # fog on the minimap: black over unexplored, a light dim over explored
        if self.fog_on and self.fog:
            veil = pygame.Surface((r.w, r.h), pygame.SRCALPHA)
            fc = self.fog_cell
            for gy in range(self.fog_rows):
                for gx in range(self.fog_cols):
                    st = self.fog[gy][gx]
                    if st == 2:
                        continue
                    a = 190 if st == 0 else 70
                    pygame.draw.rect(veil, (10, 9, 14, a),
                                     (int(gx * fc * sx), int(gy * fc * sy),
                                      int(fc * sx) + 1, int(fc * sy) + 1))
            screen.blit(veil, (r.x, r.y))
        # camera viewport rect (zoom-aware)
        vx = r.x + self.cam_x * sx; vy = r.y + self.cam_y * sy
        vw = (self.W / self.zoom) * sx; vh = ((self.H - 118) / (self.zoom * ISO)) * sy
        pygame.draw.rect(screen, (240, 230, 120),
                         (int(vx), int(vy), int(vw), int(vh)), 1)
        # flashing under-attack pings
        for p in self.pings:
            px = int(r.x + p["x"] * sx); py = int(r.y + p["y"] * sy)
            pulse = int(3 + 6 * abs(math.sin(p["t"] * 6)))
            if int(p["t"] * 6) % 2 == 0:
                pygame.draw.circle(screen, (240, 90, 80), (px, py), pulse, 2)

    # ── ink-pop UI: panels expand in with a sharp 150ms ink blot ────────────
    def _ink_t(self, key, active, dur=0.15):
        """Advance and return the eased 0..1 expansion for a UI element. Sharp:
        ease-out cubic over `dur` seconds, symmetric collapse."""
        t = self._ink_anim.get(key, 0.0)
        step = self._frame_dt / max(0.001, dur)
        t = min(1.0, t + step) if active else max(0.0, t - step)
        self._ink_anim[key] = t
        e = 1.0 - (1.0 - t) ** 3
        return e

    def _ink_panel(self, screen, rect, t, fill=(26, 23, 32), edge=(120, 104, 80),
                   alpha=225, anchor="center"):
        """Draw a panel expanding from its anchor as a paper-ink blot: sharp
        scale-in with a whisker of overshoot, plus tiny ink beads on the rim
        while it's still spreading. Returns the current (scaled) rect."""
        if t <= 0.001:
            return None
        s = t * (1.0 + 0.05 * math.sin(min(1.0, t) * math.pi))
        w = max(4, int(rect.w * s)); h = max(4, int(rect.h * s))
        if anchor == "bottom":
            r = pygame.Rect(rect.centerx - w // 2, rect.bottom - h, w, h)
        elif anchor == "topleft":
            r = pygame.Rect(rect.x, rect.y, w, h)
        else:
            r = pygame.Rect(rect.centerx - w // 2, rect.centery - h // 2, w, h)
        panel = pygame.Surface((r.w, r.h), pygame.SRCALPHA)
        pygame.draw.rect(panel, (*fill, int(alpha * min(1.0, t * 1.4))),
                         panel.get_rect(), border_radius=9)
        pygame.draw.rect(panel, (*edge, 235), panel.get_rect(), 1, border_radius=9)
        screen.blit(panel, r.topleft)
        if t < 0.999:                      # ink beads racing along the rim
            rng = random.Random(key_hash := (rect.x * 31 + rect.y))
            for b in range(4):
                a = rng.uniform(0, math.tau) + t * 9.0
                bx = r.centerx + math.cos(a) * (r.w / 2)
                by = r.centery + math.sin(a) * (r.h / 2)
                pygame.draw.circle(screen, edge, (int(bx), int(by)),
                                   max(1, int(3 * (1 - t))))
        return r

    # ── command palette ─────────────────────────────────────────────────────
    def _pal_open(self):
        self.pal = {"q": "", "sel": 0}
        self._sfx("select", 0.4)

    def _pal_commands(self):
        """The live command list: core verbs, production for what you own,
        research you can start, control groups with members, user scripts."""
        cmds = [
            ("Select: Army", lambda: self._quick_action("army")),
            ("Select: Scribes", lambda: self._quick_action("workers")),
            ("Select: Idle workers", lambda: self._quick_action("idle")),
            ("Camera: Base", lambda: self._quick_action("base")),
            ("Order: Stop", lambda: self._script_op({"op": "stop"})),
            ("Order: Hold position", lambda: self._script_op({"op": "hold"})),
            ("Toggle: Fog of war", lambda: self._script_op({"op": "fog"})),
            ("Toggle: Tech tree", lambda: setattr(self, "show_tech", not self.show_tech)),
        ]
        for n in range(1, 10):
            g = self._group(n)
            if g:
                cmds.append((f"Group {n}: select ({len(g)})",
                             (lambda nn: lambda: self._script_op(
                                 {"op": "group", "n": nn, "do": "select"}))(n)))
        for sk, spec in self.facdata(0)["structures"].items():
            if self._struct_available(0, sk) and spec.get("cost", 0) > 0:
                cmds.append((f"Build: {spec['name']}  ·  {spec['cost']} ink",
                             (lambda k: lambda: self._palette_action(("build", k)))(sk)))
        makers = [s for s in self.structs if s.team == 0 and s.complete]
        seen_units = set()
        for s in makers:
            for uk in self.sspec(0, s.kind).get("makes", []):
                if uk in seen_units:
                    continue
                seen_units.add(uk)
                uspec = self.uspec(0, uk)
                cmds.append((f"Train: {uspec['name']}  ·  {uspec['cost']} ink",
                             (lambda k: lambda: self._script_op(
                                 {"op": "train", "unit": k}))(uk)))
        for s in makers:
            for up in self.sspec(0, s.kind).get("researches", []):
                if up in self.upgrades[0] or (s.research and s.research[0] == up):
                    continue
                u = self.facdata(0)["upgrades"][up]
                cmds.append((f"Research: {u['name']}  ·  {u['cost']} ink",
                             (lambda st, uu: lambda: self._start_research(st, uu))(s, up)))
        for sc in self.pal_scripts:
            cmds.append((f"Script: {sc['name']}",
                         (lambda ops: lambda: self._run_script(ops))(sc["ops"])))
        return cmds

    @staticmethod
    def _fuzzy(q, label):
        """Subsequence match score: higher is better, None = no match."""
        if not q:
            return 0
        ql, ll = q.lower(), label.lower()
        pos = -1; score = 0
        for c in ql:
            nxt = ll.find(c, pos + 1)
            if nxt < 0:
                return None
            score += 3 if nxt == pos + 1 else 1     # reward runs
            if nxt == 0 or ll[nxt - 1] in " :·":
                score += 2                          # reward word starts
            pos = nxt
        return score

    def _pal_filtered(self):
        q = self.pal["q"]
        scored = []
        for i, (label, fn) in enumerate(self._pal_commands()):
            sc = self._fuzzy(q, label)
            if sc is not None:
                scored.append((-sc, i, label, fn))
        scored.sort()
        return [(label, fn) for _, _, label, fn in scored[:12]]

    def _pal_key(self, ev):
        k = ev.key
        if k == pygame.K_ESCAPE:
            self.pal = None; return
        rows = self._pal_filtered()
        if k in (pygame.K_RETURN, pygame.K_KP_ENTER):
            if rows:
                _, fn = rows[min(self.pal["sel"], len(rows) - 1)]
                self.pal = None
                fn()
            else:
                self.pal = None
            return
        if k == pygame.K_UP:
            self.pal["sel"] = max(0, self.pal["sel"] - 1); return
        if k == pygame.K_DOWN:
            self.pal["sel"] = min(max(0, len(rows) - 1), self.pal["sel"] + 1); return
        if k == pygame.K_BACKSPACE:
            self.pal["q"] = self.pal["q"][:-1]; self.pal["sel"] = 0; return
        ch = getattr(ev, "unicode", "")
        if ch and ch.isprintable():
            self.pal["q"] += ch; self.pal["sel"] = 0

    def _draw_palette_overlay(self, screen):
        t = self._ink_t("palette", self.pal is not None)
        if t <= 0.001:
            return
        f = self.ctx["font"]; sf = self.ctx["sfont"]
        rows = self._pal_filtered() if self.pal else []
        w = min(560, self.W - 80)
        h = 54 + 30 * max(1, len(rows))
        box = pygame.Rect(self.W // 2 - w // 2, 64, w, h)
        r = self._ink_panel(screen, box, t, fill=(24, 21, 30), edge=(190, 160, 90),
                            anchor="topleft")
        if r is None or t < 0.55 or not self.pal:
            return
        q = self.pal["q"] or ""
        prompt = f.render("› " + q + "▏", True, (240, 230, 205))
        screen.blit(prompt, (box.x + 16, box.y + 12))
        pygame.draw.line(screen, (90, 78, 58), (box.x + 12, box.y + 40),
                         (box.right - 12, box.y + 40), 1)
        y = box.y + 48
        for i, (label, _fn) in enumerate(rows):
            hot = (i == self.pal["sel"])
            if hot:
                hr = pygame.Rect(box.x + 8, y - 3, box.w - 16, 27)
                pygame.draw.rect(screen, (58, 48, 34), hr, border_radius=6)
                pygame.draw.rect(screen, (190, 160, 90), hr, 1, border_radius=6)
            screen.blit(sf.render(label, True,
                                  (244, 234, 210) if hot else (196, 186, 168)),
                        (box.x + 20, y))
            y += 30
        if not rows:
            screen.blit(sf.render("no matching command", True, (150, 140, 126)),
                        (box.x + 20, y))

    # ── palette script executor (buildscript ethos: bad ops flash & skip) ───
    def _run_script(self, ops):
        for op in ops:
            try:
                self._script_op(op)
            except Exception as ex:
                self.flash(f"script op {op.get('op')}: {ex}")

    def _script_op(self, op):
        t = op.get("op")
        if t == "flash":
            self.flash(str(op.get("text", "")))
        elif t == "select":
            self._quick_action(op.get("what", "army"))
        elif t == "camera":
            if "x" in op and "y" in op:
                self._centre_on(float(op["x"]), float(op["y"]))
            else:
                self._quick_action("base")
        elif t == "stop":
            for u in self._sel_units():
                u.goal = None; u.target = None
                u.attack_move = False; u.state = "idle"
                u.stance = "aggressive"; u.patrol = None
        elif t == "hold":
            for u in self._sel_units():
                u.goal = None; u.state = "idle"; u.stance = "hold"; u.patrol = None
        elif t == "fog":
            self.fog_on = not self.fog_on
            if self.fog_on and self.fog:
                self._update_fog(force=True)
        elif t == "group":
            n = int(op.get("n", 1))
            if op.get("do", "select") == "assign":
                self.groups[n] = [u for u in self.selected
                                  if isinstance(u, Unit) and u.alive]
            else:
                self._select(list(self._group(n)))
        elif t == "train":
            uk = op.get("unit"); count = int(op.get("count", 1))
            makers = [s for s in self.structs if s.team == 0 and s.complete
                      and uk in self.sspec(0, s.kind).get("makes", [])]
            if not makers:
                self.flash(f"Nothing can train '{uk}'."); return
            for _ in range(count):
                s = min(makers, key=lambda s: len(s.queue))
                self._queue_train(s, uk)
        elif t == "research":
            up = op.get("upgrade")
            for s in self.structs:
                if s.team == 0 and s.complete and                         up in self.sspec(0, s.kind).get("researches", []):
                    self._start_research(s, up); return
            self.flash(f"Nothing can research '{up}'.")
        elif t == "build":
            self._script_build(op.get("kind"))
        else:
            self.flash(f"Unknown script op '{t}'.")

    def _script_build(self, sk):
        """Auto-place a structure near the town hall at the first clear spot
        (a widening ring search), assigning the nearest worker — so scripts can
        raise buildings without a placement click."""
        if sk not in self.facdata(0)["structures"]:
            self.flash(f"Unknown structure '{sk}'."); return
        spec = self.sspec(0, sk)
        if not self._struct_available(0, sk):
            self.flash(f"{spec['name']} requires more tech."); return
        if self.ink[0] < spec["cost"]:
            self.flash("Not enough ink."); return
        hall = next((s for s in self.structs
                     if s.team == 0 and s.kind == "scriptorium"), None)
        workers = [u for u in self.units if u.team == 0 and u.worker and u.alive]
        if hall is None or not workers:
            self.flash("No base / no workers."); return
        spot = None
        for ring in range(1, 10):
            rr = hall.r + spec["r"] + 24 + ring * 46
            for k in range(10):
                a = k / 10 * math.tau + ring * 0.35
                wx = hall.x + math.cos(a) * rr
                wy = hall.y + math.sin(a) * rr
                if not self._placement_blocked(wx, wy, spec["r"]):
                    spot = (wx, wy); break
            if spot:
                break
        if spot is None:
            self.flash("No room near the base."); return
        wx, wy = spot
        self.ink[0] -= spec["cost"]
        st = Structure(sk, wx, wy, 0, complete=False, spec=spec)
        self.structs.append(st); self.stats[0]["structs"] += 1
        w = min(workers, key=lambda u: math.hypot(u.x - wx, u.y - wy))
        w.goal = (wx, wy); w.state = "move"
        self._sfx("deploy", 0.7)
        self.flash(f"Raising {spec['name']}…")

    def _draw_msg(self, screen):
        f = self.ctx["font"]
        t = self._ink_t("toast", self.msg_t > 0.25)
        if t <= 0.001:
            return
        lbl = f.render(self.msg, True, (245, 238, 222))
        bg = pygame.Rect(self.W // 2 - lbl.get_width() // 2 - 12, 14,
                         lbl.get_width() + 24, lbl.get_height() + 10)
        self._ink_panel(screen, bg, t, fill=(22, 18, 14), edge=(150, 128, 92),
                        alpha=195)
        if t > 0.6:
            screen.blit(lbl, (bg.x + 12, bg.y + 5))

    def _draw_end(self, screen):
        f = self.ctx["bfont"]; nf = self.ctx["font"]; sf = self.ctx["sfont"]
        win = self.result == "win"
        spectate = getattr(self, "spectate", False)
        if spectate:
            # no "you" — name the victorious faction instead
            winner = 0 if self.result == "win" else 1
            txt = f"{self.facdata(winner)['name']} WINS"
            col = self._tcol(winner)
        else:
            txt = "VICTORY" if win else "DEFEAT"
            col = (150, 220, 150) if win else (220, 120, 110)
        s = pygame.Surface((self.W, self.H), pygame.SRCALPHA); s.fill((10, 8, 6, 205))
        screen.blit(s, (0, 0))
        tnow = getattr(self, "_anim_t", 0.0)
        # a broad banner in the relevant faction's colour behind the title
        bannercol = self._tcol(0 if win else 1)
        band = pygame.Surface((self.W, 120), pygame.SRCALPHA)
        band.fill((*bannercol, 40))
        pygame.draw.line(band, (*bannercol, 160), (0, 0), (self.W, 0), 2)
        pygame.draw.line(band, (*bannercol, 160), (0, 119), (self.W, 119), 2)
        screen.blit(band, (0, 70))
        # celebratory paper scraps drifting down (on a win, or always in spectate)
        if win or spectate:
            import random as _rnd
            rr = _rnd.Random(7)
            for i in range(60):
                bx = rr.uniform(0, self.W)
                sp = rr.uniform(20, 60)
                by = (rr.uniform(0, self.H) + tnow * sp) % self.H
                c = bannercol if i % 2 else (230, 220, 190)
                sz = rr.randint(2, 4)
                sc = pygame.Surface((sz, sz), pygame.SRCALPHA); sc.fill((*c, 200))
                sc = pygame.transform.rotate(sc, (tnow * 120 + i * 30) % 360)
                screen.blit(sc, (int(bx), int(by)))
        lbl = f.render(txt, True, col)
        screen.blit(lbl, (self.W // 2 - lbl.get_width() // 2, 90))
        mm, ss = int(self.match_t // 60), int(self.match_t % 60)
        sub = nf.render(f"{self.facdata(0)['name']} vs {self.facdata(1)['name']}  ·  "
                        f"{self.mapdef['name']}  ·  {mm}:{ss:02d}", True, (222, 214, 198))
        screen.blit(sub, (self.W // 2 - sub.get_width() // 2, 210))
        # ── tournament bracket panel ──
        if self.tournament is not None and tourney is not None:
            names = {k: v.get("name", k) for k, v in self._factbook().items()}
            lines = tourney.bracket_lines(self.tournament, names)
            if self.tournament.get("done"):
                champ = tourney.champion(self.tournament)
                lines = ["TOURNAMENT CHAMPION",
                         names.get(champ, champ) +
                         ("  —  that's you!" if self.result == "win" and
                          tourney.champion(self.tournament) == self.fac[0] else "")]
                hint = "the bracket is complete"
            elif tourney.player_alive(self.tournament):
                hint = "press  N  for the next round"
            else:
                hint = "your run is over — press N to watch the herald finish it"
            px = self.W - 360
            py = 100
            panel = pygame.Rect(px - 16, py - 12, 352, 30 + 22 * len(lines) + 26)
            pygame.draw.rect(screen, (24, 20, 28), panel, border_radius=9)
            pygame.draw.rect(screen, (216, 170, 220), panel, 1, border_radius=9)
            for i, ln in enumerate(lines):
                colr = (238, 224, 240) if i == 0 else (198, 186, 204)
                screen.blit(sf.render(ln, True, colr), (px, py + i * 22))
            screen.blit(sf.render(hint, True, (216, 170, 220)),
                        (px, py + len(lines) * 22 + 10))
        # a two-column stat table (you vs enemy)
        rows = [("Units built", "built"), ("Units lost", "lost"),
                ("Enemy units slain", "killed"), ("Veterans promoted", "vets"),
                ("Ink harvested", "harvested"), ("Structures raised", "structs")]
        x0 = self.W // 2 - 260; y0 = 260
        colx = {0: x0 + 220, 1: x0 + 400}
        screen.blit(nf.render("You", True, self._tcol(0)), (colx[0], y0))
        screen.blit(nf.render("Enemy", True, self._tcol(1)), (colx[1], y0))
        y = y0 + 34
        for label, key in rows:
            screen.blit(sf.render(label, True, (210, 202, 186)), (x0, y))
            for team in (0, 1):
                v = self.stats[team][key]
                sval = f"{int(v)}" if key != "harvested" else f"{int(v)}"
                screen.blit(nf.render(sval, True, (235, 228, 212)), (colx[team], y - 2))
            y += 30
        tip = sf.render("Press Esc to return to the menu.", True, (170, 160, 144))
        screen.blit(tip, (self.W // 2 - tip.get_width() // 2, y + 20))


if __name__ == "__main__":
    # headless smoke test: run the sim for a while, ensure the AI builds & fights
    import os
    os.environ.setdefault("SDL_VIDEODRIVER", "dummy")
    os.environ.setdefault("SDL_AUDIODRIVER", "dummy")
    pygame.init()
    pygame.display.set_mode((1280, 800))
    font = pygame.font.SysFont("georgia", 18)
    ctx = dict(W=1280, H=800, font=font, sfont=pygame.font.SysFont("georgia", 14),
               bfont=pygame.font.SysFont("georgia", 40, bold=True),
               draw_unit=None, cards={})
    r = RTS(ctx, map_id="leaflock", factions=["court", "ember"])
    # give the player a table and some army so both sides fight
    r._select([u for u in r.units if u.team == 0][:1])
    for _ in range(60 * 60):            # 60s
        r.update(1 / 60)
        if r.done:
            break
    p_structs = sum(1 for s in r.structs if s.team == 0)
    e_structs = sum(1 for s in r.structs if s.team == 1)
    e_army = sum(1 for u in r.units if u.team == 1 and not u.worker)
    assert any(s.kind == "scriptorium" for s in r.structs), "bases vanished"
    assert len(FACTIONS) == 4 and len(MAPS) == 9, "faction/map pool incomplete"
    # every faction's tech tree is internally consistent (requires point at real
    # structures; makes/researches point at real units/upgrades)
    for fk, fac in FACTIONS.items():
        for sk, sp in fac["structures"].items():
            for r2 in sp.get("requires", []):
                assert r2 in fac["structures"], f"{fk}.{sk} requires missing {r2}"
            for mk in sp.get("makes", []):
                assert mk in fac["units"], f"{fk}.{sk} makes missing {mk}"
            for uk in sp.get("researches", []):
                assert uk in fac["upgrades"], f"{fk}.{sk} researches missing {uk}"
    print(f"✅ rts self-test: {len(FACTIONS)} factions, {len(MAPS)} maps, tech trees valid; "
          f"ran 60s — player structs {p_structs}, enemy structs {e_structs}, "
          f"enemy army {e_army}, enemy upgrades {len(r.upgrades[1])}")
