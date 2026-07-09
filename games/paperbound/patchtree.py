"""
PAPERBOUND — patch tree.

Ties together three ideas that already run on content-addressing:

  • networking      — a match is reproducible from a shared hash + inputs, and
                      peers verify integrity by comparing hashes,
  • wildcard gen    — battle content is generated deterministically from a seed,
                      so the *effective* data of a match is (ruleset ⊕ seed ⊕
                      generated cards), fully described by a hash,
  • patch lineage   — patches descend from a base (patches.py fork lineage).

…into ONE structure: a **patch tree**. The root is the base ruleset (cards.json,
via carddata.data_stamp). Applying a patch adds a child node. Every *battle* adds
a leaf: it stamps the exact effective data it ran on — base ⊕ active-patch ⊕ the
deterministically-generated cards (wildcard/boss/director) — plus its gametime.

Why a tree, per battle:
  • replays can prove which ruleset they were recorded on and warn on drift,
  • net peers can match by *patch set* (same ruleset lineage) before starting,
  • generated content is reproducible, so a battle node's stamp captures the
    whole world it was played in, not just the base version.

"gametime based": each node carries a gametime (unix seconds), so the tree is
temporally ordered and versions can be matched/sorted by when they entered play.

A stamp is a short content hash. Nodes:
    {stamp, parent, kind: base|patch|battle, name, gametime, meta}
The tree persists to  $PAPERBOUND_PATCHES/tree.json  so history accrues across
sessions.
"""
import os, json, time, hashlib

SCHEMA = 1


def _h(*parts):
    """Short content hash of the given JSON-able parts (order matters)."""
    blob = json.dumps(parts, sort_keys=True, separators=(",", ":"),
                      default=list).encode("utf-8")
    return hashlib.sha256(blob).hexdigest()[:16]


# ── the three stamp kinds ──────────────────────────────────────────────────
def ruleset_stamp(base_stamp, patch_hash=""):
    """Identity of the ACTIVE ruleset a battle uses: base data ⊕ active patch.
    Two builds match here iff they'd play by the same rules (patch set)."""
    return _h("ruleset", base_stamp, patch_hash or "")


def battle_stamp(ruleset, seed, generated_cards=None):
    """Identity of ONE battle's full effective world: the active ruleset, the
    match seed (which deterministically fixes wildcard/generated content), and a
    digest of the cards that were actually generated during it. Reproducible:
    the same (ruleset, seed, generated set) always yields this stamp."""
    gen = sorted((generated_cards or {}).keys())
    gen_digest = _h("gen", {k: (generated_cards or {})[k] for k in gen})
    return _h("battle", ruleset, int(seed) & 0xffffffff, gen_digest)


# ── the tree ───────────────────────────────────────────────────────────────
class PatchTree:
    def __init__(self):
        self.nodes = {}          # stamp -> node dict

    def _put(self, stamp, parent, kind, name, meta=None):
        node = self.nodes.get(stamp)
        if node is None:
            node = {"stamp": stamp, "parent": parent, "kind": kind,
                    "name": name, "gametime": int(time.time()), "meta": meta or {}}
            self.nodes[stamp] = node
        return node

    def add_base(self, base_stamp, name="base game"):
        return self._put(base_stamp, None, "base", name)

    def add_patch(self, base_stamp, patch_meta):
        """Add a patch node under the base (or under its fork parent if present).
        patch_meta is a patches.py meta dict {name, hash, base, …}."""
        self.add_base(base_stamp)
        rs = ruleset_stamp(base_stamp, patch_meta.get("hash", ""))
        parent = base_stamp
        # if this patch forks another that's in the tree, hang it off that node
        pbase = patch_meta.get("base")
        if pbase:
            for n in self.nodes.values():
                if n["kind"] == "patch" and n["meta"].get("patch_hash") == pbase:
                    parent = n["stamp"]; break
        return self._put(rs, parent, "patch", patch_meta.get("name", "patch"),
                         meta={"patch_hash": patch_meta.get("hash", ""),
                               "author": patch_meta.get("author", "")})

    def add_battle(self, base_stamp, patch_hash, seed, generated_cards=None,
                   result=None, name=None):
        """Add a battle leaf. Its parent is the active ruleset node (patch or
        base); its stamp captures ruleset ⊕ seed ⊕ generated content."""
        rs = ruleset_stamp(base_stamp, patch_hash)
        if patch_hash:
            # ensure a ruleset node exists for this patch set
            self._put(rs, base_stamp, "patch", "patch " + patch_hash[:8],
                      meta={"patch_hash": patch_hash})
        else:
            self.add_base(base_stamp)
            rs = base_stamp
        bs = battle_stamp(ruleset_stamp(base_stamp, patch_hash), seed, generated_cards)
        return self._put(bs, rs, "battle", name or f"battle {bs[:8]}",
                         meta={"seed": int(seed) & 0xffffffff, "result": result,
                               "generated": sorted((generated_cards or {}).keys())})

    def ancestry(self, stamp):
        """Chain from a node up to the root: [node, …, base]."""
        out = []
        cur = self.nodes.get(stamp)
        seen = set()
        while cur and cur["stamp"] not in seen:
            seen.add(cur["stamp"]); out.append(cur)
            cur = self.nodes.get(cur.get("parent"))
        return out

    def children(self, stamp):
        return [n for n in self.nodes.values() if n.get("parent") == stamp]

    def roots(self):
        return [n for n in self.nodes.values() if not n.get("parent")]

    # ── persistence ──
    def to_doc(self):
        return {"schema": SCHEMA, "nodes": list(self.nodes.values())}

    @classmethod
    def from_doc(cls, doc):
        t = cls()
        for n in (doc or {}).get("nodes", []):
            if "stamp" in n:
                t.nodes[n["stamp"]] = n
        return t


def _tree_path():
    base = os.environ.get("PAPERBOUND_PATCHES") or \
        os.path.join(os.path.expanduser("~"), ".paperbound", "patches")
    try:
        os.makedirs(base, exist_ok=True)
    except OSError:
        base = "."
    return os.path.join(base, "tree.json")


def load_tree():
    try:
        with open(_tree_path()) as f:
            return PatchTree.from_doc(json.load(f))
    except Exception:
        return PatchTree()


def save_tree(tree):
    try:
        with open(_tree_path(), "w") as f:
            json.dump(tree.to_doc(), f, separators=(",", ":"))
        return True
    except OSError:
        return False


# ── version matching (the whole point) ─────────────────────────────────────
def compatibility(base_a, patch_a, base_b, patch_b):
    """Compare two (base, patch) rulesets. Returns one of:
       'exact'       — identical ruleset, will play/replay bit-for-bit,
       'same-base'   — same base data, different patch set (cards differ),
       'incompatible'— different base data entirely (rules diverged).
    This is what a replay-load or a net-handshake uses to decide."""
    if base_a != base_b:
        return "incompatible"
    if ruleset_stamp(base_a, patch_a) == ruleset_stamp(base_b, patch_b):
        return "exact"
    return "same-base"


def describe(compat):
    return {
        "exact": "matches your version",
        "same-base": "same base, different patch",
        "incompatible": "different game version",
    }.get(compat, "unknown")


if __name__ == "__main__":
    os.environ["PAPERBOUND_PATCHES"] = "/tmp/pb_tree_test"
    import shutil
    shutil.rmtree("/tmp/pb_tree_test", ignore_errors=True)

    base = "b" * 16
    t = PatchTree()
    t.add_base(base)
    # a patch node
    pm = {"name": "Featherweight Cup", "hash": "patchAAAA", "base": ""}
    pnode = t.add_patch(base, pm)
    # two battles on the base, different seeds → distinct leaves
    b1 = t.add_battle(base, "", seed=111, result="win")
    b2 = t.add_battle(base, "", seed=222, result="lose")
    assert b1["stamp"] != b2["stamp"], "different seeds must give distinct battle stamps"
    # same seed + same ruleset + same generated set → SAME stamp (reproducible)
    b1b = battle_stamp(ruleset_stamp(base, ""), 111, {})
    assert b1b == b1["stamp"], "battle stamp must be reproducible"
    # a battle on the patch ruleset hangs under the patch node
    bp = t.add_battle(base, "patchAAAA", seed=111, result="win")
    anc = [n["kind"] for n in t.ancestry(bp["stamp"])]
    assert anc[0] == "battle" and "patch" in anc and anc[-1] == "base", anc
    # generated content changes the battle stamp (wildcard reproducibility)
    bp_gen = battle_stamp(ruleset_stamp(base, ""), 111, {"wildx": {"hp": 200}})
    assert bp_gen != b1["stamp"], "generated content must move the battle stamp"
    # persistence
    save_tree(t); t2 = load_tree()
    assert len(t2.nodes) == len(t.nodes)
    # matching
    assert compatibility(base, "", base, "") == "exact"
    assert compatibility(base, "p1", base, "p2") == "same-base"
    assert compatibility(base, "", "c" * 16, "") == "incompatible"
    print(f"✅ patchtree self-test: base+patch+{len([n for n in t.nodes.values() if n['kind']=='battle'])} "
          f"battle nodes, reproducible stamps, ancestry battle→patch→base OK, "
          f"gen-content moves stamp, persistence + matching (exact/same-base/"
          f"incompatible) OK")
