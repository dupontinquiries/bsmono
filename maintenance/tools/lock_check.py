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

ROOT = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else "/home/claude/repo")
SKIP_PARTS = {"target", "vendor", ".git", "node_modules"}

SECTION = re.compile(r'^\s*\[(?:target\.[^\]]+\.)?(dependencies|dev-dependencies|build-dependencies)\]\s*$')
ANY_SECTION = re.compile(r'^\s*\[')
DEP_LINE = re.compile(r'^\s*["\']?([A-Za-z0-9_.-]+)["\']?\s*=\s*(.*)$')
PACKAGE_RENAME = re.compile(r'\bpackage\s*=\s*["\']([^"\']+)["\']')
LOCK_NAME = re.compile(r'^name = "([^"]+)"$', re.M)
WS_MEMBERS = re.compile(r'^\s*\[workspace\]', re.M)


def declared(text):
    """Dependency names as the LOCK will spell them (i.e. after `package =`)."""
    out, cur = set(), None
    for line in text.splitlines():
        m = SECTION.match(line)
        if m:
            cur = m.group(1); continue
        if ANY_SECTION.match(line):
            cur = None; continue
        if not cur:
            continue
        d = DEP_LINE.match(line)
        if not d:
            continue
        name, rhs = d.group(1), d.group(2)
        if name in {"version", "features", "path", "optional", "git", "branch",
                    "rev", "tag", "workspace", "default-features", "registry", "package"}:
            continue
        rename = PACKAGE_RENAME.search(rhs)
        out.add(rename.group(1) if rename else name)
    return out


def is_workspace_member(crate_dir):
    """True when an ancestor Cargo.toml declares a [workspace] — that root's
    lock is the one cargo uses, and this crate's own lock is vestigial."""
    for anc in list(crate_dir.parents):
        if not str(anc).startswith(str(ROOT)):
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
            want = declared(toml.read_text(errors="replace"))
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
