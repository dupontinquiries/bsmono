#!/usr/bin/env python3
"""gitsweep -- inventory every git repo under a root and extract its history.

Three stages, deliberately separate so you can re-run the cheap ones:

    discover   walk the tree, find every repo, classify it        -> repos.ndjson
    extract    one read-only `git log` per repo, with deltas      -> commits.ndjson
    index      load both into SQLite and cluster into lineages    -> history.db

Nothing here writes to a repo. Every git call is plumbing or log with an
explicit --git-dir; there is no checkout, no gc, no fsck, no index refresh.
System and user gitconfig are suppressed so a stray alias or hook cannot
change what gets run.
"""
import argparse, hashlib, json, os, subprocess, sys, time

ENV = {
    **os.environ,
    "GIT_CONFIG_NOSYSTEM": "1",
    "GIT_CONFIG_GLOBAL": "/dev/null",
    "GIT_TERMINAL_PROMPT": "0",
    "GIT_OPTIONAL_LOCKS": "0",   # do not take index.lock
    "GIT_PAGER": "cat",
    "LC_ALL": "C",
}

RS, US = "\x1e", "\x1f"


def git(gitdir, *args, timeout=300):
    """Run a git command against gitdir. Returns (rc, stdout, stderr)."""
    cmd = ["git", "--git-dir", gitdir, *args]
    try:
        p = subprocess.run(cmd, capture_output=True, text=True, errors="replace",
                           env=ENV, timeout=timeout)
        return p.returncode, p.stdout, p.stderr
    except subprocess.TimeoutExpired:
        return 124, "", f"timeout after {timeout}s"
    except OSError as e:
        return 127, "", str(e)


# ---------------------------------------------------------------- discover

def resolve_gitdir(entry):
    """entry is a path named .git (dir or file) or a bare repo dir."""
    if os.path.isdir(entry):
        return entry
    # worktree / submodule: .git is a file containing "gitdir: <path>"
    try:
        with open(entry, "r", errors="replace") as f:
            line = f.readline().strip()
    except OSError:
        return None
    if line.startswith("gitdir:"):
        target = line.split(":", 1)[1].strip()
        if not os.path.isabs(target):
            target = os.path.normpath(os.path.join(os.path.dirname(entry), target))
        return target
    return None


def looks_bare(d):
    return all(os.path.exists(os.path.join(d, x)) for x in ("HEAD", "objects", "refs"))


def discover(root, follow_symlinks=False):
    """Yield candidate gitdirs. Prunes so we never descend into repo internals."""
    seen = set()
    for dirpath, dirnames, filenames in os.walk(root, followlinks=follow_symlinks):
        hit = None
        if ".git" in dirnames or ".git" in filenames:
            hit = os.path.join(dirpath, ".git")
        elif dirpath.endswith(".git") and looks_bare(dirpath):
            hit = dirpath
        if hit:
            gd = resolve_gitdir(hit)
            if gd:
                real = os.path.realpath(gd)
                if real not in seen:
                    seen.add(real)
                    yield {"worktree": dirpath, "gitdir_link": hit, "gitdir": real}
            # never walk into a repo's own object store
            dirnames[:] = [d for d in dirnames if d != ".git"]
            if hit == dirpath:
                dirnames[:] = []


def probe(rec):
    """Cheap per-repo facts. Never touches the working tree."""
    gd = rec["gitdir"]
    out = dict(rec, error=None)

    rc, s, e = git(gd, "rev-parse", "--is-bare-repository", timeout=30)
    if rc != 0:
        out["error"] = (e or "not a repository").strip()[:300]
        return out
    out["bare"] = s.strip() == "true"

    rc, s, _ = git(gd, "symbolic-ref", "--quiet", "HEAD", timeout=30)
    out["head_ref"] = s.strip() or None

    # A linked worktree has its own gitdir but shares the parent's object store,
    # so its history is not new data. Record the shared dir and let index skip it.
    rc, s, _ = git(gd, "rev-parse", "--git-common-dir", timeout=30)
    common = s.strip()
    if common and common != ".":
        common = os.path.realpath(
            common if os.path.isabs(common) else os.path.join(gd, common))
    else:
        common = gd
    out["common_dir"] = common
    out["is_linked_worktree"] = common != gd

    rc, s, _ = git(gd, "for-each-ref", "--format=%(objectname) %(refname)", timeout=120)
    refs = [l.split(" ", 1) for l in s.splitlines() if " " in l]
    out["n_refs"] = len(refs)
    out["refs"] = {r: o for o, r in refs}
    # digest of the ref graph: identical digest == byte-identical repo state
    out["refs_digest"] = hashlib.sha1(
        "\n".join(sorted(f"{o} {r}" for o, r in refs)).encode()).hexdigest()

    rc, s, _ = git(gd, "rev-list", "--all", "--count", timeout=300)
    out["n_commits"] = int(s.strip() or 0) if rc == 0 else None

    rc, s, _ = git(gd, "rev-list", "--max-parents=0", "--all", timeout=300)
    out["root_commits"] = s.split() if rc == 0 else []

    rc, s, _ = git(gd, "config", "--get-regexp", r"^remote\..*\.url", timeout=30)
    out["remotes"] = dict(
        (l.split(None, 1) + [""])[:2] for l in s.splitlines()) if rc == 0 else {}

    out["gitdir_bytes"] = du(gd)
    return out


def du(path):
    total = 0
    for dp, _dn, fn in os.walk(path):
        for f in fn:
            try:
                total += os.lstat(os.path.join(dp, f)).st_size
            except OSError:
                pass
    return total


# ---------------------------------------------------------------- extract

# One pass gets metadata + per-file deltas. %x1e starts a record, %x1f splits
# fields; neither can appear in a subject line, unlike anything printable.
PRETTY = (f"{RS}%H{US}%P{US}%aI{US}%cI{US}%an{US}%ae{US}%D{US}%s")


def extract(repo, want_reflog=True, timeout=1800):
    gd = repo["gitdir"]
    args = ["log", "--all", "--no-renames", "--numstat", "--date=iso-strict",
            f"--pretty=format:{PRETTY}"]
    if want_reflog:
        # picks up commits that are only reachable from reflog -- often exactly
        # where the early messy work went after a reset or a rebase
        args.insert(1, "--reflog")
    rc, s, e = git(gd, *args, timeout=timeout)
    if rc != 0:
        yield {"kind": "error", "repo": gd, "stage": "log", "msg": e.strip()[:500]}
        return
    for chunk in s.split(RS):
        if not chunk.strip():
            continue
        head, _, body = chunk.partition("\n")
        f = head.split(US)
        if len(f) < 8:
            continue
        sha, parents, aiso, ciso, an, ae, decor, subject = f[:8]
        files = ins = dels = binf = 0
        paths = []
        for line in body.splitlines():
            if not line.strip():
                continue
            parts = line.split("\t")
            if len(parts) != 3:
                continue
            a, d, p = parts
            files += 1
            if a == "-" or d == "-":
                binf += 1
            else:
                ins += int(a or 0)
                dels += int(d or 0)
            if len(paths) < 40:
                paths.append(p)
        yield {
            "kind": "commit", "repo": gd, "sha": sha,
            "parents": parents.split() if parents else [],
            "adate": aiso, "cdate": ciso, "author": an, "email": ae,
            "decor": decor, "subject": subject,
            "files": files, "ins": ins, "del": dels, "bin": binf,
            "paths": paths,
            # rewrite-stable identity: filter-repo rewrites committer and sha
            # but preserves author name/email/date, so this survives the purge
            "fp": hashlib.sha1(f"{ae}\x00{aiso}\x00{subject}".encode()).hexdigest()[:16],
        }


# ---------------------------------------------------------------- index

SCHEMA = """
CREATE TABLE IF NOT EXISTS repo(
  gitdir TEXT PRIMARY KEY, worktree TEXT, bare INT, head_ref TEXT,
  n_refs INT, n_commits INT, refs_digest TEXT, gitdir_bytes INT,
  root_commits TEXT, remotes TEXT, error TEXT,
  common_dir TEXT, linked INT);
CREATE TABLE IF NOT EXISTS commits(
  repo TEXT, sha TEXT, parents TEXT, adate TEXT, cdate TEXT,
  author TEXT, email TEXT, decor TEXT, subject TEXT,
  files INT, ins INT, dels INT, bin INT, paths TEXT, fp TEXT,
  PRIMARY KEY(repo, sha));
CREATE INDEX IF NOT EXISTS i_fp   ON commits(fp);
CREATE INDEX IF NOT EXISTS i_date ON commits(adate);
CREATE INDEX IF NOT EXISTS i_repo ON commits(repo);
"""


def index(db_path, repos_nd, commits_nd):
    import sqlite3
    db = sqlite3.connect(db_path)
    db.executescript(SCHEMA)
    with open(repos_nd) as f:
        for line in f:
            r = json.loads(line)
            db.execute("INSERT OR REPLACE INTO repo VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?)", (
                r["gitdir"], r.get("worktree"), int(bool(r.get("bare"))),
                r.get("head_ref"), r.get("n_refs"), r.get("n_commits"),
                r.get("refs_digest"), r.get("gitdir_bytes"),
                json.dumps(r.get("root_commits", [])),
                json.dumps(r.get("remotes", {})), r.get("error"),
                r.get("common_dir"), int(bool(r.get("is_linked_worktree")))))
    n = 0
    with open(commits_nd) as f:
        for line in f:
            c = json.loads(line)
            if c.get("kind") != "commit":
                continue
            db.execute("INSERT OR REPLACE INTO commits VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)", (
                c["repo"], c["sha"], " ".join(c["parents"]), c["adate"], c["cdate"],
                c["author"], c["email"], c["decor"], c["subject"],
                c["files"], c["ins"], c["del"], c["bin"],
                json.dumps(c["paths"]), c["fp"]))
            n += 1
            if n % 20000 == 0:
                db.commit()
    db.commit()
    return db


# ---------------------------------------------------------------- report

def report(db_path, which, limit=40):
    import sqlite3
    db = sqlite3.connect(db_path)
    db.row_factory = sqlite3.Row
    q = db.execute

    if which == "dupes":
        # identical ref graph == byte-identical repo state == safe to collapse
        print("repos sharing an identical ref graph (pick one, delete the rest):\n")
        for g in q("""SELECT refs_digest, count(*) n, sum(gitdir_bytes) b
                      FROM repo WHERE error IS NULL AND linked=0 AND n_refs>0
                      GROUP BY refs_digest HAVING n>1 ORDER BY b DESC"""):
            waste = g["b"] - g["b"] // g["n"]
            print(f"  {g['n']} copies, {g['b']//1048576} MB total, "
                  f"~{waste//1048576} MB reclaimable:")
            for r in q("""SELECT gitdir FROM repo
                          WHERE refs_digest=? AND linked=0 AND error IS NULL""",
                       (g["refs_digest"],)):
                print(f"      {r['gitdir']}")

    elif which == "lineage":
        # cluster repos by how much commit-identity they share
        print("repo overlap by fingerprint (shared / smaller side):\n")
        rows = q("""SELECT repo, count(DISTINCT fp) n FROM commits
                    GROUP BY repo HAVING n>0 ORDER BY n DESC LIMIT ?""", (limit,))
        repos = [(r["repo"], r["n"]) for r in rows]
        for i, (a, na) in enumerate(repos):
            for b, nb in repos[i + 1:]:
                sh = q("""SELECT count(*) c FROM (SELECT fp FROM commits WHERE repo=?
                          INTERSECT SELECT fp FROM commits WHERE repo=?)""",
                       (a, b)).fetchone()["c"]
                if sh and sh / min(na, nb) > 0.5:
                    print(f"  {sh:>6} shared  {sh/min(na,nb)*100:5.1f}%  "
                          f"{a}\n{'':>24}{b}")

    elif which == "first":
        # the point of the whole exercise: earliest witness of every commit,
        # so the messy beginning gets assembled from wherever it survived
        print("earliest appearance of each distinct commit, oldest first:\n")
        for r in q("""SELECT fp, min(adate) d, subject, author,
                             count(DISTINCT repo) nrepo,
                             sum(ins) ins, sum(dels) dels
                      FROM commits GROUP BY fp ORDER BY d ASC LIMIT ?""", (limit,)):
            print(f"  {r['d'][:19]}  seen in {r['nrepo']:>2} repo(s)  "
                  f"+{r['ins']}/-{r['dels']:<6} {r['subject'][:70]}")

    elif which == "orphans":
        # commits living in exactly one repo are the ones a delete would destroy
        print("commits present in exactly ONE repo (deleting it loses them):\n")
        for r in q("""SELECT repo, count(*) n, min(adate) lo, max(adate) hi
                      FROM commits WHERE fp IN
                        (SELECT fp FROM commits GROUP BY fp HAVING count(DISTINCT repo)=1)
                      GROUP BY repo ORDER BY n DESC LIMIT ?""", (limit,)):
            print(f"  {r['n']:>6} unique  {r['lo'][:10]} .. {r['hi'][:10]}  {r['repo']}")

    elif which == "unreachable":
        # reflog-only commits: work that was reset/rebased away and never landed
        print("commits reachable only from reflog (no ref decoration):\n")
        for r in q("""SELECT repo, adate, subject, ins, dels FROM commits
                      WHERE decor='' AND sha NOT IN
                        (SELECT DISTINCT p.value FROM commits c,
                         json_each('["'||replace(c.parents,' ','","')||'"]') p
                         WHERE c.parents<>'')
                      ORDER BY adate LIMIT ?""", (limit,)):
            print(f"  {r['adate'][:19]}  +{r['ins']}/-{r['dels']:<6} "
                  f"{r['subject'][:60]}\n{'':>24}{r['repo']}")


# ---------------------------------------------------------------- cli

def main():
    ap = argparse.ArgumentParser(prog="gitsweep")
    sub = ap.add_subparsers(dest="cmd", required=True)

    d = sub.add_parser("discover"); d.add_argument("root"); d.add_argument("-o", default="repos.ndjson")
    x = sub.add_parser("extract");  x.add_argument("-i", default="repos.ndjson")
    x.add_argument("-o", default="commits.ndjson"); x.add_argument("--no-reflog", action="store_true")
    x.add_argument("--timeout", type=int, default=1800)
    i = sub.add_parser("index")
    i.add_argument("--repos", default="repos.ndjson"); i.add_argument("--commits", default="commits.ndjson")
    i.add_argument("-o", default="history.db")
    p = sub.add_parser("report")
    p.add_argument("which", choices=["dupes", "lineage", "first", "orphans", "unreachable"])
    p.add_argument("--db", default="history.db"); p.add_argument("-n", type=int, default=40)
    a = ap.parse_args()

    if a.cmd == "discover":
        n = 0
        with open(a.o, "w") as out:
            for rec in discover(a.root):
                r = probe(rec)
                out.write(json.dumps(r) + "\n"); out.flush()
                n += 1
                print(f"[{n:>4}] {'ERR ' if r.get('error') else ''}"
                      f"{r.get('n_commits','?'):>7} commits  "
                      f"{(r.get('gitdir_bytes') or 0)//1048576:>6} MB  {r['gitdir']}",
                      file=sys.stderr)
        print(f"discovered {n} repos -> {a.o}", file=sys.stderr)

    elif a.cmd == "extract":
        repos = [json.loads(l) for l in open(a.i)]
        skipped = sum(1 for r in repos if r.get("is_linked_worktree"))
        repos = [r for r in repos
                 if not r.get("error") and not r.get("is_linked_worktree")]
        if skipped:
            print(f"skipping {skipped} linked worktree(s) -- they share a parent "
                  f"object store and add no history", file=sys.stderr)
        t0 = time.time()
        with open(a.o, "w") as out:
            for k, r in enumerate(repos, 1):
                n = 0
                for rec in extract(r, want_reflog=not a.no_reflog, timeout=a.timeout):
                    out.write(json.dumps(rec) + "\n"); n += 1
                out.flush()
                print(f"[{k}/{len(repos)}] {n:>7} commits  {time.time()-t0:7.1f}s  {r['gitdir']}",
                      file=sys.stderr)

    elif a.cmd == "index":
        db = index(a.o, a.repos, a.commits)
        nr, nc = (db.execute(q).fetchone()[0] for q in
                  ("SELECT count(*) FROM repo", "SELECT count(*) FROM commits"))
        print(f"{nr} repos, {nc} commits -> {a.o}", file=sys.stderr)

    elif a.cmd == "report":
        report(a.db, a.which, a.n)


if __name__ == "__main__":
    main()
