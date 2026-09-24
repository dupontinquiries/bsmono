#!/usr/bin/env python3
"""chatgrep -- search a Claude conversation export without unzipping it.

    index    build a SQLite FTS5 index from the .zip     (once, ~1 min)
    find     full-text search, ranked, with snippets
    show     print a hit with surrounding context
    list     conversations, biggest first

Works straight off the .zip -- 174 MB of markdown stays compressed on disk.
FTS5 handles phrases ("note graph"), OR, NEAR and prefix (zettel*). Note the
proximity form is FTS5's own -- NEAR(a b, 20) -- not Lucene's a NEAR/20 b,
which is a syntax error.

The index is CONTENTLESS: FTS5 stores the inverted index and no copy of the
text, because the .zip on disk is already the source of truth. That is
232.6 MB -> 64.2 MB on this corpus, which is the difference between viable
and not on a phone. The cost is that snippet() cannot work (there is no
stored text to snip), so snippets are reconstructed by re-reading the one
zip member a hit points at: measured 15-18 ms cold, and the member is then
cached for the rest of the session.

    python3 chatgrep.py index claude-msgs-md.zip
    python3 chatgrep.py find '"terminal workflow" OR zettel*'
    python3 chatgrep.py find 'NEAR(web2qml philosophy, 20)'
    python3 chatgrep.py show 12 --around 'notes root'
"""
import argparse, os, re, sqlite3, sys, zipfile

DB = "chats.db"
# Split each conversation into windows so a hit points somewhere, not at a
# 12 MB file. Chosen to be a few screens of text.
WIN = 4000
OVERLAP = 400


def build(zip_path, db_path):
    z = zipfile.ZipFile(zip_path)
    convos = [n for n in z.namelist() if n.endswith(".md") and n.count("/") == 1]
    if os.path.exists(db_path):
        os.remove(db_path)
    db = sqlite3.connect(db_path)
    db.executescript("""
      CREATE TABLE conv(id INTEGER PRIMARY KEY, name TEXT, path TEXT, bytes INT, chunks INT);
      -- A contentless FTS5 table stores NO column values -- not even ones
      -- marked UNINDEXED, which read back as NULL. So the locator lives in a
      -- plain table keyed on the same rowid: two integers per chunk.
      CREATE TABLE loc(rowid INTEGER PRIMARY KEY, conv_id INT, off INT);
      CREATE VIRTUAL TABLE chunk USING fts5(body, content='',
                                            tokenize='porter unicode61');
    """)
    for cid, n in enumerate(sorted(convos)):
        try:
            t = z.read(n).decode("utf-8", "replace")
        except Exception as e:
            print(f"  skip {n}: {e}", file=sys.stderr)
            continue
        name = n.split("/")[0]
        offs, off = [], 0
        while off < len(t):
            offs.append(off); off += WIN - OVERLAP
        db.execute("INSERT INTO conv VALUES(?,?,?,?,?)", (cid, name, n, len(t), len(offs)))
        for o in offs:
            cur = db.execute("INSERT INTO chunk(body) VALUES(?)", (t[o:o + WIN],))
            db.execute("INSERT INTO loc VALUES(?,?,?)", (cur.lastrowid, cid, o))
        if cid % 25 == 0:
            db.commit()
            print(f"  [{cid}/{len(convos)}] {name[:60]}", file=sys.stderr)
    db.commit()
    db.execute("INSERT INTO chunk(chunk) VALUES('optimize')")
    db.commit()
    nc, nk = (db.execute(q).fetchone()[0] for q in
              ("SELECT count(*) FROM conv", "SELECT count(*) FROM chunk"))
    print(f"{nc} conversations, {nk} chunks -> {db_path} "
          f"({os.path.getsize(db_path)//1048576} MB)", file=sys.stderr)


def _terms(q):
    return [w.strip('"*') for w in re.findall(r'"[^"]+"|[\w*]+', q)
            if w.upper() not in ("OR", "AND", "NOT", "NEAR") and len(w.strip('"*')) > 2]


def find(db_path, query, limit, per_conv, zip_path=None):
    db = sqlite3.connect(db_path)
    db.row_factory = sqlite3.Row
    z = zipfile.ZipFile(zip_path) if zip_path else None
    cache, terms = {}, _terms(query)

    def snip(path, off, width=260):
        if not z:
            return "(pass the .zip to see snippets)"
        if path not in cache:
            cache[path] = z.read(path).decode("utf-8", "replace")
        t = cache[path][off:off + WIN]
        for w in terms:
            m = re.search(re.escape(w), t, re.I)
            if m:
                a = max(0, m.start() - width // 2)
                return " ".join(t[a:a + width].split())
        return " ".join(t[:width].split())

    try:
        rows = db.execute("""
          SELECT c.name, c.id, c.path, l.off AS offset, bm25(chunk) AS rank
          FROM chunk JOIN loc l ON l.rowid = chunk.rowid
                     JOIN conv c ON c.id = l.conv_id
          WHERE chunk MATCH ? ORDER BY rank LIMIT ?""", (query, limit * 4)).fetchall()
    except sqlite3.OperationalError as e:
        sys.exit(f"bad query: {e}\n(phrases: '\"note graph\"'  prefix: 'zettel*'  "
                 f"proximity: 'NEAR(a b, 20)'  -- NOT 'a NEAR/20 b')")
    seen, shown = {}, 0
    for r in rows:
        if seen.get(r["name"], 0) >= per_conv:
            continue
        seen[r["name"]] = seen.get(r["name"], 0) + 1
        shown += 1
        print(f"\n[{r['id']}] @{r['offset']}  {r['name']}")
        print("    " + snip(r["path"], r["offset"])[:300])
        if shown >= limit:
            break
    if not shown:
        print("no matches")
    else:
        print(f"\n{shown} hits in {len(seen)} conversations. "
              f"`show <id> --at <offset>` to read one.")


def show(zip_path, db_path, cid, at, around, width):
    db = sqlite3.connect(db_path)
    row = db.execute("SELECT name, path FROM conv WHERE id=?", (cid,)).fetchone()
    if not row:
        sys.exit(f"no conversation {cid}")
    name, path = row
    t = zipfile.ZipFile(zip_path).read(path).decode("utf-8", "replace")
    if around:
        m = re.search(re.escape(around), t, re.I)
        if not m:
            sys.exit(f"{around!r} not found in {name}")
        at = max(0, m.start() - width // 2)
    print(f"=== {name}  [{at}..{at+width}] of {len(t)} ===\n")
    print(t[at:at + width])


def listing(db_path, n):
    db = sqlite3.connect(db_path)
    print(f"{'id':>4} {'size':>8}  conversation")
    for r in db.execute("SELECT id,name,bytes FROM conv ORDER BY bytes DESC LIMIT ?", (n,)):
        print(f"{r[0]:>4} {r[2]//1024:>7}K  {r[1][:76]}")


def main():
    ap = argparse.ArgumentParser(prog="chatgrep")
    s = ap.add_subparsers(dest="cmd", required=True)
    b = s.add_parser("index"); b.add_argument("zip"); b.add_argument("--db", default=DB)
    f = s.add_parser("find"); f.add_argument("query"); f.add_argument("zip", nargs="?")
    f.add_argument("--db", default=DB)
    f.add_argument("-n", type=int, default=20); f.add_argument("--per-conv", type=int, default=3)
    w = s.add_parser("show"); w.add_argument("id", type=int); w.add_argument("zip", nargs="?")
    w.add_argument("--db", default=DB); w.add_argument("--at", type=int, default=0)
    w.add_argument("--around"); w.add_argument("--width", type=int, default=4000)
    l = s.add_parser("list"); l.add_argument("--db", default=DB); l.add_argument("-n", type=int, default=40)
    a = ap.parse_args()

    if a.cmd == "index":
        build(a.zip, a.db)
    elif a.cmd == "find":
        find(a.db, a.query, a.n, a.per_conv, a.zip)
    elif a.cmd == "show":
        zp = a.zip or sqlite3.connect(a.db).execute("SELECT 1").fetchone() and None
        if not a.zip:
            sys.exit("show needs the zip path: chatgrep.py show <id> <archive.zip> --around '...'")
        show(a.zip, a.db, a.id, a.at, a.around, a.width)
    else:
        listing(a.db, a.n)


if __name__ == "__main__":
    main()
