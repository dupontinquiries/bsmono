"""
expansions.py — content-pack toggles for Paperbound.

Two layers, both driven from a single persisted state file:

  • SETS   — every card in cards.json carries a "set" (core, wilds, tideswell,
             clockwork, …). This module lets the player toggle whole sets on or
             off; a disabled set is simply forced OUT of rotation via the same
             `carddata.META[...]["rotation"]` gate the game already respects, so
             deck-building, packs, and matchmaking all pick it up for free.
             "core" is locked on.

  • PACKS  — external expansion files in the `expansions/` directory beside this
             module. Each pack is one JSON document that can carry NEW battler
             cards (full cards.json schema, origami `model` included) and/or NEW
             RTS maps (rts_data.json map schema, overlays included). Enabled
             packs are merged into `carddata` and — when the Foldworks RTS boots
             — into `rts.MAPS` as extra pickable maps. Cards from a pack get
             `set` = the pack id, so the SETS toggle covers them uniformly.

Pack schema (expansions/<id>.json):
    {
      "id": "dragon_legion",
      "name": "Dragon Legion",
      "blurb": "Dragon-mounted cavalry & ancient warfare.",
      "cards":    { "<key>": { ...cards.json card... }, ... },     # optional
      "rts_maps": { "<key>": { ...rts_data.json map... }, ... }    # optional
    }

State file (expansions_state.json beside this module):
    { "sets": { "<set-or-pack-id>": true|false, ... } }
Anything missing from the state defaults to ENABLED, so shipping a new set never
hides content until a player opts out.

Determinism note: netplay/replays already guard ruleset identity through
carddata.data_stamp(); merged pack cards change the stamp exactly like editing
cards.json would, so mismatched peers are detected, not silently divergent.

Run `python expansions.py --selftest` for a headless round-trip check.
"""

import os, json, copy

HERE = os.path.dirname(os.path.abspath(__file__))
PACKS_DIR = os.path.join(HERE, "expansions")
STATE_PATH = os.path.join(HERE, "expansions_state.json")

LOCKED_SETS = {"core"}          # can never be disabled
# token/support sets that ride along with their parent set rather than
# appearing as their own toggle row (e.g. menagerie_token belongs to menagerie)
_SET_ALIASES = {"menagerie_token": "menagerie"}

_carddata = None                # the carddata module, once bound
_packs = {}                     # pack id -> parsed pack doc
_state = {"sets": {}}           # persisted toggles
_base_rotation = {}             # card key -> original rotation flag (restore on re-enable)
_rts_applied = set()            # pack ids already merged into a live rts module


# ── state ────────────────────────────────────────────────────────────────────

def load_state():
    global _state
    try:
        with open(STATE_PATH) as f:
            doc = json.load(f)
        if isinstance(doc, dict) and isinstance(doc.get("sets"), dict):
            _state = {"sets": {str(k): bool(v) for k, v in doc["sets"].items()}}
    except Exception:
        _state = {"sets": {}}
    return _state


def save_state():
    try:
        with open(STATE_PATH, "w") as f:
            json.dump(_state, f, indent=1, sort_keys=True)
        return True
    except Exception as ex:
        print(f"[expansions] could not save state: {ex}")
        return False


def enabled(set_id):
    """A set/pack is enabled unless explicitly toggled off (locked sets always on)."""
    canon = _SET_ALIASES.get(set_id, set_id)
    if canon in LOCKED_SETS:
        return True
    return _state["sets"].get(canon, True)


def toggle(set_id, on=None):
    """Toggle (or set) a set/pack. Returns the new value. Locked sets refuse."""
    canon = _SET_ALIASES.get(set_id, set_id)
    if canon in LOCKED_SETS:
        return True
    cur = enabled(canon)
    val = (not cur) if on is None else bool(on)
    _state["sets"][canon] = val
    save_state()
    _apply_rotation()
    return val


# ── discovery ────────────────────────────────────────────────────────────────

def discover_packs():
    """Parse every expansions/*.json into _packs (bad files are skipped, not fatal)."""
    global _packs
    _packs = {}
    if not os.path.isdir(PACKS_DIR):
        return _packs
    for fn in sorted(os.listdir(PACKS_DIR)):
        if not fn.endswith(".json"):
            continue
        path = os.path.join(PACKS_DIR, fn)
        try:
            with open(path) as f:
                doc = json.load(f)
            pid = doc.get("id") or os.path.splitext(fn)[0]
            doc["id"] = pid
            doc.setdefault("name", pid.replace("_", " ").title())
            _packs[pid] = doc
        except Exception as ex:
            print(f"[expansions] skipping bad pack {fn}: {ex}")
    return _packs


def catalog():
    """[(set_id, display_name, blurb, n_cards, locked, enabled)] for the UI —
    built-in sets from carddata.META plus external packs, core first."""
    rows = []
    counts = {}
    if _carddata is not None:
        for key, meta in _carddata.META.items():
            s = _SET_ALIASES.get(meta.get("set", "core"), meta.get("set", "core"))
            counts[s] = counts.get(s, 0) + 1
    seen = set()
    order = ["core"] + sorted(s for s in counts if s != "core") + \
            sorted(p for p in _packs if p not in counts)
    for s in order:
        if s in seen:
            continue
        seen.add(s)
        pack = _packs.get(s)
        name = (pack or {}).get("name", s.replace("_", " ").title())
        blurb = (pack or {}).get("blurb", "")
        n = counts.get(s, len((pack or {}).get("cards", {})))
        rows.append((s, name, blurb, n, s in LOCKED_SETS, enabled(s)))
    return rows


# ── application ──────────────────────────────────────────────────────────────

def init(carddata_module):
    """Bind carddata, load state, merge enabled packs' cards, apply rotation.
    Call ONCE at startup, after carddata.load() succeeded."""
    global _carddata
    _carddata = carddata_module
    load_state()
    discover_packs()
    # remember authored rotation flags before we start overriding them
    for key, meta in _carddata.META.items():
        _base_rotation[key] = meta.get("rotation", True)
    _merge_pack_cards()
    _apply_rotation()


def _merge_pack_cards():
    """Merge every pack's cards into carddata (DOC/CARDS/META), tagging set=pack id.
    Runs for ALL packs — the enabled/disabled distinction is expressed purely
    through rotation, so toggling in the menu never needs a restart."""
    if _carddata is None:
        return
    for pid, pack in _packs.items():
        for key, card in (pack.get("cards") or {}).items():
            card = copy.deepcopy(card)
            card.setdefault("id", key)
            card["set"] = pid
            card.setdefault("rarity", "common")
            card.setdefault("rotation", True)
            if _carddata.DOC is not None:
                _carddata.DOC.setdefault("cards", {})[key] = card
            _carddata.CARDS[key] = _carddata._legacy_entry(card)
            _carddata.META[key] = {
                "rarity": card.get("rarity", "common"),
                "set": pid,
                "rotation": card.get("rotation", True),
            }
            _base_rotation[key] = card.get("rotation", True)
    # merged content is part of ruleset identity → recompute the stamp lazily
    try:
        _carddata._DATA_STAMP = None
    except Exception:
        pass


def _apply_rotation():
    """Force cards of disabled sets out of rotation; restore authored flags for
    enabled ones. This is the single switch the rest of the game listens to."""
    if _carddata is None:
        return
    for key, meta in _carddata.META.items():
        s = meta.get("set", "core")
        meta["rotation"] = _base_rotation.get(key, True) if enabled(s) else False


def apply_rts(rts_module):
    """Merge enabled packs' RTS maps into a live `rts` module (post load_data).
    Safe to call every time the RTS scene starts — merges are idempotent and
    maps from packs toggled off since last time are removed again."""
    for pid, pack in _packs.items():
        maps = pack.get("rts_maps") or {}
        if not maps:
            continue
        if enabled(pid):
            for mk, m in maps.items():
                m = copy.deepcopy(m)
                if "size" in m:
                    m["size"] = tuple(m["size"])
                if "spawns" in m:
                    m["spawns"] = [tuple(s) for s in m["spawns"]]
                rts_module._MAP_OBJECTS[mk] = list(m.get("objects", []))
                rts_module._MAP_HIGH[mk] = [tuple(h) for h in m.get("highground", [])]
                rts_module.MAPS[mk] = m
            _rts_applied.add(pid)
        elif pid in _rts_applied:
            for mk in maps:
                rts_module.MAPS.pop(mk, None)
                rts_module._MAP_OBJECTS.pop(mk, None)
                rts_module._MAP_HIGH.pop(mk, None)
            _rts_applied.discard(pid)


# ── scene (self-contained UI the host mounts, RTS-setup style) ───────────────

class Scene:
    """A minimal toggle screen. The host draws the desk/book chrome and calls
    draw(screen, fonts, mouse) + clicked(pos); we only own the rows."""

    def __init__(self, book_rect):
        self.book = book_rect
        self.rows = []          # (rect, set_id) rebuilt each draw

    def draw(self, screen, fonts, mouse, shade, PAGE, INK, GOLD):
        import pygame
        font, bfont, sfont = fonts
        b = self.book
        self.rows = []
        y = b.top + 104
        left = b.left + 44
        wdt = b.w - 88
        head = sfont.render("Toggle sets in or out of rotation. Core is always on. "
                            "Changes apply immediately — no restart.", True, shade(INK, 2.0))
        screen.blit(head, (left, y - 26))
        for s, name, blurb, n, locked, on in catalog():
            r = pygame.Rect(left, y, wdt, 46)
            hover = r.collidepoint(mouse)
            pygame.draw.rect(screen, shade(PAGE, 0.94 if on else (0.80 if hover else 0.74)),
                             r, border_radius=9)
            pygame.draw.rect(screen, GOLD if on else shade(INK, 3.2), r, 2, border_radius=9)
            # checkbox
            cb = pygame.Rect(r.left + 12, r.centery - 10, 20, 20)
            pygame.draw.rect(screen, shade(INK, 3.0), cb, 2, border_radius=4)
            if on:
                pygame.draw.line(screen, GOLD, (cb.left + 4, cb.centery),
                                 (cb.centerx - 1, cb.bottom - 5), 3)
                pygame.draw.line(screen, GOLD, (cb.centerx - 1, cb.bottom - 5),
                                 (cb.right - 3, cb.top + 4), 3)
            label = f"{name}   ·   {n} cards" + ("   (always on)" if locked else "")
            screen.blit(font.render(label, True, INK), (cb.right + 12, r.top + 5))
            if blurb:
                screen.blit(sfont.render(blurb, True, shade(INK, 2.1)),
                            (cb.right + 12, r.top + 26))
            if not locked:
                self.rows.append((r, s))
            y += 54

    def clicked(self, pos):
        """Toggle the row under pos. Returns the set id toggled, or None."""
        for r, s in self.rows:
            if r.collidepoint(pos):
                toggle(s)
                return s
        return None


# ── selftest ─────────────────────────────────────────────────────────────────

def _selftest():
    import types
    import carddata
    assert carddata.load(), "cards.json must load"
    init(carddata)
    rows = catalog()
    assert any(r[0] == "core" for r in rows), "core set missing from catalog"
    # pick a non-locked set and round-trip a toggle through the rotation gate
    victim = next(r[0] for r in rows if not r[4])
    keys = [k for k, m in carddata.META.items() if m["set"] == victim]
    assert keys, f"no cards in set {victim}"
    on_before = carddata.in_rotation(keys[0])
    toggle(victim, on=False)
    assert not carddata.in_rotation(keys[0]), "disable did not leave rotation"
    toggle(victim, on=True)
    assert carddata.in_rotation(keys[0]) == _base_rotation[keys[0]], "restore failed"
    # RTS merge round-trip against a stub module
    stub = types.SimpleNamespace(MAPS={}, _MAP_OBJECTS={}, _MAP_HIGH={})
    for pid, pack in _packs.items():
        if pack.get("rts_maps"):
            toggle(pid, on=True)
            apply_rts(stub)
            assert set(pack["rts_maps"]) <= set(stub.MAPS), "pack maps not merged"
            toggle(pid, on=False)
            apply_rts(stub)
            assert not (set(pack["rts_maps"]) & set(stub.MAPS)), "pack maps not removed"
            toggle(pid, on=True)
    print(f"[expansions] selftest OK — {len(rows)} sets, {len(_packs)} packs")


if __name__ == "__main__":
    import sys
    if "--selftest" in sys.argv:
        _selftest()
    else:
        load_state(); discover_packs()
        print("packs:", {k: v.get("name") for k, v in _packs.items()})
        print("state:", _state)
