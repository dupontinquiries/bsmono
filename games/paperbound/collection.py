"""
Paperbound collection & discovery persistence — the local "backend".

Saves the player's owned cards (including rare variants), which cards they've
discovered, and the running state of discovery events to a JSON save file so
progress persists across sessions. This is a local, file-based store: robust,
offline-friendly, and easy to reset. The API is deliberately small so the game
just calls grant()/owns()/save().

Save location: $PAPERBOUND_SAVE if set, else ~/.paperbound/collection.json
"""
import json, os, time, random

def _save_path():
    env = os.environ.get("PAPERBOUND_SAVE")
    if env:
        return env
    root = os.path.join(os.path.expanduser("~"), ".paperbound")
    try:
        os.makedirs(root, exist_ok=True)
    except OSError:
        root = "."
    return os.path.join(root, "collection.json")

SAVE_PATH = _save_path()

# in-memory state (mirrored to disk)
STATE = {
    "schema": 1,
    "owned": {},          # card_key -> count   (card_key may be "id" or "id#variant")
    "discovered": [],     # ordered list of card_keys first seen
    "events": {},         # discovery-event bookkeeping (cooldowns, seeds)
    "log": [],            # recent discovery log entries {key, how, t}
    "stats": {"finds": 0, "matches": 0},
    "prefs": {},          # misc persisted preferences (e.g. chosen tower troop)
}
_LOADED = False


def load():
    global STATE, _LOADED
    try:
        with open(SAVE_PATH) as f:
            data = json.load(f)
        if isinstance(data, dict) and "owned" in data:
            STATE.update(data)
    except (OSError, ValueError):
        pass
    _LOADED = True
    return STATE


def save():
    try:
        tmp = SAVE_PATH + ".tmp"
        with open(tmp, "w") as f:
            json.dump(STATE, f, indent=1)
        os.replace(tmp, SAVE_PATH)
        return True
    except OSError:
        return False


def owns(card_key):
    return STATE["owned"].get(card_key, 0) > 0

def count(card_key):
    return STATE["owned"].get(card_key, 0)

def owned_keys():
    return [k for k, n in STATE["owned"].items() if n > 0]

def discovered():
    return list(STATE["discovered"])


def grant(card_key, how="found", n=1):
    """Add n copies of a card (or variant) to the collection. Returns a dict
    describing the grant, incl. whether it was newly discovered."""
    prev = STATE["owned"].get(card_key, 0)
    STATE["owned"][card_key] = prev + n
    is_new = prev == 0
    if is_new and card_key not in STATE["discovered"]:
        STATE["discovered"].append(card_key)
    STATE["stats"]["finds"] += n
    STATE["log"].insert(0, {"key": card_key, "how": how, "t": int(time.time()), "new": is_new})
    STATE["log"] = STATE["log"][:40]
    save()
    return {"key": card_key, "new": is_new, "count": STATE["owned"][card_key], "how": how}


def record_match():
    STATE["stats"]["matches"] += 1
    save()


# ── discovery-event bookkeeping (cooldowns so events don't spam) ──
def event_ready(name, cooldown_s):
    last = STATE["events"].get(name, 0)
    return (time.time() - last) >= cooldown_s

def mark_event(name):
    STATE["events"][name] = time.time()
    save()


def recent_log(n=8):
    return STATE["log"][:n]


def get_pref(key, default=None):
    return STATE.get("prefs", {}).get(key, default)

def set_pref(key, value):
    STATE.setdefault("prefs", {})[key] = value
    save()


def reset():
    """Wipe the collection (for a fresh start / testing)."""
    global STATE
    STATE = {"schema": 1, "owned": {}, "discovered": [], "events": {},
             "log": [], "stats": {"finds": 0, "matches": 0}}
    save()


load()

if __name__ == "__main__":
    os.environ["PAPERBOUND_SAVE"] = "/tmp/pb_test_collection.json"
    globals()["SAVE_PATH"] = os.environ["PAPERBOUND_SAVE"]
    reset()
    print("grant:", grant("sabertooth", "cache"))
    print("grant variant:", grant("mammoth#gilded", "forest"))
    print("re-grant (dup):", grant("sabertooth", "table"))
    print("owns sabertooth:", owns("sabertooth"), "count:", count("sabertooth"))
    print("discovered:", discovered())
    print("event ready(cache,60):", event_ready("cache", 60))
    mark_event("cache")
    print("event ready after mark:", event_ready("cache", 60))
    save()
    # reload from disk to prove persistence
    STATE["owned"] = {}
    load()
    print("reloaded owned:", STATE["owned"])
    print("✅ collection persistence works")
