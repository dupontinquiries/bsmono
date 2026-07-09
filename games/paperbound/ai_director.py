"""
PAPERBOUND — AI Director.

A self-contained "living page" system that generates cards, maps and battlefield
twists at runtime and feeds them to the game as validated JSON. Two backends:

  • procedural  — deterministic-ish templates + RNG. Needs no network, always
                  works, and is what the offline game and the tests use.
  • llm         — an OpenAI-compatible chat endpoint (OpenAI, Ollama, LM Studio,
                  llama.cpp, vLLM, groq, together, Anthropic-via-proxy, …). The
                  model is asked for STRICT JSON; whatever comes back is run
                  through the same validator as the procedural path, so a bad or
                  malicious model response can never reach the simulation.

Everything that touches the network runs on background threads and pushes
results into a thread-safe queue the game drains on the main loop — the render
loop never blocks on I/O (see the windowing-safety work in game.py).

Public surface used by game.py:
    cfg = DirectorConfig.from_env()          # read PAPERBOUND_LLM_* env vars
    smith = CardSmith(cfg)
    smith.request(prompt, on_done)           # async → on_done(card_dict|None)
    card = smith.procedural(rng, theme=...)  # sync, offline
    director = Director(cfg); director.start(get_state)   # game-master loop
    for ev in director.drain(): apply(ev)    # {"kind": "card"|"map"|"twist"|...}

Card JSON schema (one card):
    {id,name,cost,hp,dmg,spd,rng,size,flying,building,riverhop,count,
     paper:[r,g,b], ability, rarity, set, rotation, blurb}
"""
import os, json, math, time, queue, random, threading

try:
    import urllib.request, urllib.error
    _NET = True
except Exception:
    _NET = False

# ── stat envelopes: every generated card is clamped into these so it can never
#    break the sim (a 1e9-HP flyer) or the renderer (a negative size). ──
BOUNDS = {
    "cost":  (1, 9),
    "hp":    (80, 2000),
    "dmg":   (0, 320),
    "spd":   (0, 110),
    "rng":   (16, 240),
    "size":  (14, 34),
    "count": (1, 6),
}
ABILITIES = [
    "pounce", "armored", "feather", "chill", "frenzy", "drinker", "slam",
    "taunt", "toxic", "sharp", "relentless", "towerbane", "doublestrike",
    "rush", "thorns", "plague", "inksplash", "knock", "freeze", "stealth",
    "snipe", "charge", "beacon", "bulwark", "rebirth", "rampage", "blink",
    "artillery", "roll", "radiance", "leap", "tidal", "molten", "warp",
    "siphon", "phalanx", "echo", "gale", "quill", "harvest",
]
RARITIES = ["common", "uncommon", "rare", "epic", "legendary"]
# weather / hazard / poi tags a map may use — must match game.py handlers
WEATHERS = [None, "lightning", "embers", "snow", "aurora", "sand", "sandstorm",
            "gulls", "mist"]
HAZARDS = [None, "lava", "fissures", "quicksand", "tide", "gears", "waterfalls"]
POIS = [None, "inktubs"]

_ADJ = ["Ember", "Frost", "Iron", "Shadow", "Gilded", "Ink", "Storm", "Bone",
        "Paper", "Ivory", "Ash", "River", "Star", "Clockwork", "Thorn", "Void",
        "Sun", "Moss", "Cinder", "Glass"]
_NOUN = ["Fox", "Serpent", "Warden", "Moth", "Golem", "Heron", "Beetle",
         "Mare", "Drake", "Sentry", "Lynx", "Kite", "Boar", "Wisp", "Crane",
         "Tortoise", "Jackal", "Owl", "Ram", "Toad"]
_MAP_ADJ = ["Fractured", "Sunken", "Ashen", "Frozen", "Whispering", "Gilded",
            "Torn", "Flooded", "Withered", "Starlit", "Molten", "Pressed"]
_MAP_NOUN = ["Folio", "Margin", "Leaf", "Chapter", "Page", "Atlas", "Ledger",
             "Codex", "Manuscript", "Almanac"]


def _clamp(v, lo, hi):
    try:
        v = float(v)
    except Exception:
        return lo
    if math.isnan(v) or math.isinf(v):
        return lo
    return max(lo, min(hi, v))


def _slug(name, taken):
    base = "".join(c for c in name.lower() if c.isalnum()) or "card"
    base = base[:16]
    s = base
    i = 2
    while s in taken:
        s = f"{base}{i}"; i += 1
    return s


# ───────────────────────────── validation ─────────────────────────────────
def sanitize_card(raw, taken_ids=(), rng=None):
    """Coerce ANY dict-ish thing into a valid, in-bounds card record, or None.
    This is the single choke-point every generated card passes through, so the
    game only ever sees safe data regardless of how creative the model got."""
    rng = rng or random
    if not isinstance(raw, dict):
        return None
    name = str(raw.get("name") or raw.get("title") or "").strip()[:22]
    if not name:
        name = f"{rng.choice(_ADJ)} {rng.choice(_NOUN)}"
    cid = str(raw.get("id") or "").strip().lower()
    cid = "".join(c for c in cid if c.isalnum())
    if not cid or cid in taken_ids:
        cid = _slug(name, set(taken_ids))
    def gi(k, default):
        lo, hi = BOUNDS[k]
        return int(round(_clamp(raw.get(k, default), lo, hi)))
    card = {
        "id": cid, "name": name,
        "cost": gi("cost", 3),
        "hp": gi("hp", 400), "dmg": gi("dmg", 60), "spd": gi("spd", 60),
        "rng": gi("rng", 24), "size": gi("size", 22),
        "flying": bool(raw.get("flying", False)),
        "building": bool(raw.get("building", False)),
        "riverhop": bool(raw.get("riverhop", False)),
        "count": gi("count", 1),
    }
    # buildings don't move and don't fly
    if card["building"]:
        card["spd"] = 0; card["flying"] = False
    # a swarm (count>1) can't be a heavy tank — keep it fair
    if card["count"] > 1:
        card["hp"] = min(card["hp"], 360)
    paper = raw.get("paper")
    if isinstance(paper, (list, tuple)) and len(paper) >= 3:
        card["paper"] = [int(_clamp(paper[i], 0, 255)) for i in range(3)]
    else:
        card["paper"] = [rng.randint(70, 220) for _ in range(3)]
    ab = str(raw.get("ability") or raw.get("sigil") or "").strip().lower()
    card["ability"] = ab if ab in ABILITIES else rng.choice(ABILITIES)
    rar = str(raw.get("rarity") or "").strip().lower()
    card["rarity"] = rar if rar in RARITIES else "common"
    card["set"] = str(raw.get("set") or "living")[:24]
    card["rotation"] = bool(raw.get("rotation", True))
    card["blurb"] = str(raw.get("blurb") or raw.get("desc") or "")[:120]
    # optional origami field-model recipe: a list of known-good "fold" parts. It's
    # validated through the origami engine's own sanitizer, so a model authored by
    # an LLM (or logged from a card) can only ever reference safe primitives.
    model = raw.get("model")
    if isinstance(model, (list, tuple)) and model:
        try:
            import origami
            clean = origami.fold_sanitize(model)
            if clean:
                card["model"] = clean
        except Exception:
            pass
    return card


def sanitize_map(raw, rng=None):
    """Coerce a dict into a valid arena record the game can `set_arena` on."""
    rng = rng or random
    if not isinstance(raw, dict):
        raw = {}
    def col(v, fallback):
        if isinstance(v, (list, tuple)) and len(v) >= 3:
            return [int(_clamp(v[i], 0, 255)) for i in range(3)]
        return list(fallback)
    name = str(raw.get("name") or f"The {rng.choice(_MAP_ADJ)} {rng.choice(_MAP_NOUN)}")[:34]
    page = col(raw.get("page"), (196, 182, 150))
    # derived, harmonious palette so the page is always readable
    page_line = [max(0, c - 34) for c in page]
    desk = col(raw.get("desk"), (30, 22, 18))
    weather = raw.get("weather")
    if weather not in WEATHERS: weather = rng.choice(WEATHERS)
    hazard = raw.get("hazard")
    if hazard not in HAZARDS: hazard = rng.choice(HAZARDS)
    poi = raw.get("poi")
    if poi not in POIS: poi = None
    river = col(raw.get("river"), (26, 30, 52))
    river_hi = col(raw.get("river_hi"), [min(255, c + 90) for c in river])
    ambient = col(raw.get("ambient"), [max(20, c // 3 + 30) for c in page])
    return {
        "name": name, "chapter": str(raw.get("chapter") or "Living Page")[:28],
        "page": page, "page_line": page_line,
        "desk": desk, "desk_grain": [max(0, c - 8) for c in desk],
        "cover": col(raw.get("cover"), (64, 30, 24)),
        "river": river, "river_hi": river_hi,
        "ribbon": col(raw.get("ribbon"), (150, 60, 40)),
        "book": (300, 24, 680, 664), "top_y": 74, "bot_y": 664,
        "ambient": ambient, "light_main": (640, 110),
        "lights": [((640, 110), 980, [min(255, c + 40) for c in ambient]),
                   ((320, 640), 460, [min(255, c + 20) for c in ambient])],
        "candles": bool(raw.get("candles", rng.random() < 0.5)),
        "weather": weather, "hazard": hazard, "poi": poi,
        "blurb": str(raw.get("blurb") or "")[:120],
    }


# ─────────────────────────── procedural smiths ────────────────────────────
def procedural_card(rng=None, theme=None, taken_ids=(), cost=None):
    """Build a fair, playable card from templates — no network required."""
    rng = rng or random
    archetypes = [
        # (hp, dmg, spd, rng, size, flying, building, count, abilities)
        ("brute",   (900, 1600), (90, 150), (30, 46), (22, 28), (26, 32), 0, 0, 1, ["slam", "rampage", "towerbane", "charge"]),
        ("skirmish",(280, 520),  (60, 110), (58, 82), (22, 28), (16, 22), 0, 0, 1, ["pounce", "sharp", "rush", "doublestrike"]),
        ("flyer",   (200, 460),  (50, 100), (66, 92), (22, 30), (16, 22), 1, 0, 1, ["feather", "swiftdive", "blink", "snipe"]),
        ("swarm",   (120, 240),  (34, 64),  (60, 90), (18, 24), (14, 18), 0, 0, 3, ["relentless", "plague", "leap"]),
        ("support", (300, 560),  (30, 70),  (40, 150),(20, 30), (18, 24), 0, 0, 1, ["beacon", "radiance", "siphon", "chill"]),
        ("tower",   (900, 1600), (0, 90),   (0, 0),   (150, 220),(28, 32),0, 1, 1, ["artillery", "bulwark", "taunt", "thorns"]),
        ("caster",  (240, 460),  (40, 90),  (120, 200),(22, 30),(18, 24), 0, 0, 1, ["freeze", "inksplash", "molten", "warp"]),
    ]
    at = rng.choice(archetypes)
    (nm, hp_r, dmg_r, spd_r, rng_r, size_r, fly, bld, cnt, abis) = at
    def ri(r): return rng.randint(int(r[0]), int(r[1]))
    raw = {
        "name": f"{rng.choice(_ADJ)} {rng.choice(_NOUN)}",
        "hp": ri(hp_r), "dmg": ri(dmg_r), "spd": ri(spd_r) if not bld else 0,
        "rng": ri(rng_r) if rng_r[1] else 0, "size": ri(size_r),
        "flying": bool(fly), "building": bool(bld), "count": cnt,
        "ability": rng.choice(abis),
        "paper": [rng.randint(70, 220) for _ in range(3)],
        "set": "living", "rarity": rng.choice(RARITIES),
        "riverhop": rng.random() < 0.12,
    }
    # give the card a bespoke origami model matching its archetype so generated
    # creatures arrive with a fitting fold, not a generic silhouette
    raw["model"] = procedural_model(nm, rng)
    # cost roughly follows a stat budget so generated cards feel balanced
    budget = (raw["hp"] / 220 + raw["dmg"] / 60 + (raw["rng"] / 90) +
              (0.6 if raw["flying"] else 0) + (raw["count"] - 1) * 0.5)
    raw["cost"] = int(_clamp(cost if cost else round(budget * 0.9), *BOUNDS["cost"]))
    if theme:
        words = [w for w in str(theme).replace(",", " ").split() if w.isalpha()]
        adj = next((w.title() for w in words if len(w) > 2), rng.choice(_ADJ))
        raw["name"] = f"{adj[:12]} {rng.choice(_NOUN)}"
        raw["set"] = str(theme)[:24]
    return sanitize_card(raw, taken_ids, rng)


def procedural_model(archetype, rng=None):
    """Build an origami fold recipe (list of known-good parts) that fits an
    archetype. Returns raw parts; sanitize_card runs them through
    origami.fold_sanitize so they're always safe. Purely offline."""
    rng = rng or random
    j = lambda a, b: round(rng.uniform(a, b), 2)
    def part(p, at, size=1.0, ang=0.0, tone=1.0, h=0.5, count=None):
        o = {"part": p, "at": [round(at[0], 2), round(at[1], 2)],
             "size": size, "ang": ang, "tone": tone, "h": h}
        if count is not None:
            o["count"] = count
        return o
    if archetype == "flyer":
        return [part("slab", [0, 0.05], j(0.7, 0.9), h=0.4),
                part("wing", [-0.25, -0.2], j(1.0, 1.3), j(0.4, 0.7), 0.9),
                part("wing", [0.45, -0.15], j(1.0, 1.25), -j(0.3, 0.6), 1.05),
                part("wedge", [0.95, -0.12], j(0.4, 0.6), -0.1, 1.1),
                part("tail", [-0.9, 0.05], j(0.6, 0.85), 0.25, 0.85)]
    if archetype == "swarm":
        return [part("slab", [0, 0.05], j(0.6, 0.75), h=0.4),
                part("wedge", [0.7, -0.25], j(0.35, 0.5), -0.15, 1.05),
                part("spine", [-0.15, -0.15], j(0.4, 0.55), tone=0.85,
                     count=rng.randint(2, 4)),
                part("leg", [-0.45, 0.38], 0.35, h=0.4), part("leg", [0.45, 0.38], 0.35, h=0.4)]
    if archetype == "tower":
        return [part("shell", [0, -0.1], j(1.0, 1.2), h=j(0.8, 1.0)),
                part("panel", [0, 0.2], 0.9, tone=0.7),
                part("spine", [-0.1, -0.5], 0.5, tone=1.1, count=rng.randint(3, 5)),
                part("wedge", [0, -0.7], j(0.4, 0.55), 0.0, 1.1)]
    if archetype == "caster" or archetype == "support":
        return [part("slab", [0, 0.1], j(0.65, 0.8), h=0.6),
                part("wedge", [0, -0.55], j(0.45, 0.6), 0.0, 1.1),   # hooded head
                part("crest", [0, -0.35], j(0.5, 0.65), h=0.4, tone=1.05),
                part("tusk", [-0.55, -0.2], j(0.6, 0.8), 0.5, 1.1),  # staff
                part("orb", [-0.6, -0.55], 0.14, tone=1.3)]
    if archetype == "brute":
        return [part("slab", [0, 0], j(1.0, 1.15), h=j(0.6, 0.7)),
                part("leg", [-0.6, 0.4], 0.55, h=0.5), part("leg", [0.6, 0.4], 0.55, h=0.5),
                part("wedge", [0.8, -0.32], j(0.7, 0.85), -0.18, 1.08),
                part("tusk", [0.92, 0.0], j(0.7, 1.0), j(0.3, 0.45)),
                part("crest", [-0.1, -0.45], j(0.7, 0.9), h=0.5, tone=0.95)]
    # skirmish / default: fierce low quadruped
    return [part("slab", [0, 0], j(0.85, 1.0), h=0.5),
            part("leg", [-0.6, 0.4], 0.45, h=0.5), part("leg", [0.55, 0.4], 0.45, h=0.5),
            part("wedge", [0.82, -0.35], j(0.65, 0.8), -0.15, 1.08),
            part("crest", [0.0, -0.4], j(0.35, 0.55), h=0.35, tone=1.05),
            part("tail", [-0.9, -0.1], j(0.55, 0.75), j(0.35, 0.55), 0.9)]


def procedural_map(rng=None, theme=None):
    rng = rng or random
    palettes = [
        dict(page=(196, 182, 150), river=(26, 30, 52), cover=(64, 30, 24), weather=None, hazard=None),
        dict(page=(150, 156, 150), river=(60, 120, 140), cover=(52, 66, 62), weather="mist", hazard="waterfalls"),
        dict(page=(214, 176, 118), river=(176, 142, 86), cover=(120, 84, 40), weather="sandstorm", hazard="quicksand"),
        dict(page=(36, 42, 62), river=(20, 26, 48), cover=(24, 28, 52), weather="aurora", hazard=None),
        dict(page=(150, 116, 96), river=(50, 24, 16), cover=(66, 30, 20), weather="embers", hazard="lava"),
        dict(page=(196, 206, 168), river=(30, 60, 46), cover=(44, 66, 34), weather=None, hazard=None, poi="inktubs"),
    ]
    p = dict(rng.choice(palettes))
    p["name"] = f"The {rng.choice(_MAP_ADJ)} {rng.choice(_MAP_NOUN)}"
    if theme:
        p["name"] = f"The {str(theme).title().split()[0][:12]} {rng.choice(_MAP_NOUN)}"
    return sanitize_map(p, rng)


# ────────────────────────────── LLM client ────────────────────────────────
class DirectorConfig:
    """Where/how to reach an OpenAI-compatible chat endpoint. All optional —
    with no config the system runs fully procedural (offline)."""
    def __init__(self, base_url=None, api_key=None, model=None, provider="openai",
                 enabled=False, temperature=0.9, timeout=30):
        self.base_url = (base_url or "https://api.openai.com/v1").rstrip("/")
        self.api_key = api_key or ""
        self.model = model or "gpt-4o-mini"
        self.provider = provider          # "openai" | "ollama" | "anthropic" | …
        self.enabled = bool(enabled)
        self.temperature = temperature
        self.timeout = timeout

    @classmethod
    def from_env(cls):
        """Read PAPERBOUND_LLM_* (falls back to OPENAI_*). Enabled only if a key
        or a localhost base-url (Ollama/LM Studio) is present."""
        base = os.environ.get("PAPERBOUND_LLM_BASE") or os.environ.get("OPENAI_BASE_URL")
        key = os.environ.get("PAPERBOUND_LLM_KEY") or os.environ.get("OPENAI_API_KEY")
        model = os.environ.get("PAPERBOUND_LLM_MODEL") or os.environ.get("OPENAI_MODEL")
        prov = os.environ.get("PAPERBOUND_LLM_PROVIDER", "openai")
        local = bool(base and ("localhost" in base or "127.0.0.1" in base))
        return cls(base_url=base, api_key=key, model=model, provider=prov,
                   enabled=bool(key) or local)


class LLMClient:
    """Minimal OpenAI-compatible chat client (stdlib urllib; no deps). Also
    speaks Anthropic's messages API when provider='anthropic'."""
    def __init__(self, cfg: DirectorConfig):
        self.cfg = cfg

    def available(self):
        return _NET and self.cfg.enabled and bool(self.cfg.base_url)

    def complete(self, system, user, max_tokens=700):
        """Blocking call — ALWAYS invoke from a worker thread. Returns text or
        None. Never raises; failures degrade to None so callers fall back."""
        if not self.available():
            return None
        try:
            if self.cfg.provider == "anthropic":
                return self._anthropic(system, user, max_tokens)
            return self._openai(system, user, max_tokens)
        except Exception:
            return None

    def _post(self, url, headers, payload):
        data = json.dumps(payload).encode()
        req = urllib.request.Request(url, data=data, headers=headers, method="POST")
        with urllib.request.urlopen(req, timeout=self.cfg.timeout) as r:
            return json.loads(r.read().decode())

    def _openai(self, system, user, max_tokens):
        url = f"{self.cfg.base_url}/chat/completions"
        headers = {"Content-Type": "application/json"}
        if self.cfg.api_key:
            headers["Authorization"] = f"Bearer {self.cfg.api_key}"
        payload = {
            "model": self.cfg.model,
            "messages": [{"role": "system", "content": system},
                         {"role": "user", "content": user}],
            "temperature": self.cfg.temperature,
            "max_tokens": max_tokens,
        }
        # ask for JSON mode where supported (OpenAI, vLLM); harmless elsewhere
        payload["response_format"] = {"type": "json_object"}
        try:
            d = self._post(url, headers, payload)
        except Exception:
            payload.pop("response_format", None)      # server rejected it; retry
            d = self._post(url, headers, payload)
        return d["choices"][0]["message"]["content"]

    def _anthropic(self, system, user, max_tokens):
        url = f"{self.cfg.base_url}/messages" if "anthropic" in self.cfg.base_url \
            else "https://api.anthropic.com/v1/messages"
        headers = {"Content-Type": "application/json",
                   "x-api-key": self.cfg.api_key,
                   "anthropic-version": "2023-06-01"}
        payload = {"model": self.cfg.model or "claude-3-5-haiku-latest",
                   "max_tokens": max_tokens, "system": system,
                   "messages": [{"role": "user", "content": user}]}
        d = self._post(url, headers, payload)
        return "".join(b.get("text", "") for b in d.get("content", []))


def _extract_json(text):
    """Pull the first JSON object out of a model reply (handles ```json fences,
    prose wrappers, trailing commentary)."""
    if not text:
        return None
    t = text.strip()
    if "```" in t:
        t = t.split("```", 2)[1]
        if t.lstrip().lower().startswith("json"):
            t = t.split("\n", 1)[1] if "\n" in t else t
    # find the outermost {...}
    a, b = t.find("{"), t.rfind("}")
    if a == -1 or b == -1 or b <= a:
        return None
    try:
        return json.loads(t[a:b + 1])
    except Exception:
        return None


# ───────────────────────────────── smiths ─────────────────────────────────
_CARD_SYS = (
    "You are the loremaster of Paperbound, an origami paper-and-ink lane battler. "
    "Invent ONE playable troop or building card. Respond with ONLY a JSON object, "
    "no prose. Fields: id (lowercase slug), name, cost (1-9), hp (80-2000), "
    "dmg (0-320), spd (0-110; 0 for buildings), rng (16-240), size (14-34), "
    "flying (bool), building (bool), riverhop (bool), count (1-6), "
    "paper ([r,g,b] 0-255), ability (one of: " + ", ".join(ABILITIES) + "), "
    "rarity (common/uncommon/rare/epic/legendary), blurb (short flavor), and "
    "model. The 'model' is this creature's origami field sculpture: an array of "
    "3-8 folded parts, each {part, at:[x,y], size, ang, tone, h}. part is one of: "
    "slab (body block), panel (flat plate), wedge (head/beak/fin/horn), wing "
    "(flapping wing), spine (row of scales/quills; add count 2-6), leg (support), "
    "crest (mane/crown/plume), tusk (tusk/horn/antenna pair), tail, shell (dome), "
    "orb (eye/light). at is body-local, x -1.4..1.4 (facing), y up-negative "
    "-1.4..1.4; size ~0.3-1.3; ang radians; tone 0.6-1.3 shade; h 0-1 extrude. "
    "Compose a fitting silhouette: e.g. a flyer = slab + two wings + beak wedge + "
    "tail; a tank = shell + panel + spines; a beast = slab + legs + head wedge + "
    "tail. Balance cost against power. Buildings have spd 0."
)
_MAP_SYS = (
    "You are the cartographer of Paperbound. Invent ONE battlefield 'page'. "
    "Respond with ONLY a JSON object. Fields: name, chapter, page ([r,g,b] paper "
    "tone), desk ([r,g,b]), cover ([r,g,b]), river ([r,g,b]), ribbon ([r,g,b]), "
    "candles (bool), weather (one of: none, lightning, embers, snow, aurora, "
    "sand, sandstorm, gulls, mist), hazard (one of: none, lava, fissures, "
    "quicksand, tide, gears, waterfalls), blurb (short flavor)."
)


class CardSmith:
    def __init__(self, cfg=None):
        self.cfg = cfg or DirectorConfig()
        self.llm = LLMClient(self.cfg)

    def procedural(self, rng=None, theme=None, taken_ids=(), cost=None):
        return procedural_card(rng, theme, taken_ids, cost)

    def request(self, prompt, on_done, taken_ids=(), rng=None):
        """Async: generate a card (LLM if available, else procedural) and call
        on_done(card|None) from a worker thread. Never blocks the caller."""
        rng = rng or random
        def _work():
            card = None
            if self.llm.available():
                txt = self.llm.complete(_CARD_SYS, prompt or "Invent a surprising card.")
                card = sanitize_card(_extract_json(txt), taken_ids, rng)
            if card is None:
                card = procedural_card(rng, theme=prompt, taken_ids=taken_ids)
            try:
                on_done(card)
            except Exception:
                pass
        threading.Thread(target=_work, daemon=True).start()


class MapSmith:
    def __init__(self, cfg=None):
        self.cfg = cfg or DirectorConfig()
        self.llm = LLMClient(self.cfg)

    def procedural(self, rng=None, theme=None):
        return procedural_map(rng, theme)

    def request(self, prompt, on_done, rng=None):
        rng = rng or random
        def _work():
            mp = None
            if self.llm.available():
                txt = self.llm.complete(_MAP_SYS, prompt or "Invent a striking page.")
                mp = sanitize_map(_extract_json(txt), rng)
            if mp is None:
                mp = procedural_map(rng, theme=prompt)
            try:
                on_done(mp)
            except Exception:
                pass
        threading.Thread(target=_work, daemon=True).start()


# ───────────────────────────── the Director ───────────────────────────────
class Director:
    """The AI game-master. On its own cadence it inspects a JSON snapshot of the
    battle and emits *events* that vary the page to keep players adapting:

        {"kind": "card",  "card": <card dict>, "team": 0|1|None}
        {"kind": "map",   "map":  <arena dict>}
        {"kind": "twist", "twist": "weather"|"hazard"|"frenzy"|"gift_ink"|…,
                          "value": <...>, "text": "on-screen announcement"}

    The game drains these with drain() on the main loop and applies them safely.
    With an LLM configured the Director *asks the model* what to do next given
    the state; offline it uses a weighted heuristic. Either way the payloads are
    validated before they leave this module."""
    def __init__(self, cfg=None, seed=None):
        self.cfg = cfg or DirectorConfig()
        self.llm = LLMClient(self.cfg)
        self.card_smith = CardSmith(self.cfg)
        self.map_smith = MapSmith(self.cfg)
        self.rng = random.Random(seed)
        self._q = queue.Queue()
        self._stop = threading.Event()
        self._thread = None
        self.period = float(os.environ.get("PAPERBOUND_DIRECTOR_PERIOD", "14"))
        self.intensity = 1.0            # ramps up as a match drags on

    # -- lifecycle ---------------------------------------------------------
    def start(self, get_state):
        """get_state() -> a small JSON-able dict describing the battle."""
        if self._thread and self._thread.is_alive():
            return
        self._stop.clear()
        self._thread = threading.Thread(target=self._loop, args=(get_state,), daemon=True)
        self._thread.start()

    def stop(self):
        self._stop.set()

    def drain(self):
        """Non-blocking: yield all pending events (call on the main loop)."""
        out = []
        while True:
            try:
                out.append(self._q.get_nowait())
            except queue.Empty:
                break
        return out

    def push(self, ev):
        self._q.put(ev)

    # -- the loop ----------------------------------------------------------
    def _loop(self, get_state):
        # a small opening gift so the page feels alive immediately
        time.sleep(2.0)
        while not self._stop.is_set():
            try:
                state = {}
                try:
                    state = get_state() or {}
                except Exception:
                    state = {}
                ev = self.decide(state)
                if ev:
                    self._q.put(ev)
            except Exception:
                pass
            # cadence tightens as intensity climbs (more chaos late-game)
            wait = max(5.0, self.period / max(0.5, self.intensity))
            self._stop.wait(wait)

    # -- decision making ---------------------------------------------------
    def decide(self, state):
        """Pick the next twist. Tries the LLM; falls back to a heuristic."""
        self.intensity = min(3.0, 1.0 + float(state.get("elapsed", 0)) / 90.0)
        if self.llm.available():
            ev = self._decide_llm(state)
            if ev:
                return ev
        return self._decide_heuristic(state)

    def _decide_heuristic(self, state):
        roll = self.rng.random()
        taken = tuple(state.get("card_ids", ()))
        # weight: mostly small twists, sometimes a fresh card, rarely a new map
        if roll < 0.42:
            card = procedural_card(self.rng, taken_ids=taken)
            team = self.rng.choice([0, 1, None])
            return {"kind": "card", "card": card, "team": team,
                    "text": f"The page inks a new figure: {card['name']}"}
        if roll < 0.62:
            w = self.rng.choice([w for w in WEATHERS if w])
            return {"kind": "twist", "twist": "weather", "value": w,
                    "text": f"The weather turns: {w}"}
        if roll < 0.78:
            h = self.rng.choice([h for h in HAZARDS if h])
            return {"kind": "twist", "twist": "hazard", "value": h,
                    "text": f"A hazard spreads across the page: {h}"}
        if roll < 0.9:
            return {"kind": "twist", "twist": "gift_ink",
                    "value": self.rng.choice([2, 3, 4]),
                    "text": "The inkwell brims — bonus ink for all."}
        mp = procedural_map(self.rng)
        return {"kind": "map", "map": mp,
                "text": f"The page turns to {mp['name']}"}

    def _decide_llm(self, state):
        sys = (
            "You are the Director of Paperbound, a live game-master keeping a duel "
            "surprising and fair. Given a JSON battle snapshot, choose ONE action "
            "to make players adapt. Respond with ONLY a JSON object of the form "
            '{"kind":"twist","twist":"weather|hazard|gift_ink|frenzy","value":...,'
            '"text":"short dramatic announcement"} OR {"kind":"card","team":0|1|null,'
            '"card":{...full card...}} OR {"kind":"map","map":{...full map...}}. '
            "Keep it fair; escalate slowly."
        )
        txt = self.llm.complete(sys, json.dumps(state)[:2000], max_tokens=800)
        obj = _extract_json(txt)
        if not isinstance(obj, dict):
            return None
        kind = obj.get("kind")
        if kind == "card":
            card = sanitize_card(obj.get("card"), tuple(state.get("card_ids", ())), self.rng)
            if not card:
                return None
            team = obj.get("team")
            team = team if team in (0, 1) else None
            return {"kind": "card", "card": card, "team": team,
                    "text": str(obj.get("text") or f"A new figure: {card['name']}")[:80]}
        if kind == "map":
            mp = sanitize_map(obj.get("map"), self.rng)
            return {"kind": "map", "map": mp,
                    "text": str(obj.get("text") or f"The page turns to {mp['name']}")[:80]}
        if kind == "twist":
            tw = str(obj.get("twist") or "")
            val = obj.get("value")
            if tw == "weather" and val not in WEATHERS: val = self.rng.choice([w for w in WEATHERS if w])
            if tw == "hazard" and val not in HAZARDS: val = self.rng.choice([h for h in HAZARDS if h])
            if tw == "gift_ink":
                try: val = int(_clamp(val, 1, 6))
                except Exception: val = 3
            if tw not in ("weather", "hazard", "gift_ink", "frenzy"):
                return None
            return {"kind": "twist", "twist": tw, "value": val,
                    "text": str(obj.get("text") or "The page shifts…")[:80]}
        return None


if __name__ == "__main__":
    # offline self-test — exercises validation, procedural smiths, the director
    rng = random.Random(1)
    n_ok = 0
    n_modeled = 0
    try:
        import origami as _og
        _og_ok = True
    except Exception:
        _og_ok = False
    for _ in range(500):
        c = procedural_card(rng)
        assert c and BOUNDS["cost"][0] <= c["cost"] <= BOUNDS["cost"][1]
        assert BOUNDS["hp"][0] <= c["hp"] <= BOUNDS["hp"][1]
        assert c["ability"] in ABILITIES
        if c["building"]: assert c["spd"] == 0
        # every generated card should now carry a valid origami model
        if _og_ok and isinstance(c.get("model"), list) and c["model"]:
            assert all(p["part"] in _og.PART_TYPES for p in c["model"])
            n_modeled += 1
        n_ok += 1
    # adversarial inputs the validator must survive
    for bad in [None, {}, {"hp": "banana"}, {"cost": 1e9, "size": -5},
                {"name": "x" * 999, "paper": [999, -1, "z"]},
                {"ability": "DELETE ALL"}, {"hp": float("inf")}, [1, 2, 3],
                {"count": 99, "hp": 99999}]:
        c = sanitize_card(bad, rng=rng)
        if c is not None:
            assert BOUNDS["hp"][0] <= c["hp"] <= BOUNDS["hp"][1]
            assert c["ability"] in ABILITIES
            assert 0 <= c["paper"][0] <= 255
    for _ in range(100):
        m = procedural_map(rng)
        assert m["weather"] in WEATHERS and m["hazard"] in HAZARDS
    # director heuristic emits only valid events
    d = Director(seed=2)
    kinds = set()
    for _ in range(200):
        ev = d._decide_heuristic({"elapsed": rng.randint(0, 200), "card_ids": ["sabertooth"]})
        assert ev["kind"] in ("card", "map", "twist")
        kinds.add(ev["kind"])
    # extraction from messy model output
    assert _extract_json('```json\n{"a":1}\n```') == {"a": 1}
    assert _extract_json('Sure! {"name":"Fox","hp":300} hope that helps') == {"name": "Fox", "hp": 300}
    assert _extract_json("no json here") is None
    print(f"✅ ai_director self-test: {n_ok} procedural cards ({n_modeled} with "
          f"valid origami models), adversarial inputs survived, director "
          f"kinds={sorted(kinds)}")
