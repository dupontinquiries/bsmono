"""
Paperbound card data layer.

Loads cards, abilities, and rare variants from cards.json so new cards and
balance patches ship as data, not code. Exposes a CARDS dict shaped exactly
like the legacy in-code dict game.py expects (name/cost/hp/dmg/spd/rng/size/
flying/building/riverhop/count/paper/sigil), plus rich metadata (rarity, set,
rotation) and a variant system.

Design:
  • cards.json is the single source of truth. If it's missing/corrupt, the game
    still runs from whatever fallback dict is passed to install_fallback().
  • Abilities are declarative descriptors keyed by id; the actual behaviors live
    in game.py's engine. A new card just references an existing ability id (plus
    tuned stats), so authors add cards without touching engine code.
  • Variants (Inked/Gilded/Shadow…) are alternate printings that scale stats and
    may add a second ability. variant_of() bakes a variant into a CARDS-style
    entry with a synthetic key like "sabertooth#gilded".
"""
import json, os, random

_HERE = os.path.dirname(os.path.abspath(__file__))
CARDS_JSON = os.path.join(_HERE, "cards.json")

DOC = None                 # the raw parsed cards.json
CARDS = {}                 # runtime dict game.py consumes (legacy shape)
ABILITIES = {}             # id -> descriptor
VARIANTS = {}              # id -> variant spec
META = {}                  # id -> {rarity,set,rotation} for base cards
VARIANT_WEIGHTS = {}
RARITY_ORDER = ["common", "uncommon", "rare", "epic", "legendary"]


def _legacy_entry(card):
    """Convert a JSON card record into the flat dict shape game.py expects."""
    e = {
        "name": card["name"],
        "cost": card["cost"],
        "hp": card["hp"], "dmg": card["dmg"], "spd": card["spd"], "rng": card["rng"],
        "size": card["size"],
        "flying": card.get("flying", False),
        "paper": tuple(card["paper"]),
        "sigil": card.get("ability", "none"),
    }
    if card.get("building"): e["building"] = True
    if card.get("riverhop"): e["riverhop"] = True
    if card.get("count", 1) > 1: e["count"] = card["count"]
    if card.get("blurb"): e["blurb"] = card["blurb"]
    if isinstance(card.get("model"), (list, tuple)): e["model"] = card["model"]
    if isinstance(card.get("model_extra"), (list, tuple)): e["model_extra"] = card["model_extra"]
    # tokens (spawned-only units like the beekeeper's bees) are flagged so the
    # game can keep them out of the craftable/deck pool.
    if str(card.get("set", "")).endswith("_token") or card.get("token"):
        e["token"] = True
    return e


def load(path=CARDS_JSON):
    """Load cards.json into module state. Returns True on success."""
    global DOC, CARDS, ABILITIES, VARIANTS, META, VARIANT_WEIGHTS, RARITY_ORDER, _DATA_STAMP
    _DATA_STAMP = None                # recompute the ruleset stamp after a load
    try:
        with open(path) as f:
            DOC = json.load(f)
    except Exception as ex:
        print(f"[carddata] could not load {path}: {ex}")
        return False
    ABILITIES = DOC.get("abilities", {})
    VARIANTS = DOC.get("variants", {})
    VARIANT_WEIGHTS = DOC.get("variant_weights", {"standard": 100})
    RARITY_ORDER = DOC.get("rarity_order", RARITY_ORDER)
    CARDS = {}
    META = {}
    for key, card in DOC["cards"].items():
        CARDS[key] = _legacy_entry(card)
        META[key] = {
            "rarity": card.get("rarity", "common"),
            "set": card.get("set", "core"),
            "rotation": card.get("rotation", True),
        }
    return True


_DATA_STAMP = None
def data_stamp():
    """A content hash of the loaded ruleset (cards + abilities + variants). This
    is the base identity of the data a battle ran on — the root of the patch
    tree. Two builds with identical data produce the same stamp; any balance or
    content change moves it, which is how replays/net peers detect a mismatch.
    Cached until the next load()."""
    global _DATA_STAMP
    if _DATA_STAMP is not None:
        return _DATA_STAMP
    import hashlib
    ident = {
        "cards": DOC.get("cards", {}) if DOC else {},
        "abilities": DOC.get("abilities", {}) if DOC else {},
        "variants": DOC.get("variants", {}) if DOC else {},
    }
    blob = json.dumps(ident, sort_keys=True, separators=(",", ":")).encode("utf-8")
    _DATA_STAMP = hashlib.sha256(blob).hexdigest()
    return _DATA_STAMP


def install_fallback(cards_dict, sigil_colors=None, sigil_desc=None):
    """If cards.json failed to load, adopt game.py's built-in dict so the game
    still runs. Metadata defaults to common/core/in-rotation."""
    global CARDS, META
    if CARDS:
        return
    CARDS = dict(cards_dict)
    META = {k: {"rarity": "common", "set": "core", "rotation": True} for k in CARDS}


# ── rarity / rotation helpers ─────────────────────────────────────────────
def rarity(key):
    base = key.split("#")[0]
    return META.get(base, {}).get("rarity", "common")

def card_set(key):
    base = key.split("#")[0]
    return META.get(base, {}).get("set", "core")

def in_rotation(key):
    base = key.split("#")[0]
    return META.get(base, {}).get("rotation", True)

def rotation_cards():
    return [k for k in CARDS if in_rotation(k)]

def rarity_rank(r):
    return RARITY_ORDER.index(r) if r in RARITY_ORDER else 0


# ── variants ──────────────────────────────────────────────────────────────
def variant_key(base_key, variant_id):
    return base_key if variant_id in (None, "standard") else f"{base_key}#{variant_id}"

def base_of(key):
    return key.split("#")[0]

def variant_of(key):
    return key.split("#")[1] if "#" in key else "standard"

def make_variant(base_key, variant_id):
    """Return a CARDS-style entry for base_key printed as variant_id, registering
    it in CARDS under the synthetic key so game.py can treat it like any card."""
    if variant_id in (None, "standard"):
        return base_key
    vk = variant_key(base_key, variant_id)
    if vk in CARDS:
        return vk
    spec = VARIANTS.get(variant_id)
    if not spec or base_key not in CARDS:
        return base_key
    base = dict(CARDS[base_key])
    mods = spec.get("mods", {})
    for stat, mult in mods.items():
        if stat in base and isinstance(base[stat], (int, float)):
            base[stat] = int(round(base[stat] * mult)) if stat != "spd" else base[stat] * mult
    if spec.get("suffix"):
        base["name"] = f"{base['name']} ({spec['suffix']})"
    if spec.get("tint"):
        pr, pg, pb = base["paper"]; tr, tg, tb = spec["tint"]
        base["paper"] = ((pr + tr) // 2, (pg + tg) // 2, (pb + tb) // 2)
    # a variant may grant a SECOND ability; game.py reads one sigil, so we keep
    # the base sigil but stash the bonus for engines that support it.
    add = spec.get("add_ability")
    if add:
        base["variant_ability"] = add
    base["variant"] = variant_id
    CARDS[vk] = base
    return vk

def roll_variant(rng=random):
    """Weighted random variant id for a freshly-found card."""
    ids = list(VARIANT_WEIGHTS.keys())
    weights = [VARIANT_WEIGHTS[i] for i in ids]
    return rng.choices(ids, weights=weights, k=1)[0]

def variant_display(variant_id):
    v = VARIANTS.get(variant_id, {})
    return v.get("name") or "Standard"


# auto-load on import
load()

if __name__ == "__main__":
    ok = load()
    print(f"loaded={ok} cards={len(CARDS)} abilities={len(ABILITIES)} variants={len(VARIANTS)}")
    print("rotation cards:", len(rotation_cards()))
    # exercise a variant
    vk = make_variant("sabertooth", "gilded")
    print("variant key:", vk)
    print("gilded sabertooth:", CARDS[vk])
    print("random variants:", [roll_variant() for _ in range(8)])
