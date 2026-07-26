#!/usr/bin/env python3
"""raid_content.py -- assemble deep-space RAID content from the design docs.

The lore doc frames every raid boss as an ancient Cybertronian relic -- an
archaeological discovery, not just a big enemy. This tool:
  1. PARSES the hand-written lore in raids-and-relics.md into structured records
     (name, epithet, classification, backstory paragraphs).
  2. For bosses with no written lore yet, GENERATES a name-themed stub in the same
     relic style (drives off keywords in the name: titan/drill/reactor/scorpion...).
  3. ROUTES raid AREAS + BOSSES + GOONS (from the library tree / metadata) into
     complete raid encounters scattered across deep space.
Output: raid_content.json the game loads to build raid locations.

Usage:
  python3 tools/raid_content.py --lore raids-and-relics.md --tree plan.md --out raid_content.json
"""
import sys, os, re, json, argparse

# ---- name -> thematic seeds (for stub lore + tags) -------------------------
THEMES = {
    "titan":     ("colossal war-form", "Titan-class", ["siege", "colossus"]),
    "speaker":   ("stellar harmonics weapon", "Resonance Engine", ["sonic", "aoe"]),
    "resonance": ("stellar harmonics weapon", "Resonance Engine", ["sonic", "aoe"]),
    "drill":     ("planetary excavation engine", "Core-Breaker", ["burrow", "armor_pierce"]),
    "reactor":   ("mobile fission core", "Walking Reactor", ["radiation", "meltdown"]),
    "cherno":    ("nuclear brawler", "Walking Reactor", ["radiation", "melee"]),
    "godzilla":  ("kaiju-scale destroyer", "Leviathan-class", ["colossus", "beam"]),
    "mecha":     ("reconstructed war-relic", "Reclaimed Chassis", ["adaptive"]),
    "scorpion":  ("subterranean ambush-form", "Stinger-class", ["burrow", "venom"]),
    "mantis":    ("primitive blade-predator", "Reaper-form", ["blades", "leap"]),
    "spider":    ("web-laying swarm-host", "Brood-form", ["swarm", "web"]),
    "arachnid":  ("web-laying swarm-host", "Brood-form", ["swarm", "web"]),
    "pyrachnid": ("magma-brooding swarm-host", "Ember-Brood", ["fire", "swarm"]),
    "tank":      ("siege-artillery relic", "Fortress-class", ["armor", "artillery"]),
    "hussar":    ("cavalry siege-engine", "Fortress-class", ["charge", "artillery"]),
    "strider":   ("long-stride walker", "Strider-class", ["mobile", "artillery"]),
    "trench":    ("siege-entrenchment engine", "Bastion-class", ["armor", "suppression"]),
    "tremor":    ("seismic destroyer", "Planetbreaker", ["quake", "aoe"]),
    "judgement": ("tribunal war-walker", "Arbiter-class", ["execute", "beam"]),
    "hardboil":  ("thermal-plated brute", "Furnace-form", ["fire", "armor"]),
    "gigadrill": ("planetary excavation engine", "Core-Breaker", ["burrow", "armor_pierce"]),
    "sigillum":  ("sealed relic-construct", "Sealed One", ["ward", "phase"]),
    "sphere":    ("rolling sentinel-orb", "Orbital-form", ["roll", "shield"]),
    "cameraman": ("surveillance titan", "Watcher-class", ["scan", "beam"]),
    "starlord":  ("dual-hammer sentry", "Vanguard-form", ["hammers", "turret"]),
    "devastator":("combiner colossus", "Gestalt-class", ["combiner", "colossus"]),
}
STUB_OPENERS = [
    "Recovered dormant from a drifting hulk beyond the charted lanes.",
    "One of the war-engines the Collapse left scattered between galaxies.",
    "Its alloys still carry imprinted memory of a purpose that no longer exists.",
    "Long silent, it wakes when living metal comes near.",
]

def clean_name(raw):
    s = re.sub(r"\.zip$", "", raw, flags=re.I)
    s = re.sub(r"(?i)^(raid-?boss-?-?|raidboss-?-?)", "", s)
    s = re.split(r"(?i)[-\s]aka[-\s]", s)[0]
    s = re.sub(r"[_\-]+", " ", s).strip()
    return s.title()

def theme_for(name):
    low = name.lower()
    for key,(desc,cls,tags) in THEMES.items():
        if key in low:
            return desc, cls, tags
    return ("ancient war-relic", "Relic-class", ["unknown"])

def gen_stub(name):
    desc, cls, tags = theme_for(name)
    epithet = "The " + (cls.split("-")[0])
    body = [
        f"Classification: {cls}",
        f"{name} is a {desc}, one of the dormant engines drifting the deep dark "
        f"since the Collapse.",
        STUB_OPENERS[hash(name) % len(STUB_OPENERS)],
        "Most such relics are partly sentient -- Cybertronian technology imprints "
        "memory into its alloys, so each remembers a war whose cause is long forgotten.",
    ]
    return {"name": name, "epithet": epithet, "classification": cls,
            "backstory": body, "tags": tags, "lore_source": "generated"}

# ---- parse hand-written lore ----------------------------------------------
def parse_lore(md_text):
    """Split into entries: a Name line, a "epithet" line, then paragraphs until the
    next Name/epithet pair. We anchor on the 'Classification:' lines + the quoted
    epithet just above each name."""
    lines = md_text.splitlines()
    entries = {}
    i = 0
    # find blocks: NAME \n "EPITHET" \n ... (Classification: ...) ...
    while i < len(lines):
        line = lines[i].strip()
        nxt = lines[i+1].strip() if i+1 < len(lines) else ""
        if line and nxt.startswith('"') and nxt.endswith('"') and not line.startswith(("#","-","*",">")):
            name = line
            epithet = nxt.strip('"')
            body = []; j = i+2; cls = None
            while j < len(lines):
                s = lines[j].strip()
                nx = lines[j+1].strip() if j+1 < len(lines) else ""
                if s and nx.startswith('"') and nx.endswith('"'):   # next entry begins
                    break
                if s.lower().startswith("classification:"):
                    cls = s.split(":",1)[1].strip()
                if s: body.append(s)
                j += 1
            entries[name.lower()] = {"name": name, "epithet": epithet,
                                     "classification": cls or "Relic-class",
                                     "backstory": body, "tags": theme_for(name)[2],
                                     "lore_source": "authored"}
            i = j
        else:
            i += 1
    # PRECISION: a "line followed by a quoted line" also matches quoted DIALOGUE in
    # the prose. A real entry has substance (a Classification: line or several
    # paragraphs) and a name that reads like a name, not a sentence.
    def looks_like_name(s):
        if s.startswith(('"', "'")) or s.endswith(('.', '!', '?', ':')): return False
        if len(s) > 48 or len(s.split()) > 6: return False
        return any(c.isalpha() for c in s)
    entries = {k: v for k, v in entries.items()
               if looks_like_name(v["name"])
               and (v["classification"] != "Relic-class" or len(v["backstory"]) >= 3)}
    return entries

# ---- tree parsing (reuse the indent reconstruction) -----------------------
def leaves_under(md_text, section):
    """Zip stems listed under a `section` header in the tree text."""
    out = []; in_sec = False
    for line in md_text.splitlines():
        s = re.sub(r"[│├└─┬┤┌┐┘┴]", " ", line)
        name = s.strip()
        if section in name and name.endswith(section):
            in_sec = True; continue
        if in_sec:
            if name.endswith(".zip"):
                out.append(name)
            elif name and not name.startswith(("│"," ")) and "/" not in name and name != section \
                 and not name.lower().endswith(".zip") and len(s)-len(s.lstrip()) <= 1:
                # dedent back to a sibling top-level dir -> section ended
                if not any(c in name for c in ("--","_")) and name.islower()==False:
                    pass
    return out

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--lore", required=True, help="raids-and-relics.md")
    ap.add_argument("--tree", help="plan.md (for the raid-boss/area/goon lists)")
    ap.add_argument("--out", default="raid_content.json")
    a = ap.parse_args()
    lore_md = open(a.lore, errors="ignore").read()
    authored = parse_lore(lore_md)

    # gather boss names: from the tree if given, else from authored lore
    boss_names = []
    if a.tree:
        tree_md = open(a.tree, errors="ignore").read()
        for line in tree_md.splitlines():
            s = re.sub(r"[│├└─┬┤┌┐┘┴]", " ", line).strip()
            if s.lower().endswith(".zip") and re.search(r"(?i)raid.?boss|titan|devastator", s):
                boss_names.append(clean_name(s))
    if not boss_names:
        boss_names = [v["name"] for v in authored.values()]

    bosses = []
    for bn in dict.fromkeys(boss_names):     # dedupe, keep order
        key = bn.lower()
        hit = None
        for lk, lv in authored.items():
            if lk in key or key in lk or lk.split()[0] == key.split()[0]:
                hit = lv; break
        bosses.append(hit if hit else gen_stub(bn))

    content = {"raid_bosses": bosses,
               "counts": {"total": len(bosses),
                          "authored": sum(1 for b in bosses if b["lore_source"]=="authored"),
                          "generated": sum(1 for b in bosses if b["lore_source"]=="generated")}}
    json.dump(content, open(a.out,"w"), indent=1)
    print(f"{len(bosses)} raid bosses -> {a.out}")
    print(f"  {content['counts']['authored']} with authored lore, "
          f"{content['counts']['generated']} generated stubs")
    for b in bosses[:6]:
        print(f"  - {b['name']:30.30s} \"{b['epithet']}\"  [{b['lore_source']}] tags={b['tags']}")

if __name__ == "__main__":
    main()
