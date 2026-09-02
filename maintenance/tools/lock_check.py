#!/usr/bin/env python3
"""lock_check.py — find committed Cargo.lock files that predate a dependency
in their own Cargo.toml.

WHY THIS EXISTS

Three of these turned up by accident in one session:

  * shared/voice-kit  — the lock predated the crypto-kit dev-dependency, so
    `cargo test --offline` failed on a clean clone, against TODO.md's claim
    that every kit builds offline. Two parallel sessions hit it separately and
    produced the same regenerated blob.
  * shared/scene-kit  — same class, found by the c8 parallel session.
  * games/blackstar-rs — ruzstd, declared for the v4 pack decoder, absent from
    the lock. Surfaced only because a test run left the tree dirty.

Each was found by tripping over it. Accident is a bad detector, and the failure
is quiet: a stale lock works fine online, because cargo just resolves the
missing name and rewrites the file. It only bites offline, or on a clean clone,
or in CI — which is to say, somewhere inconvenient.

WHY A TEXT SCAN AND NOT `cargo metadata`

Resolving 116 crates on one core takes minutes and wants the network. This
needs neither. It answers exactly one question — is this declared name present
in the lock at all — and cannot see version-range drift. That is the failure
mode that actually occurred three times.

FALSE POSITIVES IT HAD TO LEARN

The first version reported four hits; three were wrong, and each wrong one is
a rule now:

  * RENAMED DEPS. `gtk = { version = "0.8", package = "gtk4" }` is locked as
    "gtk4", not "gtk". Resolve `package =` before comparing (bot-arena).
  * WORKSPACE MEMBERS. A member crate with its own vestigial Cargo.lock is not
    the lock cargo uses; the workspace root's is. Skip a lock whose crate is
    listed in an ancestor's [workspace] members (studio-collab,
    unendlich-engine).
  * TARGET-SPECIFIC AND OPTIONAL DEPS are still real and still checked — being
    behind a feature does not exempt a name from the lock.
"""
import re, sys, pathlib

# .resolve() is load-bearing, not tidiness: is_workspace_member() compares a
# crate's ancestors against ROOT, and with a relative ROOT ("." — the way
# CLAUDE.md §7 and C11_TODO both spell the invocation) the very first ancestor
# fails the containment test, so NOTHING is ever recognised as a workspace
# member and every member crate's vestigial lock is reported stale. That is
# where C11-2a's "unendlich-engine has not locked audiopus, egui, ..." came
# from; it is a member of games/unendlich's workspace and its own lock is not
# the one cargo uses.
ROOT = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else "/home/claude/repo").resolve()
SKIP_PARTS = {"target", "vendor", ".git", "node_modules"}

SECTION = re.compile(r'^\s*\[(?:target\.[^\]]+\.)?(dependencies|dev-dependencies|build-dependencies)\]\s*$')
ANY_SECTION = re.compile(r'^\s*\[')
DEP_LINE = re.compile(r'^\s*["\']?([A-Za-z0-9_.-]+)["\']?\s*=\s*(.*)$')
OPTIONAL = re.compile(r'\boptional\s*=\s*true\b')
PACKAGE_RENAME = re.compile(r'\bpackage\s*=\s*["\']([^"\']+)["\']')
LOCK_NAME = re.compile(r'^name = "([^"]+)"$', re.M)
WS_MEMBERS = re.compile(r'^\s*\[workspace\]', re.M)


def declared(text, with_dev=True):
    """Dependency names as the LOCK will spell them (i.e. after `package =`).

    `with_dev=False` for a crate reached THROUGH a path dependency: cargo
    resolves dev-dependencies for the crate being built, not for its path
    dependencies, so a path dep's dev-deps are not in the dependent's lock and
    demanding them would be a false positive on every crate with a test-only
    dependency anywhere in its graph.
    """
    out, cur = set(), None
    for line in text.splitlines():
        m = SECTION.match(line)
        if m:
            cur = m.group(1); continue
        if ANY_SECTION.match(line):
            cur = None; continue
        if not cur or (cur == "dev-dependencies" and not with_dev):
            continue
        d = DEP_LINE.match(line)
        if not d:
            continue
        name, rhs = d.group(1), d.group(2)
        if name in {"version", "features", "path", "optional", "git", "branch",
                    "rev", "tag", "workspace", "default-features", "registry", "package"}:
            continue
        # `serde.workspace = true` is the dotted spelling of workspace
        # inheritance; the lock spells the crate `serde`. Without this the
        # checker demanded a crate literally named "serde.workspace".
        if name.endswith(".workspace"):
            name = name[: -len(".workspace")]
        # OPTIONAL dependencies past the first hop are skipped. A dependent can
        # turn a path dep's default features OFF — bot-arena-egui takes
        # `bot-arena = { path = "..", default-features = false }` precisely to
        # get the engine without GTK — and cargo then leaves those optional
        # crates out of the lock entirely. Deciding which are active needs real
        # feature resolution, which a line scanner cannot do, so the checker
        # UNDER-reports here rather than crying wolf. Verified the right way
        # round: `cargo generate-lockfile` was run on every crate this flagged,
        # and for these it regenerated the lock WITHOUT them — cargo's own
        # verdict that they do not belong.
        if not with_dev and OPTIONAL.search(rhs):
            continue
        rename = PACKAGE_RENAME.search(rhs)
        out.add(rename.group(1) if rename else name)
    return out


PATH_DEP = re.compile(r'\bpath\s*=\s*["\']([^"\']+)["\']')


def path_deps(text, crate_dir, with_dev=True):
    """Directories of this manifest's `path = "..."` dependencies.

    Only inside a dependency section, so a `[package]` or `[[bin]]` `path` is
    not mistaken for one.
    """
    out, cur = [], None
    for line in text.splitlines():
        m = SECTION.match(line)
        if m:
            cur = m.group(1); continue
        if ANY_SECTION.match(line):
            cur = None; continue
        if not cur or (cur == "dev-dependencies" and not with_dev):
            continue
        d = DEP_LINE.match(line)
        if not d:
            continue
        if d.group(1) in {"version", "features", "path", "optional", "git", "branch",
                          "rev", "tag", "workspace", "default-features", "registry", "package"}:
            continue
        # Same rule as `declared`: do not FOLLOW an optional path dep past the
        # first hop either. Skipping only its name while still walking into it
        # collects everything IT depends on, which is how `worldhost-unendlich`
        # was asked for `egui` — reached through
        # `unendlich-ui = { path = "...", optional = true }`, a crate it never
        # builds.
        if not with_dev and OPTIONAL.search(d.group(2)):
            continue
        pm = PATH_DEP.search(d.group(2))
        if pm:
            out.append((crate_dir / pm.group(1)).resolve())
    return out


def declared_transitive(crate_dir):
    """Every crate name that must appear in this crate's lock.

    NOT just the crate's own [dependencies]. Cargo's lock covers the whole
    resolved graph, so when a PATH dependency gains a dependency, this crate's
    committed lock goes stale — and the crate's own manifest does not change,
    so a check that reads one manifest cannot see it.

    That is not hypothetical: it happened twice in one session. `uikit-egui`
    gained `math-kit`, and both `uikit-term`'s and `voiceterm-egui`'s locks were
    rewritten by cargo on the first build while this script reported
    "stale: 0" on the very same tree. Every crate in the fleet that reaches
    uikit-egui through a path dep had the same latent staleness, and it would
    have kept surfacing one lockfile at a time, on whichever crate someone
    happened to build next.

    Registry dependencies are NOT followed: their transitive names are in the
    lock too, but reading them needs the registry, and a stale lock caused by an
    upstream crate's new dependency is a `cargo update` question rather than a
    committed-file question. Path deps are the ones this repo can go stale
    against by editing itself.

    Cycles are possible in principle (dev-dependencies can be circular), hence
    the visited set.
    """
    seen, names, queue = set(), set(), [(crate_dir, True)]
    while queue:
        d, with_dev = queue.pop()
        if d in seen:
            continue
        seen.add(d)
        toml = d / "Cargo.toml"
        try:
            text = toml.read_text(errors="replace")
        except OSError:
            # A path dep pointing at nothing is a real problem, but it is
            # cargo's to report — this check is about locks, and inventing a
            # second error here would bury the one that matters.
            continue
        names |= declared(text, with_dev)
        # Everything past the first hop is a path DEPENDENCY, so its own
        # dev-dependencies are out of scope from here down.
        queue.extend((p, False) for p in path_deps(text, d, with_dev))
    return names


def is_workspace_member(crate_dir):
    """True when an ancestor Cargo.toml declares a [workspace] — that root's
    lock is the one cargo uses, and this crate's own lock is vestigial."""
    for anc in list(crate_dir.parents):
        # Containment by path parts, not string prefix: "/x/repo2" starts with
        # "/x/repo" and is not inside it.
        try:
            anc.relative_to(ROOT)
        except ValueError:
            break
        t = anc / "Cargo.toml"
        if t.exists():
            try:
                if WS_MEMBERS.search(t.read_text(errors="replace")):
                    return True
            except OSError:
                pass
    return False


def main():
    locks = [p for p in sorted(ROOT.rglob("Cargo.lock"))
             if not SKIP_PARTS & set(p.parts)]
    problems, skipped = [], 0
    for lock in locks:
        toml = lock.with_name("Cargo.toml")
        if not toml.exists():
            continue
        if is_workspace_member(lock.parent):
            skipped += 1
            continue
        try:
            want = declared_transitive(lock.parent)
            have = {n.replace("_", "-") for n in LOCK_NAME.findall(lock.read_text(errors="replace"))}
        except OSError as e:
            print(f"  ?? {lock.parent.relative_to(ROOT)}: {e}")
            continue
        missing = sorted(n for n in want if n.replace("_", "-") not in have)
        if missing:
            problems.append((lock.parent.relative_to(ROOT), missing))

    print(f"scanned {len(locks)} lockfiles ({skipped} skipped as workspace members)")
    print(f"stale: {len(problems)}\n")
    for d, m in problems:
        print(f"  {d}")
        print(f"      declared but not locked: {', '.join(m)}")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
