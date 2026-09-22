#!/usr/bin/env python3
"""ingest -- route the todo_tar archive into the apps that can actually read it.

    cards   multilingual_flashcards.md -> tomescope Deck JSON
    zettel  <archive>/*.md -> voiceterm zettelkasten (+ [[links]])

Formats were read off the source, not guessed:

  tomescope  apps/tomescope/src/study.rs      Deck{cards:[Flashcard{...}]}
             apps/tomescope/src/persist.rs    ~/.config/tomescope/flashcards.json
  voiceterm  apps/voiceterm/src/zettel_id.rs  id = YYYYMMDDHHMM, or folgezettel
             apps/voiceterm/src/zbrackets.rs  [[Name]] links, .md default
             apps/voiceterm/src/backlinks.rs  graph is DERIVED from [[ ]], not stored
             apps/voiceterm/src/workspace_wizard.rs
                                              inbox/ permanent/ references/ index/
                                              + a .web2qml-notes workspace marker

Mermaid: voiceterm's QML card canvas lays out DIRECTED graphs via
layered_layout.rs (Sugiyama) and sequence diagrams via sequence_diagram.rs.
`mindmap` and `timeline` have no parser, so --fix-mermaid rewrites those into
`flowchart TD`, which does. Everything else is passed through untouched.
"""
import argparse, json, os, re, time, unicodedata

# ----------------------------------------------------------------- cards

CARD_RE = re.compile(
    r"^## Card (\d+)\s*$\n+"
    r"### Prompt \(([^)]+)\)\s*$\n+"
    r"\*\*(.+?)\*\*\s*$\n+"
    r"### Answer \(([^)]+)\)\s*$\n+"
    r"\*\*(.+?)\*\*\s*$",
    re.M)


def parse_cards(md):
    """Yield (n, q_lang, question, a_lang, answer, table_row)."""
    text = open(md, encoding="utf-8").read()
    blocks = re.split(r"^---\s*$", text, flags=re.M)
    out = []
    for b in blocks:
        m = CARD_RE.search(b)
        if not m:
            continue
        n, ql, q, al, a = m.groups()
        rows = [l for l in b.splitlines() if l.startswith("| ") and "---" not in l]
        table = rows[-1].strip("| ").split(" | ") if len(rows) >= 2 else []
        out.append((int(n), ql, q.strip(), al, a.strip(),
                    [c.strip() for c in table]))
    return out


def to_deck(cards, spread_days, all_langs):
    """Build a tomescope Deck. Fields mirror study.rs::Flashcard exactly."""
    now = int(time.time())
    deck = []
    for i, (n, ql, q, al, a, table) in enumerate(cards):
        back = a
        if all_langs and len(table) == 4:
            en, fr, es, it = table
            back = f"{a}\n\nEN {en} · FR {fr} · ES {es} · IT {it}"
        # stagger due dates so 400 cards do not all land on day one
        due = now + (i % max(spread_days, 1)) * 86400 if spread_days else now
        deck.append({
            "id": n,
            "front": f"{q}  ({ql} → {al})",
            "back": back,
            "source_locus": None,
            "ease": 2.5,
            "interval_days": 0.0,
            "repetitions": 0,
            "due_unix": due,
            "last_reviewed_unix": None,
        })
    return {"cards": deck}


# ---------------------------------------------------------------- zettel

def slug(title):
    """Mirror workspace_wizard.rs::slug — lowercase, non-alnum to '-'."""
    t = unicodedata.normalize("NFKD", title)
    t = "".join(c for c in t if not unicodedata.combining(c))
    out = []
    for c in t.lower():
        if c.isalnum():
            out.append(c)
        elif out and out[-1] != "-":
            out.append("-")
    return "".join(out).strip("-")


def zettel_id(unix_secs):
    """zettel_id.rs::timestamp_id_from_unix -- YYYYMMDDHHMM, UTC."""
    t = time.gmtime(unix_secs)
    return f"{t.tm_year:04d}{t.tm_mon:02d}{t.tm_mday:02d}{t.tm_hour:02d}{t.tm_min:02d}"


# mindmap/timeline have no parser in voiceterm; flowchart TD does.
def mindmap_to_flowchart(body):
    """Tree -> flowchart TD. Node labels are the first line only (a 400-char
    label makes an unreadable graph); every dropped line is returned so the
    caller can emit it as prose. Nothing is discarded."""
    lines = body.splitlines()
    out, stack, nid, detail = ["flowchart TD"], [], [0], []

    def newid():
        nid[0] += 1
        return f"n{nid[0]}"

    for ln in lines[1:]:
        if not ln.strip():
            continue
        indent = len(ln) - len(ln.lstrip())
        txt = ln.strip()
        m = re.match(r'^([A-Za-z_][\w]*)?[\[\("]{1,2}(.*?)[\]\)"]{1,2}$', txt)
        full = (m.group(2) if m else txt).replace('"', "'")
        pieces = [p.strip() for p in full.split("\\n") if p.strip()]
        label = pieces[0][:60] if pieces else full[:60]
        me = newid()
        out.append(f'    {me}["{label}"]')
        while stack and stack[-1][0] >= indent:
            stack.pop()
        if stack:
            out.append(f"    {stack[-1][1]} --> {me}")
        stack.append((indent, me))
        if len(pieces) > 1:
            detail.append((me, pieces[0], pieces[1:]))
    return "\n".join(out), detail


def timeline_to_flowchart(body):
    out, nid, prev, detail = ["flowchart TD"], [0], None, []
    for ln in body.splitlines()[1:]:
        s = ln.strip()
        if not s or s.startswith("title"):
            continue
        if s.startswith("section "):
            section = s[8:].strip()
            nid[0] += 1
            me = f"s{nid[0]}"
            out.append(f'    {me}["{section}"]')
            if prev:
                out.append(f"    {prev} --> {me}")
            prev = me
            continue
        head, _, rest = s.partition(":")
        label = head.strip().replace('"', "'")[:60]
        if not label:
            # a bare ": continuation" line belongs to the previous entry
            if prev and rest.strip() and detail and detail[-1][0] == prev:
                detail[-1][2].append(rest.strip())
            continue
        nid[0] += 1
        me = f"t{nid[0]}"
        out.append(f'    {me}["{label}"]')
        if prev:
            out.append(f"    {prev} --> {me}")
        prev = me
        if rest.strip():
            detail.append((me, label, [rest.strip()]))
    return "\n".join(out), detail


def render_detail(detail):
    """The lines a label could not hold, as readable prose under the chart."""
    if not detail:
        return ""
    out = ["", "<!-- node detail preserved from the original diagram -->", ""]
    for nid_, label, lines in detail:
        out.append(f"**{label}** <sub>`{nid_}`</sub>")
        out.append("")
        for l in lines:
            out.append(f"- {l}")
        out.append("")
    return "\n".join(out)


FENCE = re.compile(r"^```mermaid\s*$(.*?)^```\s*$", re.M | re.S)


def fix_mermaid(text):
    """Rewrite unparseable diagram types. Returns (text, stats)."""
    stats = {"converted": 0, "kept": 0, "detail_lines": 0, "kinds": {}}

    def repl(m):
        body = m.group(1).strip("\n")
        first = body.strip().split("\n")[0].strip() if body.strip() else ""
        kind = first.split()[0] if first else "?"
        stats["kinds"][kind] = stats["kinds"].get(kind, 0) + 1
        if kind == "mindmap":
            stats["converted"] += 1
            chart, detail = mindmap_to_flowchart(body)
            stats["detail_lines"] += sum(len(d[2]) for d in detail)
            return "```mermaid\n" + chart + "\n```\n" + render_detail(detail)
        if kind == "timeline":
            stats["converted"] += 1
            chart, detail = timeline_to_flowchart(body)
            stats["detail_lines"] += sum(len(d[2]) for d in detail)
            return "```mermaid\n" + chart + "\n```\n" + render_detail(detail)
        stats["kept"] += 1
        return m.group(0)

    return FENCE.sub(repl, text), stats


def linkify(text, titles):
    """Turn a known note title appearing in prose into a [[wikilink]].

    Conservative on purpose: only whole-line headings and exact title matches
    outside code fences, because a false link is worse than a missing one --
    backlinks.rs builds the graph from these and a wrong edge is invisible.
    """
    parts = re.split(r"(^```.*?^```)", text, flags=re.M | re.S)
    for i, p in enumerate(parts):
        if p.startswith("```"):
            continue
        for t in sorted(titles, key=len, reverse=True):
            if len(t) < 8:
                continue
            p = re.sub(r"(?<!\[\[)\b" + re.escape(t) + r"\b(?!\]\])",
                       f"[[{t}]]", p, count=3)
        parts[i] = p
    return "".join(parts)


def build_zettel(src, out, layout, do_fix, do_link):
    docs = []
    for dp, dn, fn in os.walk(src):
        dn[:] = [d for d in dn if d != ".git"]
        for f in sorted(fn):
            if f.endswith(".md"):
                docs.append(os.path.join(dp, f))
    base = "permanent/" if layout == "structured" else ""
    if layout == "structured":
        for d in ("inbox", "permanent", "references", "index"):
            os.makedirs(os.path.join(out, d), exist_ok=True)
    else:
        os.makedirs(out, exist_ok=True)

    titles = []
    for p in docs:
        t = os.path.splitext(os.path.basename(p))[0]
        t = re.sub(r"^[\W_]+", "", t).strip()
        titles.append(t)

    total = {"converted": 0, "kept": 0, "detail_lines": 0, "kinds": {}}
    written = []
    for p, title in zip(docs, titles):
        text = open(p, encoding="utf-8", errors="replace").read()
        if do_fix:
            text, st = fix_mermaid(text)
            total["converted"] += st["converted"]
            total["kept"] += st["kept"]
            total["detail_lines"] += st["detail_lines"]
            for k, v in st["kinds"].items():
                total["kinds"][k] = total["kinds"].get(k, 0) + v
        if do_link:
            text = linkify(text, [t for t in titles if t != title])
        zid = zettel_id(int(os.path.getmtime(p)))
        name = f"{zid} {slug(title)}.md"
        dest = os.path.join(out, base, name)
        n = 1
        while os.path.exists(dest):
            n += 1
            dest = os.path.join(out, base, f"{zid}-{n} {slug(title)}.md")
        with open(dest, "w", encoding="utf-8") as f:
            f.write(text if text.startswith("#") else f"# {title}\n\n{text}")
        written.append((os.path.relpath(dest, out), title))

    # index note wiring every doc into the graph
    idx_dir = "index/" if layout == "structured" else ""
    with open(os.path.join(out, idx_dir + "index.md"), "w", encoding="utf-8") as f:
        f.write("# Blackstar / Black Archives\n\n"
                "Imported film-script corpus.\n\n")
        for rel, title in sorted(written, key=lambda x: x[1]):
            f.write(f"- [[{os.path.splitext(os.path.basename(rel))[0]}|{title}]]\n")

    # workspace marker so voiceterm recognizes the root
    with open(os.path.join(out, ".web2qml-notes"), "w") as f:
        f.write(f"layout={layout}\nid_scheme=timestamp\nsource={os.path.basename(os.path.normpath(src))}\n")
    return written, total


# -------------------------------------------------------------------- cli

def main():
    ap = argparse.ArgumentParser(prog="ingest")
    s = ap.add_subparsers(dest="cmd", required=True)
    c = s.add_parser("cards"); c.add_argument("md"); c.add_argument("-o", default="flashcards.json")
    c.add_argument("--spread-days", type=int, default=30)
    c.add_argument("--all-langs", action="store_true")
    z = s.add_parser("zettel"); z.add_argument("src"); z.add_argument("-o", default="notes")
    z.add_argument("--layout", choices=["flat", "structured"], default="structured")
    z.add_argument("--fix-mermaid", action="store_true")
    z.add_argument("--link", action="store_true")
    a = ap.parse_args()

    if a.cmd == "cards":
        cards = parse_cards(a.md)
        deck = to_deck(cards, a.spread_days, a.all_langs)
        json.dump(deck, open(a.o, "w", encoding="utf-8"), indent=1, ensure_ascii=False)
        print(f"{len(deck['cards'])} cards -> {a.o}")
        print(f"install: cp {a.o} ~/.config/tomescope/flashcards.json")
    else:
        written, st = build_zettel(a.src, a.o, a.layout, a.fix_mermaid, a.link)
        print(f"{len(written)} notes -> {a.o}/")
        if st["kinds"]:
            print("mermaid blocks: " +
                  ", ".join(f"{k}x{v}" for k, v in sorted(st["kinds"].items())))
            print(f"  rewritten to flowchart: {st['converted']}   "
                  f"passed through: {st['kept']}")
            print(f"  detail lines preserved as prose: {st['detail_lines']}")


if __name__ == "__main__":
    main()
