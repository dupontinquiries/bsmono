"""
PAPERBOUND — trophies (proof-of-work origami awards).

A battle report (replay) is a signed, content-addressed record of a match: its
`battle_stamp` is a hash of the exact ruleset ⊕ seed ⊕ generated content. That
stamp is *proof the battle happened this way* — so we mint a trophy from it. The
stamp seeds every visible feature, which means the same report always yields the
same trophy, and any two different battles yield visibly different ones. The
trophy literally encodes the match.

What the stamp decides (all deterministic):
  • SHAPE       — cup / obelisk / star / shield / crown (the silhouette),
  • MATERIAL    — paper type: kraft / vellum / foil / parchment / washi / onyx,
                  each a palette + sheen,
  • BASE SIGIL  — an ink-blot identicon: a symmetric 5×5 blot pattern (like a
                  GitHub identicon, ink-styled) unique to the report,
  • ACCENTS     — gem/filigree count + placement,
  • RARITY      — from the result & margin (a flawless win mints a grander award).

What the MATCH CONTENT decides (from meta: arena + cards used):
  • EFFECTS     — particle/VFX auras: lava/ember arenas → fire, tide/mist → water,
                  frost cards → frost, ink themes → smoke, else a soft aura. These
                  reuse the diffusion VFX concepts.

`trophy_from_report(report)` → a spec dict (pure data, no pygame). game.py renders
it as a folded paper sculpture with the identicon on its face and the effects
swirling around it, using the origami + diffusion systems.
"""
import hashlib, json

SHAPES = ["cup", "obelisk", "star", "shield", "crown"]
MATERIALS = {
    "kraft":     {"base": (150, 110, 70),  "sheen": 0.15, "name": "Kraft"},
    "vellum":    {"base": (222, 210, 176), "sheen": 0.25, "name": "Vellum"},
    "foil":      {"base": (210, 200, 120), "sheen": 0.70, "name": "Gold Foil"},
    "parchment": {"base": (198, 170, 120), "sheen": 0.20, "name": "Parchment"},
    "washi":     {"base": (200, 150, 170), "sheen": 0.30, "name": "Washi"},
    "onyx":      {"base": (60, 58, 74),    "sheen": 0.55, "name": "Onyx Leaf"},
}
MATERIAL_ORDER = list(MATERIALS.keys())
EFFECTS = ["fire", "water", "frost", "smoke", "aura", "ember"]


def _seed_bytes(report):
    """The proof-of-work seed: the battle_stamp if present, else a hash of the
    report's identifying fields. Returns a long hex digest we slice for features."""
    meta = report.get("meta", {}) if isinstance(report, dict) else {}
    stamp = meta.get("battle_stamp") or meta.get("base_stamp")
    if not stamp:
        ident = {"seed": meta.get("seed"), "arena": meta.get("arena"),
                 "mode": meta.get("mode"), "result": report.get("result"),
                 "ticks": report.get("ticks")}
        stamp = hashlib.sha256(json.dumps(ident, sort_keys=True).encode()).hexdigest()
    # widen the entropy so we have plenty of independent bytes to slice
    return hashlib.sha256(stamp.encode()).hexdigest() + hashlib.sha256(
        (stamp + "salt").encode()).hexdigest()


def _cards_used(report):
    meta = report.get("meta", {}) if isinstance(report, dict) else {}
    used = set(meta.get("deck", []) or [])
    used |= set((report.get("cards", {}) or {}).keys())
    # also any keys that appear in the recorded events
    for ev in report.get("events", []) or []:
        if isinstance(ev, (list, tuple)) and len(ev) >= 2:
            used.add(ev[1])
    return used


def _effect_for(report):
    """Pick an effect from the arena + cards used (content-driven, not stamp)."""
    meta = report.get("meta", {}) if isinstance(report, dict) else {}
    arena = str(meta.get("arena", "")).lower()
    used = " ".join(_cards_used(report)).lower()
    hay = arena + " " + used
    if any(w in hay for w in ("lava", "ember", "ashen", "forge", "cinder", "phoenix", "flame")):
        return "fire"
    if any(w in hay for w in ("tide", "drown", "well", "wave", "leviathan", "tidal", "shark")):
        return "water"
    if any(w in hay for w in ("cold", "frost", "snow", "glass", "aurora", "chill")):
        return "frost"
    if any(w in hay for w in ("ink", "blot", "scribe", "quill", "obelisk")):
        return "smoke"
    if any(w in hay for w in ("moth", "lantern", "wisp", "star", "aura")):
        return "aura"
    return "ember"


def _identicon(seed_hex, n=5):
    """A symmetric ink-blot identicon: an n×n grid, mirrored left↔right, cells on
    where a hash bit is set. Deterministic per seed."""
    bits = bin(int(seed_hex[:16], 16))[2:].zfill(64)
    grid = [[False] * n for _ in range(n)]
    half = (n + 1) // 2
    k = 0
    for col in range(half):
        for row in range(n):
            on = bits[k % len(bits)] == "1"; k += 1
            grid[row][col] = on
            grid[row][n - 1 - col] = on          # mirror
    return grid


def trophy_from_report(report):
    """Mint a deterministic trophy spec from a battle report. Pure data."""
    h = _seed_bytes(report)
    def bval(i):     # a byte 0..255 from slice i
        return int(h[i * 2:i * 2 + 2], 16)
    meta = report.get("meta", {}) if isinstance(report, dict) else {}
    result = report.get("result", "win")
    ticks = report.get("ticks", 0) or 0

    shape = SHAPES[bval(0) % len(SHAPES)]
    material = MATERIAL_ORDER[bval(1) % len(MATERIAL_ORDER)]
    accents = 1 + (bval(2) % 4)                  # 1..4 gems/filigree
    hue_shift = bval(3) / 255.0
    sigil = _identicon(h)
    effect = _effect_for(report)

    # rarity from the result & how decisive it was (fast win = flawless)
    if result == "win":
        secs = ticks / 20.0
        if secs and secs < 30:
            rarity = "mythic"
        elif secs and secs < 60:
            rarity = "gold"
        else:
            rarity = "silver"
    elif result == "lose":
        rarity = "bronze"                        # a valiant-effort keepsake
    else:
        rarity = "silver"

    # a short human title woven from the match
    arena = str(meta.get("arena", "the table")).replace("_", " ").title()
    mode = str(meta.get("mode", "duel")).title()
    title = f"{MATERIALS[material]['name']} {shape.title()} of {arena}"

    return {
        "shape": shape,
        "material": material,
        "palette": MATERIALS[material],
        "accents": accents,
        "hue_shift": hue_shift,
        "sigil": sigil,                 # 5×5 bool grid (ink-blot identicon)
        "effect": effect,               # particle/VFX concept
        "rarity": rarity,
        "title": title,
        "subtitle": f"{mode} · {int(ticks/20)}s · {result}",
        "stamp": (meta.get("battle_stamp") or meta.get("base_stamp") or h)[:12],
        "seed_hex": h,
    }


def rarity_color(rarity):
    return {"mythic": (230, 120, 220), "gold": (230, 195, 90),
            "silver": (200, 205, 215), "bronze": (190, 130, 90)}.get(rarity, (200, 200, 200))


if __name__ == "__main__":
    # determinism: same report → same trophy; different → different
    r1 = {"meta": {"battle_stamp": "abc123def456", "arena": "ashen_leaf",
                   "mode": "1v1", "deck": ["phoenix", "emberfox"]},
          "result": "win", "ticks": 500}
    r2 = {"meta": {"battle_stamp": "abc123def456", "arena": "ashen_leaf",
                   "mode": "1v1", "deck": ["phoenix"]},
          "result": "win", "ticks": 500}
    r3 = {"meta": {"battle_stamp": "999888777666", "arena": "drowned_chapter",
                   "mode": "1v1", "deck": ["leviathan", "shark"]},
          "result": "win", "ticks": 400}
    t1 = trophy_from_report(r1)
    t1b = trophy_from_report({"meta": dict(r1["meta"]), "result": "win", "ticks": 500})
    t3 = trophy_from_report(r3)
    assert t1 == t1b, "same report must mint the identical trophy"
    assert t1["sigil"] != t3["sigil"], "different battles → different identicons"
    # content-driven effects
    assert t1["effect"] == "fire", t1["effect"]          # ashen/phoenix → fire
    assert t3["effect"] == "water", t3["effect"]         # drowned/leviathan → water
    # sigil is symmetric (mirrored)
    for row in t1["sigil"]:
        assert row == row[::-1], "identicon must be left-right symmetric"
    # rarity from decisiveness
    fast = trophy_from_report({"meta": {"battle_stamp": "z"*12}, "result": "win", "ticks": 400})
    assert fast["rarity"] == "mythic", fast["rarity"]    # <30s → mythic
    # variety across many reports
    shapes = set(); mats = set(); effs = set()
    for i in range(60):
        tr = trophy_from_report({"meta": {"battle_stamp": f"{i:012x}", "arena": "study"},
                                 "result": "win", "ticks": 800})
        shapes.add(tr["shape"]); mats.add(tr["material"]); effs.add(tr["effect"])
    assert len(shapes) >= 4 and len(mats) >= 4, (shapes, mats)
    print(f"✅ trophy self-test: deterministic mint, symmetric ink-blot identicon, "
          f"content-driven effects (fire/water/…), rarity from result, variety "
          f"({len(shapes)} shapes / {len(mats)} materials / {len(effs)} effects seen)")
