#!/usr/bin/env python3
"""chunkup -- pack a big asset tree into size-capped upload chunks.

    plan    classify, size, bin-pack          -> plan.json  (+ a table you read)
    pack    write chunk-NNN.tar[.zst]         -> chunks/    (+ MANIFEST.sha256)
    verify  re-hash the chunks                -> pass/fail

Sizing rule: bins are packed against RAW bytes at 98% of the cap, so the
archive is under the cap whether or not compression helps. Model assets are
mostly already-compressed (.glb/.jpg/.png), and measured ratios on this kind
of data range from ~0.7 to ~1.0 -- too variable to bin against.

Classification: directories named as pipeline OUTPUT in games/blackstar/PIPELINE.md
are skipped by default, because reprocessing regenerates them. Override with
--include-derived, or edit DERIVED below.
"""
import argparse, hashlib, json, os, subprocess, sys, tarfile

# Output directories per PIPELINE.md: process_models --out-iqm iqm,
# texture_export --out-iqm parts, part_harvest --out detailcat,
# limb_split/vehicle_rig --out kit, plus the named rib/part catalogs.
# Never packed: caches and editor droppings. .pyc in particular is compiled
# for one interpreter version (cpython-314 here) and is useless anywhere else.
JUNK_DIRS = {"__pycache__", ".git", ".ipynb_checkpoints", "node_modules"}
JUNK_EXTS = (".pyc", ".pyo", ".DS_Store")

DERIVED = {
    "iqm", "parts", "kit",
    "iqm-full-v2", "iqm-full-v2-high", "iqm-full-v2-indie",
    "detailcat", "greeblecat", "vehiclecat", "roboskelcat", "skelcat",
}
# Whole-library archives: redundant with the tree they were made from.
ARCHIVE_SUFFIXES = (".tar.zst", ".tar.gz", ".tar.xz", ".zip")

# Upload these first -- PIPELINE.md step 2 says diagnose donors before
# trusting anything, so a donors-only chunk validates the run cheaply.
PRIORITY = ["donors", "references", "bots", "vehicles"]


def human(n):
    for u in ("B", "K", "M", "G", "T"):
        if n < 1024 or u == "T":
            return f"{n:.1f}{u}" if u != "B" else f"{n}B"
        n /= 1024


def load_patterns(path):
    """One regex per line, '#' comments. Kept as a FILE, not baked in, because
    what counts as excludable is a judgement call that belongs to whoever owns
    the library — and it needs to be reviewable and diffable, not buried in
    a tool's source."""
    import re as _re
    pats = []
    for line in open(path, encoding="utf-8"):
        line = line.split("#", 1)[0].strip()
        if line:
            pats.append(_re.compile(line, _re.I))
    return pats


def excluded(rel, pats):
    return any(p.search(rel) for p in pats)


def parse_size(s):
    s = str(s).strip().upper()
    mult = {"K": 1024, "M": 1024**2, "G": 1024**3}
    if s and s[-1] in mult:
        return int(float(s[:-1]) * mult[s[-1]])
    if s.endswith("B"):
        s = s[:-1]
    return int(float(s))


# ------------------------------------------------------------------ plan

def units(root, include_derived=False, follow=False):
    """Yield (relpath, bytes, nfiles) for each atomic unit.

    A unit is a top-level model folder -- the thing the pipeline treats as one
    model. Keeping it whole means a chunk can be processed on its own; only a
    unit larger than the cap gets split, and that is reported.
    """
    root = os.path.abspath(root)
    for name in sorted(os.listdir(root)):
        full = os.path.join(root, name)
        if name in JUNK_DIRS or name.endswith(JUNK_EXTS):
            b, n = dirsize(full) if os.path.isdir(full) else (os.path.getsize(full), 1)
            yield ("SKIP-junk", name, b, n); continue
        if not include_derived:
            if name in DERIVED:
                # Size it. Reporting 0 made the classifier look like it saved
                # 600 MB when it actually saved 3 GB, which is the number that
                # decides whether the split is worth doing at all.
                b, n = dirsize(full) if os.path.isdir(full) else (os.path.getsize(full), 1)
                yield ("SKIP-derived", name, b, n); continue
            if name.endswith(ARCHIVE_SUFFIXES):
                sz = os.path.getsize(full) if os.path.isfile(full) else 0
                yield ("SKIP-archive", name, sz, 1); continue
        if os.path.isfile(full):
            yield ("unit", name, os.path.getsize(full), 1); continue
        if not os.path.isdir(full) or (os.path.islink(full) and not follow):
            continue
        # one unit per second-level entry, so "bots" becomes ~50 model folders
        # rather than one 800 MB blob
        if name in JUNK_DIRS:
            yield ("SKIP-junk", name, *dirsize(full)); continue
        subs = sorted(os.listdir(full))
        if not subs:
            continue
        for sub in subs:
            sfull = os.path.join(full, sub)
            rel = f"{name}/{sub}"
            b, n = dirsize(sfull) if os.path.isdir(sfull) else (os.path.getsize(sfull), 1)
            if b:
                yield ("unit", rel, b, n)


def dirsize(path):
    total = files = 0
    for dp, _dn, fn in os.walk(path):
        for f in fn:
            try:
                total += os.lstat(os.path.join(dp, f)).st_size
                files += 1
            except OSError:
                pass
    return total, files


def split_unit(root, rel, cap):
    """Break an oversize unit into per-file pieces that each fit."""
    base = os.path.join(root, rel)
    out, cur, cursz = [], [], 0
    for dp, dn, fn in os.walk(base):
        dn[:] = [d for d in dn if d not in JUNK_DIRS]
        for f in sorted(fn):
            if f.endswith(JUNK_EXTS):
                continue
            p = os.path.join(dp, f)
            try:
                sz = os.lstat(p).st_size
            except OSError:
                continue
            r = os.path.relpath(p, root)
            if sz > cap:
                out.append(([r], sz)); continue
            if cursz + sz > cap:
                out.append((cur, cursz)); cur, cursz = [], 0
            cur.append(r); cursz += sz
    if cur:
        out.append((cur, cursz))
    return out


def plan(root, cap, include_derived, margin=0.98, pats=()):
    target = int(cap * margin)
    keep, skipped, oversize = [], [], []
    for kind, rel, b, n in units(root, include_derived):
        if kind.startswith("SKIP"):
            skipped.append((kind, rel, b)); continue
        if pats and excluded(rel, pats):
            skipped.append(("SKIP-excluded", rel, b)); continue
        (oversize if b > target else keep).append((rel, b, n))

    items = []                      # (members|None, rel, bytes, nfiles)
    for rel, b, n in keep:
        items.append(([rel], rel, b, n))
    for rel, b, n in oversize:
        for members, sz in split_unit(root, rel, target):
            items.append((members, rel + " (split)", sz, len(members)))

    # first-fit-decreasing: good packing, and stable across runs
    items.sort(key=lambda x: -x[2])
    prio = {p: i for i, p in enumerate(PRIORITY)}
    bins = []
    for members, rel, b, n in items:
        for bn in bins:
            if bn["bytes"] + b <= target:
                bn["members"] += members; bn["bytes"] += b
                bn["files"] += n; bn["units"].append(rel)
                break
        else:
            bins.append({"members": list(members), "bytes": b,
                         "files": n, "units": [rel]})
    # emit priority content first so an interrupted upload is still useful
    bins.sort(key=lambda bn: min(
        (prio.get(u.split("/")[0], 99) for u in bn["units"]), default=99))
    for i, bn in enumerate(bins, 1):
        bn["id"] = i
    return {"root": os.path.abspath(root), "cap": cap, "target": target,
            "include_derived": include_derived,
            "skipped": skipped, "oversize": [o[0] for o in oversize],
            "chunks": bins}


def show(p):
    tot = sum(b["bytes"] for b in p["chunks"])
    skip = sum(s[2] for s in p["skipped"])
    print(f"root {p['root']}")
    print(f"cap  {human(p['cap'])}  (packing to {human(p['target'])} raw)\n")
    if p["skipped"]:
        print("skipped:")
        for kind, rel, b in p["skipped"]:
            print(f"   {kind:<14} {human(b):>8}  {rel}")
        print()
    if p["oversize"]:
        print(f"units larger than one chunk, split across chunks: "
              f"{', '.join(p['oversize'])}")
        print("   (extract every chunk into one directory before processing)\n")
    print(f"{'chunk':>6} {'size':>9} {'files':>7}  contents")
    for b in p["chunks"]:
        u = ", ".join(b["units"][:3]) + (f" +{len(b['units'])-3} more"
                                         if len(b["units"]) > 3 else "")
        print(f"{b['id']:>6} {human(b['bytes']):>9} {b['files']:>7}  {u[:88]}")
    print(f"\n{len(p['chunks'])} chunks, {human(tot)} to upload"
          f"{f' ({human(skip)} skipped)' if skip else ''}")


# ------------------------------------------------------------------ pack

def pack(p, outdir, compress):
    os.makedirs(outdir, exist_ok=True)
    root = p["root"]
    man, bad = [], []
    for b in p["chunks"]:
        stem = f"chunk-{b['id']:03d}.tar"
        path = os.path.join(outdir, stem)
        if os.path.exists(path) or os.path.exists(path + ".zst"):
            print(f"  chunk-{b['id']:03d}: exists, skipping", file=sys.stderr)
            continue
        with tarfile.open(path, "w") as tf:
            for m in b["members"]:
                tf.add(os.path.join(root, m), arcname=m, recursive=True)
        if compress != "none":
            path = compress_file(path, compress)
        sz = os.path.getsize(path)
        h = sha256(path)
        man.append((os.path.basename(path), sz, h))
        flag = "  !! OVER CAP" if sz > p["cap"] else ""
        print(f"  {os.path.basename(path):<22} {human(sz):>9}  {h[:16]}{flag}",
              file=sys.stderr)
        if sz > p["cap"]:
            bad.append(os.path.basename(path))
    with open(os.path.join(outdir, "MANIFEST.sha256"), "a") as f:
        for name, sz, h in man:
            f.write(f"{h}  {name}  {sz}\n")
    if bad:
        print(f"\nFAILED: {len(bad)} chunk(s) over the cap: {bad}", file=sys.stderr)
        return 1
    return 0


def compress_file(path, how):
    tools = {"zstd": ["zstd", "-19", "-T0", "-q", "--rm", path],
             "gzip": ["gzip", "-6", path], "xz": ["xz", "-6", "-T0", path]}
    ext = {"zstd": ".zst", "gzip": ".gz", "xz": ".xz"}[how]
    r = subprocess.run(tools[how], capture_output=True, text=True)
    if r.returncode != 0 or not os.path.exists(path + ext):
        print(f"  compression with {how} failed, leaving .tar: "
              f"{r.stderr.strip()[:120]}", file=sys.stderr)
        return path
    return path + ext


def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for blk in iter(lambda: f.read(1 << 20), b""):
            h.update(blk)
    return h.hexdigest()


def verify(outdir):
    mf = os.path.join(outdir, "MANIFEST.sha256")
    if not os.path.exists(mf):
        print("no MANIFEST.sha256", file=sys.stderr); return 1
    bad = 0
    for line in open(mf):
        h, name, sz = line.split()
        p = os.path.join(outdir, name)
        if not os.path.exists(p):
            print(f"MISSING  {name}"); bad += 1; continue
        got = sha256(p)
        ok = got == h and os.path.getsize(p) == int(sz)
        print(f"{'ok      ' if ok else 'MISMATCH'} {name}")
        bad += not ok
    return 1 if bad else 0


# ------------------------------------------------------------------ cli

def main():
    ap = argparse.ArgumentParser(prog="chunkup")
    s = ap.add_subparsers(dest="cmd", required=True)
    pl = s.add_parser("plan"); pl.add_argument("root")
    pl.add_argument("--limit", default="450M"); pl.add_argument("-o", default="plan.json")
    pl.add_argument("--include-derived", action="store_true")
    pl.add_argument("--exclude-file", metavar="PATTERNS",
                    help="file of regexes; matching units are left out entirely")
    sc = s.add_parser("scan", help="report which units match a pattern file, change nothing")
    sc.add_argument("root"); sc.add_argument("--exclude-file", required=True)
    pk = s.add_parser("pack"); pk.add_argument("--plan", default="plan.json")
    pk.add_argument("-o", default="chunks")
    pk.add_argument("--compress", choices=["none", "zstd", "gzip", "xz"], default="zstd")
    vf = s.add_parser("verify"); vf.add_argument("-o", default="chunks")
    a = ap.parse_args()

    if a.cmd == "scan":
        pats = load_patterns(a.exclude_file)
        hit = [(rel, b) for kind, rel, b, _n in units(a.root, True)
               if not kind.startswith("SKIP") and excluded(rel, pats)]
        allu = [(rel, b) for kind, rel, b, _n in units(a.root, True)
                if not kind.startswith("SKIP")]
        print(f"{len(hit)} of {len(allu)} units match ({human(sum(b for _r, b in hit))} "
              f"of {human(sum(b for _r, b in allu))})\n")
        for rel, b in sorted(hit, key=lambda x: -x[1]):
            print(f"  {human(b):>9}  {rel}")
        return

    if a.cmd == "plan":
        pats = load_patterns(a.exclude_file) if a.exclude_file else ()
        p = plan(a.root, parse_size(a.limit), a.include_derived, pats=pats)
        json.dump(p, open(a.o, "w"), indent=1)
        show(p)
        print(f"\nplan -> {a.o}")
    elif a.cmd == "pack":
        sys.exit(pack(json.load(open(a.plan)), a.o, a.compress))
    elif a.cmd == "verify":
        sys.exit(verify(a.o))


if __name__ == "__main__":
    main()
