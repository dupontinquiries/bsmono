#!/usr/bin/env python3
"""orphan_check.py — find .rs files no `mod` declaration reaches.

WHY THIS EXISTS

`gpu-gemm/src/specloop.rs` sat in the tree for months with no `mod specloop`
anywhere. Not "no caller" — NEVER COMPILED. Its six tests had never run, and
because nothing in the build could contradict it, two documents drifted into
disagreeing about it: `dupscan.rs` recorded the audit decision to delete it and
described that as done, while `ASAHI_GPU.md` listed it as the speculative
control loop and told the reader to wire it up. Both were written in good faith
against a file the compiler had never seen.

An orphan is worse than dead code. Dead code is compiled, type-checked and
tested, so it stays honest. An undeclared file rots silently and still reads
like the real implementation to whoever greps for it next — and in one case
here, a directory of 90 syntax-highlighted `.wgsl` shaders is dead while the
GPU runs string literals in `lib.rs`, 13 of which have already drifted apart.

TWO FALSE-POSITIVE LESSONS, both learned the hard way on the first run:

  1. DO NOT ANCHOR to line start. This tree writes
     `#[allow(dead_code)] pub mod archcfg;` with the attribute on the same
     line. An `^\\s*` anchor missed every one and reported archcfg, memplan and
     moe as orphans when their tests demonstrably run.
  2. DO EXCLUDE COMMENTS. Unanchoring alone made a prose comment reading
     "adding `mod kernels;`" count as a declaration — a comment written about
     that very orphan, which then hid it.

     Stripping string literals as well was tried and REVERTED: pathologically
     slow on a 20k-line lib.rs full of raw-string WGSL, and over-eager enough
     to turn feature-gated modules into false orphans. Prose that needs to
     mention a declaration should avoid writing it in scannable form — which
     is a cheaper fix than teaching a regex to lex Rust.
  3. DO READ `[[bin]]` PATHS from Cargo.toml. `gpu-gemm/src/serve.rs` is a
     binary target, not a module, and reporting it as an orphan is noise.
  4. DO READ `#[path = "..."]` TARGETS, not just the declared mod NAME.
     `unendlich-engine/src/render_meshref_shim.rs` is reached via
     `#[path = "render_meshref_shim.rs"] pub mod render;` — the declared name
     is `render`, which has nothing to do with the file's own stem, so the
     plain `mod x` scan learned "render is declared" and never connected it
     to the file that actually provides it. Fixed by also collecting every
     `#[path]` attribute's target stem as reachable, crate-wide — the same
     by-stem-not-by-directory simplification this tool already makes
     everywhere else, so a `#[path]`'s target being reachable from a
     DIFFERENT file than the one that redirected to it is accepted the same
     way a `mod` declaration's name already is.

Scope: every crate under the given root. A file is reachable if any `mod <stem>`
declaration appears in a non-comment position anywhere in its crate, since a
module declared inside another module is still compiled.
"""
import pathlib, re, sys

# `mod x;` or `mod x {`, anywhere on a line, but not after a // comment marker.
MOD = re.compile(r'\bmod\s+(\w+)\s*[;{]')
LINE_COMMENT = re.compile(r'//.*$')
BLOCK_COMMENT = re.compile(r'/\*.*?\*/', re.S)
BIN_PATH = re.compile(r'^\s*path\s*=\s*["\']([^"\']+)["\']', re.M)
# `#[path = "some/file.rs"]` redirects the NEXT mod declaration to a file whose stem need not match
# the declared name at all — `#[path = "render_meshref_shim.rs"] pub mod render;` makes `render` the
# declared name while the file on disk is named something else entirely. The plain MOD regex above
# only ever learns "render" is declared, never that render_meshref_shim.rs is the file that provides
# it, so that file reported as an orphan (found via unendlich-engine, C14_TODO). This does not need to
# associate a #[path] with a specific mod statement — any file matching a #[path]'s target stem
# ANYWHERE in the crate is reachable through that attribute by definition, same simplification the
# rest of this tool already makes (it compares by file STEM crate-wide, never by directory).
PATH_ATTR = re.compile(r'#\[\s*path\s*=\s*["\']([^"\']+)["\']\s*\]')


def declarations(text: str) -> set:
    """Module names declared in CODE — comments and string literals removed.

    Comments only. Stripping STRING LITERALS too was tried and reverted: on
    gpu-gemm's 20k-line lib.rs, which is full of raw-string WGSL, the regex was
    pathologically slow, and it over-stripped enough to turn feature-gated
    modules into false orphans. The one case it would have caught — a Rust
    string containing the phrase `mod kernels;` — was fixed at the source
    instead, by not writing a scannable declaration inside prose.

    Returns declared NAMES (from `mod x`) union `#[path]` TARGET STEMS — the
    latter because a #[path] redirect can point a mod declaration at a file
    whose stem has nothing to do with the declared name (`render_meshref_shim`
    reached via `#[path = "render_meshref_shim.rs"] mod render;`).
    """
    text = BLOCK_COMMENT.sub("", text)
    text = "\n".join(LINE_COMMENT.sub("", ln) for ln in text.splitlines())
    path_stems = {pathlib.Path(p).stem for p in PATH_ATTR.findall(text)}
    return set(MOD.findall(text)) | path_stems


def crate_dirs(root: pathlib.Path):
    for toml in sorted(root.rglob("Cargo.toml")):
        if any(p in toml.parts for p in ("target", "vendor", ".git", "node_modules")):
            continue
        if (toml.parent / "src").is_dir():
            yield toml.parent


def target_paths(crate: pathlib.Path) -> set:
    """Files Cargo.toml names as targets — binaries, examples, benches.

    These are compiled as their own crate roots, so they are reachable without
    any `mod` declaration. Read rather than guessed: `gpu-gemm` has a
    `[[bin]] path = "src/serve.rs"`.
    """
    toml = crate / "Cargo.toml"
    try:
        return {pathlib.Path(m) for m in BIN_PATH.findall(toml.read_text(errors="replace"))}
    except OSError:
        return set()


def scan(crate: pathlib.Path):
    src = crate / "src"
    files = sorted(src.rglob("*.rs"))
    targets = target_paths(crate)
    declared = set()
    for f in files:
        try:
            declared |= declarations(f.read_text(errors="replace"))
        except OSError:
            pass

    orphans = []
    for f in files:
        stem = f.stem
        # Crate roots and auto-discovered binaries are targets, not modules.
        if stem in ("lib", "main", "mod", "build"):
            continue
        if f.parent.name in ("bin", "examples", "tests", "benches"):
            continue
        if f.relative_to(crate) in targets:
            continue
        if stem in declared:
            continue
        try:
            text = f.read_text(errors="replace")
        except OSError:
            continue
        orphans.append((f.relative_to(crate), text.count("\n") + 1, text.count("#[test]")))
    return orphans



# ── orphaned PUBLIC FUNCTIONS ────────────────────────────────────────────────
#
# The file-level check above catches code the compiler never sees. This catches
# the next layer: code the compiler DOES see, that nothing calls.
#
# Four of these were found by hand in one maintenance session and every one had
# the same shape — the outer crate carrying its own copy of something the inner
# crate already had:
#
#   * `RUNTIME_TOOLS` advertised ten tools while the validator accepted four,
#     and `git_log` had an executor with no parser arm;
#   * two `liaison` modules, two types called `Statement`, one set live;
#   * `taint::extract_nonced`, a half-implemented SECURITY check with no
#     callers — scanning only for XML and blind to the fence form the prompt
#     teaches.
#
# The last is why this is worth automating. An unused function is usually
# harmless; an unused function that looks like the right one is a trap, because
# the next person needing that check finds it first. Each was found by noticing
# a symptom and grepping backwards. This finds them by reading.
#
# DELIBERATELY CONSERVATIVE. It only reports a `pub fn` with ZERO mentions
# anywhere in the workspace outside its own file. That misses plenty (a
# function called once from a test it shares a file with still counts as used),
# and that is the right trade: this competes for attention with real work, and
# a checker people learn to skim is worth less than one that reports five
# things a year and is right about all five.

PUB_FN = re.compile(r'^\s*pub(?:\(crate\))?\s+(?:async\s+)?(?:unsafe\s+)?fn\s+([a-z_][a-z0-9_]*)')

# Names whose only caller is the language or a derive, or which are meant to be
# reached from outside the workspace entirely.
EXEMPT = {
    "main", "new", "default", "fmt", "clone", "drop", "from", "into",
    "next", "poll", "update", "ui", "run", "build", "len", "is_empty",
}


def public_fns(path: pathlib.Path) -> list:
    """(name, line) for each pub fn declared in this file, comments excluded."""
    out = []
    src = BLOCK_COMMENT.sub("", path.read_text(errors="ignore"))
    for i, raw in enumerate(src.split("\n"), 1):
        raw = LINE_COMMENT.sub("", raw)
        m = PUB_FN.match(raw)
        if m and m.group(1) not in EXEMPT:
            out.append((m.group(1), i))
    return out


def orphan_fns(root: pathlib.Path) -> list:
    """pub fns whose name appears nowhere in the workspace but their own file.

    Tokenised ONCE rather than searched per candidate. The obvious
    implementation — for each name, scan every file — is O(names x files) and
    on this tree (thousands of each) does not finish. Counting identifier
    occurrences per file and comparing a name's total against its count in the
    declaring file is one pass over the corpus and answers the same question:
    if every occurrence is in the file that declares it, nothing else uses it.
    """
    import collections
    ident = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")
    # Vendored code is EXCLUDED from reporting but still SEARCHED: a binding
    # crate is meant to expose API nobody here calls yet, so reporting it is
    # pure noise (vendor/opendht alone accounts for most of a 4454-line first
    # run). It still has to be scanned, because a vendored file calling one of
    # our functions is a real use.
    files = [p for p in root.rglob("*.rs")
             if "/target/" not in str(p) and "/.git/" not in str(p)]
    reportable = lambda p: "/vendor/" not in str(p) and not str(p).startswith("vendor/")
    per_file = {}
    total = collections.Counter()
    for p in files:
        c = collections.Counter(ident.findall(p.read_text(errors="ignore")))
        per_file[p] = c
        total.update(c)

    declared = {}
    for p in files:
        for name, line in public_fns(p):
            declared.setdefault(name, []).append((p, line))

    # WHAT TO REPORT — the first cut got this backwards.
    #
    # "Unused public function" is accurate and useless: this tree has 4442 of
    # them, because a monorepo of 290 crates has a lot of library API that only
    # its own module calls yet. A checker reporting 4442 things is a checker
    # nobody reads.
    #
    # The signal is not UNUSED, it is UNUSED **AND DUPLICATED**: a function
    # whose name is also defined somewhere else, where one definition is live
    # and the other is not. That is the shape of every instance found by hand —
    # two `liaison` modules with two `Statement`s, `extract_nonced` beside
    # `authorize_text`, a tool list beside a tool list. The dead twin is
    # dangerous precisely BECAUSE the live one proves the name is the right
    # name to reach for.
    #
    # So the earlier version skipped multi-site names as "ambiguous" and threw
    # away the only thing worth finding.
    out = []
    for name, sites in declared.items():
        if len(sites) < 2:
            continue  # a lone unused fn is ordinary dead code, not a trap
        dead = [(p, l) for (p, l) in sites
                if total[name] == sum(per_file[q][name] for (q, _) in sites) - 0
                and per_file[p][name] == total[name] - sum(
                    per_file[q][name] for (q, _) in sites if q != p)]
        # Simpler and correct: a site is dead if the name appears nowhere
        # outside the files that DECLARE it — i.e. every mention is a
        # definition or an internal use, so no other module reaches any of them
        # by that name.
        decl_files = {p for (p, _) in sites}
        outside = total[name] - sum(per_file[q][name] for q in decl_files)
        if outside > 0:
            continue  # at least one twin is genuinely called from elsewhere
        for (p, line) in sites:
            if reportable(p):
                out.append((p.relative_to(root), line, name))
        _ = dead
    return sorted(out)


def main() -> int:
    args = [a for a in sys.argv[1:] if not a.startswith("-")]
    fns_mode = "--functions" in sys.argv
    root = pathlib.Path(args[0] if args else "/home/claude/repo")
    if fns_mode:
        import collections
        found = orphan_fns(root)
        # GROUPED BY FILE PAIR. Ungrouped, one known duplication drowns the
        # rest: apps/snapclone vs system/snapclone (C13_TODO 17, ~36k lines)
        # accounts for most of the raw count, and printing it 400 times says
        # nothing the first line did not. Grouping turns "718 functions" into
        # "these files shadow each other", which is the actionable shape and
        # the one that would have surfaced the two liaisons as a single item.
        by_name = collections.defaultdict(list)
        for rel, line, name in found:
            by_name[name].append((rel, line))
        pairs = collections.Counter()
        for name, sites in by_name.items():
            fs = tuple(sorted({str(r) for r, _ in sites}))
            if len(fs) > 1:
                pairs[fs] += 1
        print("Duplicated public functions, by the files that shadow each other.")
        print("A name defined in two places where NEITHER is reached from outside "
              "those files: the dead twin is dangerous precisely because the live "
              "one proves the name is right to reach for.\n")
        for fs, n in pairs.most_common(15):
            print(f"  {n:4d} shared name(s)")
            for f in fs:
                print(f"         {f}")
        print(f"\n{len(pairs)} shadowing file-set(s), {len(found)} function site(s).")
        return 1 if pairs else 0
    total_files = total_tests = 0
    crates = 0
    for crate in crate_dirs(root):
        crates += 1
        orphans = scan(crate)
        if not orphans:
            continue
        print(f"\n{crate.relative_to(root)}")
        for rel, lines, tests in sorted(orphans):
            total_files += 1
            total_tests += tests
            print(f"    {str(rel):48s} {lines:6d} lines  {tests:3d} tests")

    print(f"\nscanned {crates} crates")
    print(f"orphaned files: {total_files}, carrying {total_tests} tests that never run")
    return 1 if total_files else 0


if __name__ == "__main__":
    sys.exit(main())
