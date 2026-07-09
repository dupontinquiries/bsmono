"""
PAPERBOUND — patch system.

Lets people host and share their own balance/content patches for LAN tournaments.
A *patch* is a small JSON overlay on top of the base game data (cards, abilities,
arenas, tower troops, modes). It is **content-addressed**: its identity is the
SHA-256 of its canonical body, so the same content always hashes the same, can be
verified for integrity, and can't be silently tampered with. Human-friendly names
sit on top of the hash, never replace it.

Model (one patch = one dict):
    {
      "meta": {
        "name": "Featherweight Cup",
        "author": "…",
        "desc": "cheap-unit tournament ruleset",
        "created": 1720000000,
        "base": "<hash of the patch this was forked from, or '' for base game>",
        "hash": "<sha256 of the canonical body — filled in by finalize()>",
      },
      "body": {                      # the actual overlay, all sections optional
        "cards":       { "<id>": { …partial or full card fields… } },
        "abilities":   { "<id>": {…} },
        "arenas":      { "<id>": {…} },
        "tower_troops":{ "<id>": {…} },
        "modes":       { "<id>": {…} },
        "remove":      { "cards": ["<id>", …] }        # optional deletions
      }
    }

Operations (the UX the tournament flow needs):
    • subscribe (temp override) — apply_patch() over base data for this session,
    • clone (keep)              — library_save() to the local patch library,
    • fork                      — fork() a new patch descended from an existing one.

Distribution: a host serves its library over HTTP (serve_patches / the game's
existing transport); clients list + fetch by hash.

Storage: patches live in  $PAPERBOUND_PATCHES  or  ~/.paperbound/patches/  as
`<slug>@<shorthash>.patch.json`. The filename carries the short hash so files are
self-identifying and de-duplicated.
"""
import os, json, time, hashlib, re, urllib.request

SCHEMA = 1
SECTIONS = ("cards", "abilities", "arenas", "tower_troops", "modes")


# ── storage location ───────────────────────────────────────────────────────
def patches_dir():
    env = os.environ.get("PAPERBOUND_PATCHES")
    if env:
        d = env
    else:
        d = os.path.join(os.path.expanduser("~"), ".paperbound", "patches")
    try:
        os.makedirs(d, exist_ok=True)
    except OSError:
        d = "."
    return d


# ── canonical hashing ──────────────────────────────────────────────────────
def _canonical(obj):
    """Deterministic JSON: sorted keys, no whitespace, stable separators. This is
    what we hash, so two equal patches always produce the same digest."""
    return json.dumps(obj, sort_keys=True, separators=(",", ":"),
                      ensure_ascii=True).encode("utf-8")


def hash_body(body):
    """SHA-256 of a patch body (kept for diffing/dedup of pure content)."""
    return hashlib.sha256(_canonical(body)).hexdigest()


def hash_patch(patch):
    """The patch's IDENTITY hash: content body PLUS the identifying meta
    (name, author, base lineage, created). This means a fork — same body, new
    name/lineage — gets a distinct identity, while re-saving the exact same patch
    stays stable. The `hash` field itself is excluded from the digest."""
    meta = patch.get("meta", {})
    ident = {
        "name": meta.get("name", ""),
        "author": meta.get("author", ""),
        "base": meta.get("base", ""),
        "created": meta.get("created", 0),
        "body": patch.get("body", {}),
    }
    return hashlib.sha256(_canonical(ident)).hexdigest()


def short_hash(h):
    return (h or "")[:12]


def slug(name):
    s = re.sub(r"[^a-z0-9]+", "-", (name or "patch").lower()).strip("-")
    return s[:32] or "patch"


def finalize(patch):
    """Compute + stamp the patch's content hash. Returns the patch (mutated)."""
    body = patch.setdefault("body", {})
    meta = patch.setdefault("meta", {})
    meta.setdefault("name", "Untitled Patch")
    meta.setdefault("author", "")
    meta.setdefault("desc", "")
    meta.setdefault("created", int(time.time()))
    meta.setdefault("base", "")
    meta["schema"] = SCHEMA
    meta["hash"] = hash_patch(patch)
    return patch


def verify(patch):
    """Does the stamped hash actually match? (integrity check.)"""
    try:
        return patch.get("meta", {}).get("hash") == hash_patch(patch)
    except Exception:
        return False


# ── building / forking ─────────────────────────────────────────────────────
def new_patch(name, author="", desc="", body=None):
    return finalize({"meta": {"name": name, "author": author, "desc": desc},
                     "body": body or {}})


def fork(patch, new_name=None, author=""):
    """Create a fresh patch descended from `patch`: copies the body, records the
    parent's hash as `base`, and re-hashes under a new identity."""
    parent = patch.get("meta", {})
    body = json.loads(json.dumps(patch.get("body", {})))     # deep copy
    child = {"meta": {"name": new_name or (parent.get("name", "Patch") + " (fork)"),
                      "author": author or parent.get("author", ""),
                      "desc": parent.get("desc", ""),
                      "base": parent.get("hash", "")},
             "body": body}
    return finalize(child)


def diff_to_patch(base_cards, modified_cards, name, author="", desc=""):
    """Build a patch body from the difference between two CARDS-style dicts.
    Only changed/added cards go into the overlay; removed ones go to remove."""
    body = {"cards": {}}
    for k, v in modified_cards.items():
        if k not in base_cards or base_cards[k] != v:
            body["cards"][k] = v
    removed = [k for k in base_cards if k not in modified_cards]
    if removed:
        body["remove"] = {"cards": removed}
    return new_patch(name, author, desc, body)


# ── applying an overlay ────────────────────────────────────────────────────
def apply_patch(base_doc, patch):
    """Return a NEW data doc = base_doc with the patch overlaid. Non-destructive
    (base_doc is not mutated). `base_doc` is a dict shaped like cards.json
    (keys: cards, abilities, variants, …). Partial card fields are merged onto
    the existing card; unknown cards are added whole."""
    doc = json.loads(json.dumps(base_doc))          # deep copy
    body = patch.get("body", {}) if isinstance(patch, dict) else {}
    for section in SECTIONS:
        overlay = body.get(section)
        if not isinstance(overlay, dict):
            continue
        target = doc.setdefault(section, {})
        for key, val in overlay.items():
            if isinstance(val, dict) and isinstance(target.get(key), dict):
                merged = dict(target[key]); merged.update(val)   # field-level merge
                target[key] = merged
            else:
                target[key] = val
    # deletions
    rem = body.get("remove", {})
    if isinstance(rem, dict):
        for section, keys in rem.items():
            tgt = doc.get(section, {})
            for k in (keys or []):
                tgt.pop(k, None)
    doc.setdefault("_patched", {})
    doc["_patched"] = {"name": patch.get("meta", {}).get("name", "?"),
                       "hash": patch.get("meta", {}).get("hash", "")}
    return doc


# ── local library ──────────────────────────────────────────────────────────
def _filename(patch):
    h = patch.get("meta", {}).get("hash") or hash_patch(patch)
    return f"{slug(patch.get('meta', {}).get('name'))}@{short_hash(h)}.patch.json"


def library_save(patch):
    """Persist a patch to the local library (clone/keep). Returns the path."""
    finalize(patch)
    path = os.path.join(patches_dir(), _filename(patch))
    with open(path, "w") as f:
        json.dump(patch, f, indent=2)
    return path


def library_list():
    """All locally-kept patches, newest first: list of patch dicts."""
    out = []
    d = patches_dir()
    try:
        for fn in os.listdir(d):
            if fn.endswith(".patch.json"):
                try:
                    with open(os.path.join(d, fn)) as f:
                        p = json.load(f)
                    if isinstance(p, dict) and "body" in p:
                        p["_path"] = os.path.join(d, fn)
                        out.append(p)
                except Exception:
                    continue
    except OSError:
        pass
    out.sort(key=lambda p: p.get("meta", {}).get("created", 0), reverse=True)
    return out


def library_get(h):
    """Find a locally-kept patch by (short or full) hash."""
    for p in library_list():
        ph = p.get("meta", {}).get("hash", "")
        if ph == h or short_hash(ph) == short_hash(h):
            return p
    return None


def library_delete(h):
    p = library_get(h)
    if p and p.get("_path") and os.path.exists(p["_path"]):
        try:
            os.remove(p["_path"]); return True
        except OSError:
            return False
    return False


def summary(patch):
    """A compact listing row for UI: name, author, short hash, counts, lineage."""
    meta = patch.get("meta", {})
    body = patch.get("body", {})
    counts = {s: len(body.get(s, {})) for s in SECTIONS if body.get(s)}
    return {
        "name": meta.get("name", "?"), "author": meta.get("author", ""),
        "desc": meta.get("desc", ""), "hash": short_hash(meta.get("hash", "")),
        "full_hash": meta.get("hash", ""),
        "base": short_hash(meta.get("base", "")) if meta.get("base") else "",
        "counts": counts,
        "changes": sum(counts.values()),
    }


# ── LAN distribution (over the game's HTTP transport) ──────────────────────
def library_index():
    """A lightweight index of the local library for announcing to LAN clients:
    a list of summaries (no bodies), so a client can browse before fetching."""
    return [summary(p) for p in library_list()]


def fetch_patch(base_url, full_hash, timeout=6.0):
    """Pull one patch body from a host by hash. Verifies the hash on arrival, so
    a corrupted/tampered transfer is rejected. Returns the patch or None."""
    try:
        url = base_url.rstrip("/") + "/patch/" + full_hash
        with urllib.request.urlopen(url, timeout=timeout) as r:
            p = json.loads(r.read().decode())
        if verify(p) and p.get("meta", {}).get("hash") == full_hash:
            return p
    except Exception:
        pass
    return None


def fetch_index(base_url, timeout=4.0):
    """List the patches a host offers (summaries)."""
    try:
        url = base_url.rstrip("/") + "/patches"
        with urllib.request.urlopen(url, timeout=timeout) as r:
            return json.loads(r.read().decode())
    except Exception:
        return []


if __name__ == "__main__":
    # self-test: hashing, overlay, fork, library round-trip, integrity
    os.environ["PAPERBOUND_PATCHES"] = "/tmp/pb_patches_test"
    import shutil
    shutil.rmtree("/tmp/pb_patches_test", ignore_errors=True)

    base = {"cards": {"sabertooth": {"cost": 4, "hp": 300, "dmg": 60},
                      "crane": {"cost": 2, "hp": 150, "dmg": 40}},
            "abilities": {}}
    # a patch that buffs sabertooth hp and adds a new card, removes crane
    p = new_patch("Test Cup", author="me", desc="buff cats",
                  body={"cards": {"sabertooth": {"hp": 500},
                                  "newcat": {"cost": 3, "hp": 200, "dmg": 55}},
                        "remove": {"cards": ["crane"]}})
    assert verify(p), "hash mismatch on fresh patch"
    h1 = p["meta"]["hash"]
    # identical FULL patch (same name+meta+body) → identical hash (stable re-save)
    p_same = json.loads(json.dumps(p))
    assert hash_patch(p_same) == h1, "identical patch must hash the same"
    # a different name is a different identity (fork-friendly)
    p2 = new_patch("Different Name", body=p["body"])
    assert p2["meta"]["hash"] != h1, "different name should be distinct identity"
    # apply overlay
    doc = apply_patch(base, p)
    assert doc["cards"]["sabertooth"]["hp"] == 500          # field merged
    assert doc["cards"]["sabertooth"]["dmg"] == 60          # untouched field kept
    assert "newcat" in doc["cards"]                         # added
    assert "crane" not in doc["cards"]                      # removed
    assert base["cards"]["sabertooth"]["hp"] == 300         # base untouched
    # tamper detection
    p_bad = json.loads(json.dumps(p)); p_bad["body"]["cards"]["sabertooth"]["hp"] = 9999
    assert not verify(p_bad), "tamper should fail verification"
    # fork records lineage + new identity
    child = fork(p, "Test Cup Remix", author="you")
    assert child["meta"]["base"] == h1 and child["meta"]["hash"] != h1
    # library save/list/get/delete
    path = library_save(p)
    assert os.path.exists(path)
    lst = library_list(); assert any(x["meta"]["hash"] == h1 for x in lst)
    got = library_get(short_hash(h1)); assert got and got["meta"]["hash"] == h1
    idx = library_index(); assert idx and idx[0]["hash"] == short_hash(h1)
    assert library_delete(h1)
    print(f"✅ patches self-test: content-hash stable ({short_hash(h1)}), overlay "
          f"merge+add+remove OK, tamper rejected, fork lineage OK, library "
          f"save/list/get/delete OK")
