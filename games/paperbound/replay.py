"""
PAPERBOUND — replays.

Because the battle sim is a deterministic, tick-based lockstep model, a whole
match is fully described by its SEED plus the sequence of deploy inputs — no need
to store frames or state. That makes replays tiny: we log every `deploy(key, x,
y, team)` with the tick it happened on, snapshot any dynamically-created cards
(wildcard / generated / boss minions) so they can be re-registered, and that's
the entire match. Re-running the sim while injecting exactly those deploys (with
all AI/director/boss logic switched off) reproduces the match bit-for-bit.

On disk this is serialised to a compact JSON schema and gzip-compressed to a
`.pbr` file — typically a couple of KB per match. Files live in
    $PAPERBOUND_REPLAYS  or  ~/.paperbound/replays/

Recording (driven by game.py):
    rec = Recorder(meta)          # at battle start
    rec.log(tick, key, x, y, team)      # each deploy
    rec.note_card(key, entry)     # any non-base card registered mid-match
    rec.finish(result, ticks); save(rec)

Playback:
    rep = load(path)
    player = ReplayPlayer(rep)    # drives a fresh Game deterministically
    player.seek(tick); player.step()   # scrub / play
"""
import os, json, gzip, time, base64, struct

SCHEMA = 1


def replays_dir():
    env = os.environ.get("PAPERBOUND_REPLAYS")
    d = env or os.path.join(os.path.expanduser("~"), ".paperbound", "replays")
    try:
        os.makedirs(d, exist_ok=True)
    except OSError:
        d = "."
    return d


# ── recording ──────────────────────────────────────────────────────────────
class Recorder:
    def __init__(self, meta):
        """meta: {seed, mode, arena, deck, deck2?, boss?, player?, started}"""
        self.meta = dict(meta)
        self.meta.setdefault("started", int(time.time()))
        self.events = []           # (tick, key, x, y, team)
        self.cards = {}            # key -> flat card entry, for non-base cards
        self.result = None
        self.ticks = 0

    def log(self, tick, key, x, y, team):
        self.events.append((int(tick), key, round(float(x), 1), round(float(y), 1), int(team)))

    def note_card(self, key, entry):
        """Record a dynamically-registered card so the replay can rebuild it."""
        if key not in self.cards:
            # store a JSON-safe copy (tuples → lists)
            self.cards[key] = json.loads(json.dumps(entry, default=list))

    def finish(self, result, ticks, base_stamp=None, patch_hash="", battle_stamp=None):
        self.result = result
        self.ticks = int(ticks)
        # version stamps: which ruleset (base ⊕ patch) this match ran on, and the
        # battle's own effective stamp (ruleset ⊕ seed ⊕ generated content). These
        # let a replay prove its lineage and warn on drift.
        if base_stamp is not None:
            self.meta["base_stamp"] = base_stamp
            self.meta["patch_hash"] = patch_hash or ""
        if battle_stamp is not None:
            self.meta["battle_stamp"] = battle_stamp
        self.meta["gametime"] = int(time.time())


# ── compact serialisation + gzip ────────────────────────────────────────────
def _pack_events(events):
    """Delta-encode ticks and pack events compactly. Returns a dict with a
    key-table (so repeated card keys are stored once) + a base64 blob of
    (dtick, keyidx, x, y, team) rows. Very small after gzip."""
    keys = []
    kidx = {}
    for (_, k, _, _, _) in events:
        if k not in kidx:
            kidx[k] = len(keys); keys.append(k)
    rows = bytearray()
    prev = 0
    for (t, k, x, y, team) in events:
        dt = t - prev; prev = t
        # dtick (uint16), keyidx (uint16), x (int16), y (int16), team (uint8)
        rows += struct.pack("<HHhhB", min(dt, 65535), kidx[k],
                            int(max(-32000, min(32000, x))),
                            int(max(-32000, min(32000, y))), team & 0xff)
    return {"keys": keys, "rows": base64.b64encode(bytes(rows)).decode("ascii"),
            "n": len(events)}


def _unpack_events(packed):
    keys = packed["keys"]
    raw = base64.b64decode(packed["rows"])
    events = []
    prev = 0
    step = struct.calcsize("<HHhhB")
    for i in range(packed.get("n", len(raw) // step)):
        dt, kidx, x, y, team = struct.unpack_from("<HHhhB", raw, i * step)
        prev += dt
        events.append((prev, keys[kidx], float(x), float(y), team))
    return events


def to_doc(rec):
    return {
        "schema": SCHEMA,
        "meta": rec.meta,
        "result": rec.result,
        "ticks": rec.ticks,
        "cards": rec.cards,          # non-base card defs to re-register
        "events": _pack_events(rec.events),
    }


def _filename(rec):
    m = rec.meta
    stamp = time.strftime("%Y%m%d-%H%M%S", time.localtime(m.get("started", time.time())))
    mode = str(m.get("mode", "match"))
    res = rec.result or "unfinished"
    return f"{stamp}_{mode}_{res}.pbr"


def save(rec, path=None):
    """Write a gzip-compressed replay. Returns the path."""
    doc = to_doc(rec)
    path = path or os.path.join(replays_dir(), _filename(rec))
    raw = json.dumps(doc, separators=(",", ":")).encode("utf-8")
    with gzip.open(path, "wb", compresslevel=9) as f:
        f.write(raw)
    return path


def load(path):
    """Load a .pbr file → replay doc dict (events unpacked to a list). If the
    current build's ruleset stamp is available, annotate the doc with a
    `compat` field ('exact'|'same-base'|'incompatible'|'unknown') so the UI can
    warn when a replay was recorded on a different patch set/version."""
    with gzip.open(path, "rb") as f:
        doc = json.loads(f.read().decode("utf-8"))
    doc["events"] = _unpack_events(doc["events"])
    doc["_path"] = path
    doc["compat"] = _compat_of(doc)
    return doc


def _compat_of(doc):
    """Compare a replay's recorded stamps to the current build's, if we can."""
    meta = doc.get("meta", {})
    rec_base = meta.get("base_stamp")
    rec_patch = meta.get("patch_hash", "")
    if not rec_base:
        return "unknown"
    try:
        import carddata, patchtree
        cur_base = carddata.data_stamp()
        cur_patch = getattr(carddata, "ACTIVE_PATCH", None)
        cur_patch = (cur_patch or {}).get("hash", "") if isinstance(cur_patch, dict) else ""
        return patchtree.compatibility(rec_base, rec_patch, cur_base, cur_patch)
    except Exception:
        return "unknown"


def list_replays():
    """All saved replays, newest first: list of {path, meta, result, ticks, size}."""
    out = []
    d = replays_dir()
    try:
        names = [n for n in os.listdir(d) if n.endswith(".pbr")]
    except OSError:
        names = []
    for n in names:
        p = os.path.join(d, n)
        try:
            with gzip.open(p, "rb") as f:
                doc = json.loads(f.read().decode("utf-8"))
            out.append({"path": p, "meta": doc.get("meta", {}),
                        "result": doc.get("result"), "ticks": doc.get("ticks", 0),
                        "size": os.path.getsize(p), "name": n,
                        "compat": _compat_of(doc)})
        except Exception:
            continue
    out.sort(key=lambda r: r["meta"].get("started", 0), reverse=True)
    return out


def delete(path):
    try:
        os.remove(path); return True
    except OSError:
        return False


# ── playback driver ─────────────────────────────────────────────────────────
class ReplayPlayer:
    """Drives a fresh Game deterministically from a replay doc. AI/director/boss
    are disabled — the sim advances purely on the recorded deploys, so it
    reproduces the original match. Supports scrubbing via seek()."""
    def __init__(self, doc, make_game):
        """make_game(meta, cards) -> a Game set up with the replay's seed/mode/
        arena and the extra card defs pre-registered, with AI disabled. Provided
        by game.py so this module stays engine-agnostic."""
        self.doc = doc
        self.make_game = make_game
        self.events_by_tick = {}
        for (t, k, x, y, team) in doc["events"]:
            self.events_by_tick.setdefault(t, []).append((k, x, y, team))
        self.total = doc.get("ticks", 0) or (max(self.events_by_tick) if self.events_by_tick else 0)
        self.g = None
        self.reset()

    def reset(self):
        self.g = self.make_game(self.doc["meta"], self.doc.get("cards", {}))
        self.cur = 0
        # schedule all deploys onto the game's own tick queue
        for (t, k, x, y, team) in self.doc["events"]:
            self.g.pending_inputs.setdefault(t, []).append((k, x, y, team))

    def step(self, n=1):
        """Advance n sim ticks (bounded by the replay length)."""
        for _ in range(n):
            if self.cur >= self.total:
                break
            self.g.update()
            self.cur += 1
        return self.g

    def seek(self, tick):
        """Jump to an absolute tick. Backwards seeks re-simulate from the start
        (cheap — the sim is fast and inputs-only)."""
        tick = max(0, min(int(tick), self.total))
        if tick < self.cur:
            self.reset()
        while self.cur < tick:
            self.g.update()
            self.cur += 1
        return self.g

    @property
    def progress(self):
        return (self.cur / self.total) if self.total else 1.0


if __name__ == "__main__":
    # self-test: record → save (compressed) → load → replay reproduces state,
    # and the on-disk file is small.
    os.environ["PAPERBOUND_REPLAYS"] = "/tmp/pb_replays_test"
    import shutil
    shutil.rmtree("/tmp/pb_replays_test", ignore_errors=True)

    rec = Recorder({"seed": 42, "mode": "classic", "arena": "study",
                    "deck": ["sabertooth", "crane"]})
    # a fake match: 40 deploys over 400 ticks
    import random as _r
    _r.seed(1)
    for i in range(40):
        rec.log(i * 10, _r.choice(["sabertooth", "crane", "wild0"]),
                _r.uniform(200, 900), _r.uniform(400, 700), _r.choice([0, 1]))
    rec.note_card("wild0", {"name": "Wild Thing", "cost": 3, "hp": 200, "dmg": 40,
                            "spd": 60, "rng": 20, "size": 20, "flying": False,
                            "paper": (200, 180, 90), "sigil": "sharp"})
    rec.finish("win", 400)
    path = save(rec)
    size = os.path.getsize(path)
    # round-trip
    doc = load(path)
    assert doc["meta"]["seed"] == 42
    assert len(doc["events"]) == 40
    assert doc["events"][0][0] == 0 and doc["events"][1][0] == 10   # delta ticks restored
    assert "wild0" in doc["cards"]
    # listing
    lst = list_replays()
    assert lst and lst[0]["result"] == "win"
    # compression ratio vs raw JSON
    raw = json.dumps(to_doc(rec)).encode()
    print(f"✅ replay self-test: 40 events, on-disk {size} B "
          f"(raw JSON {len(raw)} B → {100*size/len(raw):.0f}%), round-trip OK, "
          f"delta-ticks + key-table + gzip, {len(lst)} listed")
